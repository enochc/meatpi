//! Push a phone notification when the car is plugged in but not charging.
//!
//! Charging is detected two ways, either one is enough:
//! - the `CHARGING` bit from the dongle's "Kia: EV6" profile, and
//! - pack power (`HV_W`): below -1 kW with the car not moving means energy is
//!   flowing into the battery. Once charging, it stays "charging" until power
//!   rises above -300 W, so a brief dip does not end the session.
//!
//! The power check is a backstop. The dongle profile's original `CHARGING`
//! and `AC_PLUG` bits were wrong for the 2024 EV6 (they always read 0); the
//! corrected expressions are in `docs/CHARGE-FLAGS.md` of the meatpi repo.
//!
//! If the plug is in and charging has not started within
//! `MH_PLUG_ALERT_SECS` (default 120), it sends one alert for that plug-in.
//! It can also announce when charging starts, which makes the "does the
//! dongle block charging?" test easy to read.
//!
//! Notifications go to ntfy (https://ntfy.sh): a plain HTTPS POST to a topic,
//! which the ntfy phone app subscribes to. The topic name is the only secret,
//! so it is random and long.
//!
//! Limits: this only sees what the dongle reports. If the dongle is asleep
//! when the car is plugged in and never wakes, nothing arrives to alert on.
//! A full battery or a charging schedule also looks like "plugged in, not
//! charging".

use crate::AppState;
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

const CHECK_EVERY: Duration = Duration::from_secs(15);
/// Readings older than this are from before the dongle went to sleep and say
/// nothing about now.
const FRESH_MS: i64 = 60_000;

pub struct Config {
    pub server: String,
    pub topic: String,
    pub alert_after_ms: i64,
    pub announce_start: bool,
}

impl Config {
    pub fn from_env() -> Option<Self> {
        let topic = std::env::var("MH_NTFY_TOPIC").ok().filter(|t| !t.trim().is_empty())?;
        let secs: i64 = std::env::var("MH_PLUG_ALERT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120);
        Some(Self {
            server: std::env::var("MH_NTFY_SERVER")
                .unwrap_or_else(|_| "https://ntfy.sh".into())
                .trim_end_matches('/')
                .to_string(),
            topic: topic.trim().to_string(),
            alert_after_ms: secs * 1000,
            announce_start: std::env::var("MH_NOTIFY_CHARGE_START").map_or(true, |v| v != "0"),
        })
    }
}

/// Pack power below this (W, negative = into the battery) starts a charge...
const CHARGE_START_W: f64 = -1000.0;
/// ...and above this ends one.
const CHARGE_END_W: f64 = -300.0;
/// km/h. Regen also pushes power into the pack, but only while moving.
const STILL_KMH: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub now: i64,
    pub plug_bit: Option<bool>,
    pub charging_bit: Option<bool>,
    pub pack_w: Option<f64>,
    pub speed: Option<f64>,
    pub soc: Option<f64>,
}

impl Reading {
    fn charging(&self, was_charging: bool) -> bool {
        if self.charging_bit == Some(true) {
            return true;
        }
        let still = self.speed.map_or(true, |s| s < STILL_KMH);
        let threshold = if was_charging { CHARGE_END_W } else { CHARGE_START_W };
        still && self.pack_w.is_some_and(|w| w <= threshold)
    }
}

#[derive(Debug, PartialEq)]
pub enum Notice {
    PluggedNotCharging { soc: Option<f64> },
    ChargingStarted { soc: Option<f64>, after_alert: bool },
}

/// The decision logic, kept free of I/O so it can be tested.
#[derive(Default)]
pub struct Watch {
    was_charging: Option<bool>,
    plugged_since: Option<i64>,
    alerted: bool,
}

impl Watch {
    pub fn step(&mut self, r: Reading, alert_after_ms: i64) -> Option<Notice> {
        // The first reading after startup only sets the baseline, so a
        // restart in the middle of a charge does not announce anything.
        let charging = r.charging(self.was_charging.unwrap_or(false));
        let plugged = charging || r.plug_bit == Some(true);
        let Some(was_charging) = self.was_charging.replace(charging) else {
            tracing::info!(?r, charging, plugged, "charge watch baseline");
            if plugged && !charging {
                self.plugged_since = Some(r.now);
            }
            return None;
        };
        if charging != was_charging {
            tracing::info!(?r, charging, "charge watch: charging changed");
        }

        if charging {
            let after_alert = std::mem::take(&mut self.alerted);
            self.plugged_since = None;
            return (!was_charging).then_some(Notice::ChargingStarted { soc: r.soc, after_alert });
        }

        if !plugged {
            self.plugged_since = None;
            self.alerted = false;
            return None;
        }

        let since = *self.plugged_since.get_or_insert(r.now);
        if !self.alerted && r.now - since >= alert_after_ms {
            self.alerted = true;
            return Some(Notice::PluggedNotCharging { soc: r.soc });
        }
        None
    }
}

fn latest_num(db: &rusqlite::Connection, key: &str, now: i64) -> rusqlite::Result<Option<f64>> {
    let row: Option<(i64, String)> = db
        .query_row("SELECT ts, value FROM latest WHERE key = ?1", params![key], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    Ok(row
        .filter(|(ts, _)| now - ts <= FRESH_MS)
        .and_then(|(_, v)| v.parse::<f64>().ok()))
}

fn read(state: &AppState) -> Option<Reading> {
    let db = state.db.lock().unwrap_or_else(|p| p.into_inner());
    let now = crate::now_ms();
    let get = |key| latest_num(&db, key, now).ok().flatten();
    let reading = Reading {
        now,
        plug_bit: get("AC_PLUG").map(|v| v >= 0.5),
        charging_bit: get("CHARGING").map(|v| v >= 0.5),
        pack_w: get("HV_W"),
        speed: get("SPEED"),
        soc: get("SOC_D"),
    };
    // Nothing fresh at all means the dongle is asleep: no opinion.
    (reading.plug_bit.is_some() || reading.charging_bit.is_some() || reading.pack_w.is_some())
        .then_some(reading)
}

fn soc_text(soc: Option<f64>) -> String {
    soc.map(|s| format!(" Battery at {s:.0}%.")).unwrap_or_default()
}

fn send(cfg: &Config, notice: &Notice) {
    let (title, body, priority, tags) = match notice {
        Notice::PluggedNotCharging { soc } => (
            "EV6 plugged in, not charging",
            format!(
                "Plugged in for {} min and charging has not started.{} \
                 Ignore this if it is scheduled or full.",
                cfg.alert_after_ms / 60_000,
                soc_text(*soc)
            ),
            "high",
            "warning,electric_plug",
        ),
        Notice::ChargingStarted { soc, after_alert } => (
            if *after_alert { "EV6 charging started after all" } else { "EV6 charging started" },
            format!("Charging is running.{}", soc_text(*soc)),
            "default",
            "zap",
        ),
    };
    let url = format!("{}/{}", cfg.server, cfg.topic);
    let result = ureq::post(&url)
        .timeout(Duration::from_secs(15))
        .set("Title", title)
        .set("Priority", priority)
        .set("Tags", tags)
        .send_string(&body);
    match result {
        Ok(_) => tracing::info!(title, "notification sent"),
        Err(e) => tracing::warn!("notification failed: {e}"),
    }
}

pub async fn run(state: AppState, cfg: Config) {
    tracing::info!(
        server = %cfg.server,
        alert_after_s = cfg.alert_after_ms / 1000,
        "charge watch on"
    );
    let cfg = std::sync::Arc::new(cfg);
    let mut watch = Watch::default();
    let mut tick = tokio::time::interval(CHECK_EVERY);
    loop {
        tick.tick().await;
        let Some(reading) = read(&state) else { continue };
        if let Some(notice) = watch.step(reading, cfg.alert_after_ms) {
            // A charge that starts normally is only worth a ping if asked for;
            // one that starts after an alert always is.
            if matches!(notice, Notice::ChargingStarted { after_alert: false, .. }) && !cfg.announce_start {
                continue;
            }
            tracing::info!(?notice, "charge watch");
            let cfg = cfg.clone();
            // ureq blocks, so keep it off the async workers.
            tokio::task::spawn_blocking(move || send(&cfg, &notice));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(now_s: i64, plugged: bool, charging: bool) -> Reading {
        Reading {
            now: now_s * 1000,
            plug_bit: Some(plugged),
            charging_bit: Some(charging),
            pack_w: Some(0.0),
            speed: Some(0.0),
            soc: Some(50.0),
        }
    }

    fn power(now_s: i64, w: f64, kmh: f64) -> Reading {
        Reading {
            now: now_s * 1000,
            plug_bit: Some(false),
            charging_bit: Some(false),
            pack_w: Some(w),
            speed: Some(kmh),
            soc: Some(50.0),
        }
    }

    const T: i64 = 120_000;

    #[test]
    fn alerts_once_when_plugged_and_idle() {
        let mut w = Watch::default();
        assert_eq!(w.step(bits(0, false, false), T), None);
        assert_eq!(w.step(bits(10, true, false), T), None);
        assert_eq!(w.step(bits(100, true, false), T), None);
        assert_eq!(w.step(bits(130, true, false), T), Some(Notice::PluggedNotCharging { soc: Some(50.0) }));
        assert_eq!(w.step(bits(300, true, false), T), None);
        assert_eq!(
            w.step(bits(310, true, true), T),
            Some(Notice::ChargingStarted { soc: Some(50.0), after_alert: true })
        );
    }

    #[test]
    fn detects_charging_from_pack_power_when_the_bits_lie() {
        let mut w = Watch::default();
        assert_eq!(w.step(power(0, 150.0, 0.0), T), None);
        assert_eq!(
            w.step(power(10, -3700.0, 0.0), T),
            Some(Notice::ChargingStarted { soc: Some(50.0), after_alert: false })
        );
        // A dip to -500 W is still charging (hysteresis)...
        assert_eq!(w.step(power(20, -500.0, 0.0), T), None);
        assert_eq!(w.step(power(30, -3700.0, 0.0), T), None);
        // ...a stop and restart announces again.
        assert_eq!(w.step(power(40, 100.0, 0.0), T), None);
        assert!(matches!(w.step(power(50, -3600.0, 0.0), T), Some(Notice::ChargingStarted { .. })));
    }

    #[test]
    fn regen_while_driving_is_not_charging() {
        let mut w = Watch::default();
        w.step(power(0, 20_000.0, 60.0), T);
        assert_eq!(w.step(power(10, -30_000.0, 55.0), T), None);
        assert_eq!(w.step(power(20, -5_000.0, 40.0), T), None);
    }

    #[test]
    fn restart_mid_charge_is_silent() {
        let mut w = Watch::default();
        assert_eq!(w.step(power(0, -3700.0, 0.0), T), None);
        assert_eq!(w.step(power(10, -3700.0, 0.0), T), None);
    }
}
