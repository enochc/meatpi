//! Push a phone notification when the car is plugged in but not charging.
//!
//! Charging means pack power (`HV_W`) below -1 kW with the car not moving:
//! energy is flowing into the battery and it is not regen. Once charging, it
//! stays "charging" until power rises above -300 W, so a brief dip does not
//! end the session. "Plugged in" is the VCMS AC plug bit (`AC_PLUG`).
//!
//! The dongle's `CHARGING` value is deliberately ignored. The bit it reads
//! (`220101 B63:1`) turned out to mean "HV system on": it is also set while
//! driving and when a parked car wakes to top up its 12 V battery, which
//! produced false "charging started" alerts. See docs/CHARGE-FLAGS.md.
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
    /// 12 V reading below which to warn (MH_LOW_12V, default 12.2 V: about
    /// half charge for a resting lead-acid battery).
    pub low_12v: f64,
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
            low_12v: std::env::var("MH_LOW_12V").ok().and_then(|v| v.parse().ok()).unwrap_or(12.2),
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
    pub pack_w: Option<f64>,
    pub speed: Option<f64>,
    pub soc: Option<f64>,
    /// 12 V battery (`LV_V`).
    pub lv_v: Option<f64>,
}

impl Reading {
    fn charging(&self, was_charging: bool) -> bool {
        let still = self.speed.map_or(true, |s| s < STILL_KMH);
        let threshold = if was_charging { CHARGE_END_W } else { CHARGE_START_W };
        still && self.pack_w.is_some_and(|w| w <= threshold)
    }
}

#[derive(Debug, PartialEq)]
pub enum Notice {
    PluggedNotCharging { soc: Option<f64> },
    ChargingStarted { soc: Option<f64>, after_alert: bool },
    Low12V { volts: f64 },
}

/// A 12 V warning is cleared once the battery reads at least this much again.
const LOW_12V_CLEAR: f64 = 12.6;

/// The decision logic, kept free of I/O so it can be tested.
#[derive(Default)]
pub struct Watch {
    was_charging: Option<bool>,
    plugged_since: Option<i64>,
    alerted: bool,
    low_12v_alerted: bool,
}

impl Watch {
    /// One warning per low spell: fires when the 12 V reads under `limit`,
    /// re-arms only after it recovers to `LOW_12V_CLEAR`.
    pub fn step_12v(&mut self, r: &Reading, limit: f64) -> Option<Notice> {
        let v = r.lv_v?;
        if v >= LOW_12V_CLEAR {
            self.low_12v_alerted = false;
        } else if v < limit && !self.low_12v_alerted {
            self.low_12v_alerted = true;
            return Some(Notice::Low12V { volts: v });
        }
        None
    }

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
        pack_w: get("HV_W"),
        speed: get("SPEED"),
        soc: get("SOC_D"),
        lv_v: get("LV_V"),
    };
    // Nothing fresh at all means the dongle is asleep: no opinion.
    (reading.plug_bit.is_some() || reading.pack_w.is_some())
        .then_some(reading)
}

fn soc_text(soc: Option<f64>) -> String {
    soc.map(|s| format!(" Battery at {s:.0}%.")).unwrap_or_default()
}

/// Post one ntfy notification. Blocking; call from a blocking task.
pub fn notify(cfg: &Config, title: &str, body: &str, priority: &str, tags: &str) {
    let url = format!("{}/{}", cfg.server, cfg.topic);
    let result = ureq::post(&url)
        .timeout(Duration::from_secs(15))
        .set("Title", title)
        .set("Priority", priority)
        .set("Tags", tags)
        .send_string(body);
    match result {
        Ok(_) => tracing::info!(title, "notification sent"),
        Err(e) => tracing::warn!("notification failed: {e}"),
    }
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
        Notice::Low12V { volts } => (
            "EV6 12V battery low",
            format!(
                "The 12V battery read {volts:.1}V. Drive or charge the car soon; \
                 an EV6 will not start with a flat 12V battery."
            ),
            "high",
            "warning,battery",
        ),
    };
    notify(cfg, title, &body, priority, tags);
}

pub async fn run(state: AppState, cfg: std::sync::Arc<Config>) {
    tracing::info!(
        server = %cfg.server,
        alert_after_s = cfg.alert_after_ms / 1000,
        "charge watch on"
    );
    let mut watch = Watch::default();
    let mut tick = tokio::time::interval(CHECK_EVERY);
    loop {
        tick.tick().await;
        let Some(reading) = read(&state) else { continue };
        if let Some(notice) = watch.step_12v(&reading, cfg.low_12v) {
            tracing::info!(?notice, "charge watch");
            let cfg = cfg.clone();
            tokio::task::spawn_blocking(move || send(&cfg, &notice));
        }
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

    fn r(now_s: i64, plugged: bool, w: f64, kmh: f64) -> Reading {
        Reading {
            now: now_s * 1000,
            plug_bit: Some(plugged),
            pack_w: Some(w),
            speed: Some(kmh),
            soc: Some(50.0),
            lv_v: None,
        }
    }

    const T: i64 = 120_000;

    #[test]
    fn warns_once_per_low_12v_spell() {
        let mut w = Watch::default();
        let v = |volts| Reading { lv_v: Some(volts), ..r(0, false, 0.0, 0.0) };
        assert_eq!(w.step_12v(&v(12.7), 12.2), None);
        assert_eq!(w.step_12v(&v(12.1), 12.2), Some(Notice::Low12V { volts: 12.1 }));
        assert_eq!(w.step_12v(&v(11.9), 12.2), None);
        // Partial recovery does not re-arm...
        assert_eq!(w.step_12v(&v(12.4), 12.2), None);
        assert_eq!(w.step_12v(&v(12.0), 12.2), None);
        // ...a real one does.
        assert_eq!(w.step_12v(&v(14.3), 12.2), None);
        assert_eq!(w.step_12v(&v(12.0), 12.2), Some(Notice::Low12V { volts: 12.0 }));
    }

    #[test]
    fn alerts_once_when_plugged_and_idle() {
        let mut w = Watch::default();
        assert_eq!(w.step(r(0, false, 0.0, 0.0), T), None);
        assert_eq!(w.step(r(10, true, 0.0, 0.0), T), None);
        assert_eq!(w.step(r(100, true, 0.0, 0.0), T), None);
        assert_eq!(w.step(r(130, true, 0.0, 0.0), T), Some(Notice::PluggedNotCharging { soc: Some(50.0) }));
        assert_eq!(w.step(r(300, true, 0.0, 0.0), T), None);
        assert_eq!(
            w.step(r(310, true, -3700.0, 0.0), T),
            Some(Notice::ChargingStarted { soc: Some(50.0), after_alert: true })
        );
    }

    #[test]
    fn charging_follows_pack_power_with_hysteresis() {
        let mut w = Watch::default();
        assert_eq!(w.step(r(0, true, 150.0, 0.0), T), None);
        assert_eq!(
            w.step(r(10, true, -3700.0, 0.0), T),
            Some(Notice::ChargingStarted { soc: Some(50.0), after_alert: false })
        );
        assert_eq!(w.step(r(20, true, -500.0, 0.0), T), None);
        assert_eq!(w.step(r(30, true, -3700.0, 0.0), T), None);
        assert_eq!(w.step(r(40, true, 100.0, 0.0), T), None);
        assert!(matches!(w.step(r(50, true, -3600.0, 0.0), T), Some(Notice::ChargingStarted { .. })));
    }

    /// The three false alarms from 2026-11-21/22, replayed.
    #[test]
    fn no_alert_for_regen_12v_top_up_or_parked_hvac() {
        let mut w = Watch::default();
        w.step(r(0, false, 0.0, 0.0), T);
        // Regen while driving at 27 km/h.
        assert_eq!(w.step(r(10, false, -2017.0, 27.0), T), None);
        assert_eq!(w.step(r(20, false, 0.0, 0.0), T), None);
        // Parked, waking to top up the 12 V battery: power flows out.
        assert_eq!(w.step(r(30, false, 374.6, 0.0), T), None);
        // Parked with the car on (HVAC etc.).
        assert_eq!(w.step(r(40, false, 3664.0, 0.0), T), None);
    }

    #[test]
    fn restart_mid_charge_is_silent() {
        let mut w = Watch::default();
        assert_eq!(w.step(r(0, true, -3700.0, 0.0), T), None);
        assert_eq!(w.step(r(10, true, -3700.0, 0.0), T), None);
    }
}
