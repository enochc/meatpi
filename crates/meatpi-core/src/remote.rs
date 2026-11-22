//! Reading the car through MeatHandler instead of talking to the dongle.
//!
//! In AutoPID mode the WiCAN Pro polls the car itself and POSTs the decoded
//! values to MeatHandler, so the dongle's ELM327 socket is closed. This module
//! fetches `GET /api/latest` from MeatHandler and turns it into the same
//! [`Sample`] the TCP path produces, so the UI does not care where data came
//! from. It also works away from home, since MeatHandler is reachable at
//! meathandler.ddns.net.

use crate::error::{Error, Result};
use crate::telemetry::Sample;
#[cfg(feature = "remote")]
use base64::Engine;
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_URL: &str = "https://meathandler.ddns.net:8443";

/// WiCAN AutoPID names from the dongle's "Kia: EV6" vehicle profile, mapped to
/// the keys in [`crate::profile`]. Both read the same 0x7E4 BMS frames, so the
/// units and signs already agree (e.g. `HV_A` is positive when discharging,
/// like `pack_current`).
pub const KEY_MAP: &[(&str, &str)] = &[
    ("SOC_D", "soc_display"),
    ("SOC", "soc_bms"),
    ("HV_V", "pack_voltage"),
    ("HV_A", "pack_current"),
    ("SOH", "soh"),
    ("HV_T_MIN", "batt_min_temp"),
    ("HV_T_MAX", "batt_max_temp"),
    ("HV_C_V_MIN", "min_cell_v"),
    ("HV_C_V_MAX", "max_cell_v"),
    ("LV_V", "aux_voltage"),
    ("REGEN_MAX", "max_regen_kw"),
    ("POWER_MAX", "max_power_kw"),
    ("HV_C_DIFF", "cell_v_deviation"),
    // 0/1 flags. The stock profile reads these from the wrong byte on the
    // 2024 EV6; the corrected expressions are in docs/CHARGE-FLAGS.md.
    ("CHARGING", "charging"),
    ("AC_PLUG", "plugged"),
];

/// One reading of MeatHandler's view of the car.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub sample: Sample,
    /// Time since the dongle last posted. `None` if it never has.
    pub age: Option<Duration>,
}

/// Blocking HTTP client for `/api/latest`. Native builds only; the web build
/// fetches through the browser and calls [`parse_latest`] directly.
#[cfg(feature = "remote")]
pub struct RemoteClient {
    agent: ureq::Agent,
    latest_url: String,
    auth: Option<String>,
}

#[cfg(feature = "remote")]
impl RemoteClient {
    /// `base_url` is the server root, e.g. [`DEFAULT_URL`]. An empty password
    /// sends no credentials.
    pub fn new(base_url: &str, user: &str, pass: &str, timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        let auth = (!pass.is_empty()).then(|| {
            let raw = format!("{user}:{pass}");
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
        });
        Self {
            agent,
            latest_url: format!("{}/api/latest", base_url.trim().trim_end_matches('/')),
            auth,
        }
    }

    pub fn url(&self) -> &str {
        &self.latest_url
    }

    pub fn fetch(&self) -> Result<Snapshot> {
        let mut req = self.agent.get(&self.latest_url);
        if let Some(auth) = &self.auth {
            req = req.set("Authorization", auth);
        }
        let body = match req.call() {
            Ok(resp) => resp.into_string().map_err(Error::Io)?,
            Err(ureq::Error::Status(code, _)) => return Err(Error::Http(code)),
            Err(ureq::Error::Transport(t)) => {
                return Err(Error::Io(std::io::Error::other(t.to_string())))
            }
        };
        parse_latest(&body)
    }
}

/// Turn an `/api/latest` body into a [`Snapshot`]. Split out from the HTTP
/// call so it can be tested without a server.
pub fn parse_latest(body: &str) -> Result<Snapshot> {
    let json: Value = serde_json::from_str(body).map_err(|e| Error::BadReply(e.to_string()))?;
    let values = json
        .get("values")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::BadReply("no `values` object".into()))?;

    let mut sample = Sample::default();
    for (wican, key) in KEY_MAP {
        match values.get(*wican).and_then(|v| v.get("value")).and_then(Value::as_f64) {
            Some(v) => sample.insert(key, v),
            None => sample.stale.push(key),
        }
    }

    let now = json.get("now").and_then(Value::as_i64);
    let last = json.get("last_post").and_then(Value::as_i64);
    let age = match (now, last) {
        (Some(n), Some(l)) => Some(Duration::from_millis(n.saturating_sub(l).max(0) as u64)),
        _ => None,
    };

    Ok(Snapshot { sample, age })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wican_names_onto_profile_keys() {
        let body = r#"{
            "now": 1795292000000, "last_post": 1795291990000, "posts_24h": 12,
            "values": {
                "SOC_D": {"ts": 1, "value": 81.5},
                "SOC":   {"ts": 1, "value": 78.0},
                "HV_V":  {"ts": 1, "value": 700.0},
                "HV_A":  {"ts": 1, "value": -10.0},
                "LV_V":  {"ts": 1, "value": 14.2},
                "TYRE_P_FL": {"ts": 1, "value": 41}
            }
        }"#;
        let snap = parse_latest(body).unwrap();
        assert_eq!(snap.sample.soc(), Some(81.5));
        assert_eq!(snap.sample.get("aux_voltage"), Some(14.2));
        assert!(snap.sample.stale.contains(&"charging"));
        assert_eq!(snap.sample.power_kw(), Some(-7.0));
        assert_eq!(snap.age, Some(Duration::from_secs(10)));
        assert!(snap.sample.stale.contains(&"soh"));
        assert!(!snap.sample.stale.contains(&"pack_voltage"));
    }

    #[test]
    fn handles_a_server_that_has_never_heard_from_the_car() {
        let snap = parse_latest(r#"{"now": 5, "last_post": null, "values": {}}"#).unwrap();
        assert!(snap.sample.is_empty());
        assert_eq!(snap.age, None);
        assert_eq!(snap.sample.stale.len(), KEY_MAP.len());
    }

    #[test]
    fn rejects_unrelated_json() {
        assert!(parse_latest(r#"{"hello": 1}"#).is_err());
    }
}
