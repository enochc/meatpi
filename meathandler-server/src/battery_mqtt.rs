//! Low 12 V alerts from the WiCAN's own battery monitor, over MQTT.
//!
//! The dongle's regular posts only arrive while it is polling the car, and
//! with Automate's minimum voltage at 13 V that is only while the car is
//! powering its 12 V. So a parked car slowly draining its 12 V produces no
//! posts at all. The WiCAN's "Battery Alert" covers that gap: while asleep
//! (under 1 mA) it checks its own supply voltage, and when it is under the
//! threshold it wakes, joins Wi-Fi and publishes to an MQTT broker. The broker
//! is Mosquitto on the Pi (deploy/setup-mqtt.sh), reachable only on the LAN.
//! This task subscribes and turns those messages into ntfy notifications.

use crate::charge_watch::{self, Config as NotifyConfig};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Re-notify at most this often while the battery stays low...
const REPEAT_MS: i64 = 6 * 3600 * 1000;
/// ...unless it has dropped by at least this much since the last alert.
const REPEAT_DROP_V: f64 = 0.2;

pub struct MqttConfig {
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
    topic: String,
}

impl MqttConfig {
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            host: var("MH_MQTT_HOST")?,
            port: var("MH_MQTT_PORT").and_then(|p| p.parse().ok()).unwrap_or(1883),
            user: var("MH_MQTT_USER"),
            pass: var("MH_MQTT_PASS"),
            topic: var("MH_MQTT_TOPIC").unwrap_or_else(|| "meathandler/battery".into()),
        })
    }
}

/// Pull a voltage out of whatever the dongle published: a bare number, or
/// JSON with a key mentioning "volt" (or failing that, the first number).
pub fn extract_voltage(payload: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(payload).ok()?.trim();
    if let Ok(v) = text.trim_end_matches('V').trim().parse::<f64>() {
        return Some(v);
    }
    let json: Value = serde_json::from_str(text).ok()?;
    fn as_num(v: &Value) -> Option<f64> {
        v.as_f64().or_else(|| v.as_str()?.trim().trim_end_matches('V').trim().parse().ok())
    }
    fn walk(v: &Value, prefer_volt: bool) -> Option<f64> {
        match v {
            Value::Object(m) => {
                if prefer_volt {
                    if let Some(n) = m
                        .iter()
                        .find(|(k, _)| k.to_lowercase().contains("volt"))
                        .and_then(|(_, v)| as_num(v))
                    {
                        return Some(n);
                    }
                }
                m.values().find_map(|v| walk(v, prefer_volt))
            }
            Value::Array(a) => a.iter().find_map(|v| walk(v, prefer_volt)),
            other if !prefer_volt => as_num(other),
            _ => None,
        }
    }
    walk(&json, true).or_else(|| walk(&json, false))
}

/// Whether this reading deserves a notification, given the last one sent.
fn worth_notifying(last: Option<(i64, f64)>, now: i64, volts: f64) -> bool {
    match last {
        None => true,
        Some((t, v)) => now - t >= REPEAT_MS || volts <= v - REPEAT_DROP_V,
    }
}

pub async fn run(cfg: MqttConfig, notify: Arc<NotifyConfig>) {
    let mut opts = MqttOptions::new("meathandler", cfg.host.clone(), cfg.port);
    opts.set_keep_alive(Duration::from_secs(60));
    if let (Some(u), Some(p)) = (&cfg.user, &cfg.pass) {
        opts.set_credentials(u.clone(), p.clone());
    }
    let (client, mut events) = AsyncClient::new(opts, 16);
    tracing::info!(host = %cfg.host, port = cfg.port, topic = %cfg.topic, "battery alert listener on");

    let mut last: Option<(i64, f64)> = None;
    loop {
        match events.poll().await {
            // Subscribe on every (re)connect; the session is not persistent.
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                if let Err(e) = client.subscribe(&cfg.topic, QoS::AtLeastOnce).await {
                    tracing::warn!("MQTT subscribe failed: {e}");
                }
            }
            Ok(Event::Incoming(Packet::Publish(msg))) => {
                let volts = extract_voltage(&msg.payload);
                let raw = String::from_utf8_lossy(&msg.payload).chars().take(200).collect::<String>();
                tracing::info!(topic = %msg.topic, ?volts, %raw, "battery alert from dongle");
                let now = crate::now_ms();
                let send = match volts {
                    Some(v) => worth_notifying(last, now, v).then(|| {
                        last = Some((now, v));
                        format!(
                            "The MeatPi reports the 12V battery at {v:.1}V while parked. \\
                             Drive or charge the car soon; an EV6 will not start with a flat 12V battery."
                        )
                    }),
                    None => Some(format!("The MeatPi sent a low-battery alert it could not read: {raw}")),
                };
                if let Some(body) = send {
                    let notify = notify.clone();
                    tokio::task::spawn_blocking(move || {
                        charge_watch::notify(&notify, "EV6 12V battery low", &body, "high", "warning,battery")
                    });
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("MQTT connection: {e}; retrying in 15 s");
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_voltage_from_likely_payloads() {
        assert_eq!(extract_voltage(b"11.8"), Some(11.8));
        assert_eq!(extract_voltage(b"11.8V"), Some(11.8));
        assert_eq!(extract_voltage(br#"{"battery_voltage": 11.8}"#), Some(11.8));
        assert_eq!(extract_voltage(br#"{"alert": "low", "voltage": "11.6V"}"#), Some(11.6));
        assert_eq!(extract_voltage(br#"{"id": 3, "v": 11.9}"#), Some(3.0));
        assert_eq!(extract_voltage(b"low battery"), None);
    }

    #[test]
    fn repeats_only_after_hours_or_a_further_drop() {
        let h = 3600 * 1000;
        assert!(worth_notifying(None, 0, 12.0));
        assert!(!worth_notifying(Some((0, 12.0)), h, 11.9));
        assert!(worth_notifying(Some((0, 12.0)), h, 11.8));
        assert!(worth_notifying(Some((0, 12.0)), 7 * h, 11.95));
    }
}
