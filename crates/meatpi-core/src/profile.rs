//! Vehicle profiles: which requests to send, and how to turn the reply bytes
//! into physical values.
//!
//! Offsets below are indices into the payload that *follows* the positive
//! response marker, i.e. the byte after `62 01 01` is index 0. That matches the
//! `a, b, c, ...` lettering used by the Torque/HKMC community PID lists, where
//! `a` = 0 and `aa` = 26.

/// How to pull one number out of a response payload.
#[derive(Debug, Clone, Copy)]
pub enum Decode {
    U8 { at: usize, scale: f64, offset: f64 },
    I8 { at: usize, scale: f64, offset: f64 },
    U16 { at: usize, scale: f64, offset: f64 },
    I16 { at: usize, scale: f64, offset: f64 },
    U32 { at: usize, scale: f64, offset: f64 },
}

impl Decode {
    pub fn apply(&self, payload: &[u8]) -> Option<f64> {
        let raw = match *self {
            Decode::U8 { at, .. } => *payload.get(at)? as f64,
            Decode::I8 { at, .. } => *payload.get(at)? as i8 as f64,
            Decode::U16 { at, .. } => {
                u16::from_be_bytes([*payload.get(at)?, *payload.get(at + 1)?]) as f64
            }
            Decode::I16 { at, .. } => {
                i16::from_be_bytes([*payload.get(at)?, *payload.get(at + 1)?]) as f64
            }
            Decode::U32 { at, .. } => u32::from_be_bytes([
                *payload.get(at)?,
                *payload.get(at + 1)?,
                *payload.get(at + 2)?,
                *payload.get(at + 3)?,
            ]) as f64,
        };
        let (scale, offset) = match *self {
            Decode::U8 { scale, offset, .. }
            | Decode::I8 { scale, offset, .. }
            | Decode::U16 { scale, offset, .. }
            | Decode::I16 { scale, offset, .. }
            | Decode::U32 { scale, offset, .. } => (scale, offset),
        };
        Some(raw * scale + offset)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Signal {
    /// Stable identifier used as the key in a [`crate::telemetry::Sample`].
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub decode: Decode,
}

#[derive(Debug, Clone, Copy)]
pub struct PidRequest {
    /// What to write to the adapter, e.g. `"220101"`.
    pub cmd: &'static str,
    /// CAN id to address with `ATSH`, e.g. `"7E4"` for the HKMC battery ECU.
    pub header: &'static str,
    /// Positive-response bytes that mark the start of the payload.
    pub marker: &'static [u8],
    pub signals: &'static [Signal],
}

#[derive(Debug, Clone, Copy)]
pub struct VehicleProfile {
    pub name: &'static str,
    /// Sent once after the generic ELM327 reset, before any polling.
    pub init_commands: &'static [&'static str],
    pub requests: &'static [PidRequest],
}

// ---------------------------------------------------------------------------
// Kia EV6 (E-GMP) — shares its BMS layout with the Ioniq 5/6 and the earlier
// Kona/Niro EV PID lists these offsets are taken from.
// ---------------------------------------------------------------------------

/// Battery ECU, frame 1: pack voltage/current, power limits, module temps.
const BMS_0101: PidRequest = PidRequest {
    cmd: "220101",
    header: "7E4",
    marker: &[0x62, 0x01, 0x01],
    signals: &[
        Signal {
            key: "soc_bms",
            label: "SOC (BMS)",
            unit: "%",
            decode: Decode::U8 { at: 4, scale: 0.5, offset: 0.0 },
        },
        // UNVERIFIED: both read 0.00 with the car stationary in Park, which is
        // defensible but proves nothing. Confirm these while actually driving.
        Signal {
            key: "max_regen_kw",
            label: "Max regen",
            unit: "kW",
            decode: Decode::U16 { at: 5, scale: 0.01, offset: 0.0 },
        },
        Signal {
            key: "max_power_kw",
            label: "Max discharge",
            unit: "kW",
            decode: Decode::U16 { at: 7, scale: 0.01, offset: 0.0 },
        },
        Signal {
            key: "pack_current",
            label: "Pack current",
            unit: "A",
            decode: Decode::I16 { at: 10, scale: 0.1, offset: 0.0 },
        },
        Signal {
            key: "pack_voltage",
            label: "Pack voltage",
            unit: "V",
            decode: Decode::U16 { at: 12, scale: 0.1, offset: 0.0 },
        },
        Signal {
            key: "batt_max_temp",
            label: "Battery max temp",
            unit: "°C",
            decode: Decode::I8 { at: 14, scale: 1.0, offset: 0.0 },
        },
        Signal {
            key: "batt_min_temp",
            label: "Battery min temp",
            unit: "°C",
            decode: Decode::I8 { at: 15, scale: 1.0, offset: 0.0 },
        },
        // Verified on the car: 3.88 V each, and 747.7 V / 3.88 V = 192.7,
        // matching the 192 cells in the 77.4 kWh pack.
        Signal {
            key: "max_cell_v",
            label: "Highest cell",
            unit: "V",
            decode: Decode::U8 { at: 23, scale: 0.02, offset: 0.0 },
        },
        Signal {
            key: "min_cell_v",
            label: "Lowest cell",
            unit: "V",
            decode: Decode::U8 { at: 25, scale: 0.02, offset: 0.0 },
        },
        Signal {
            key: "aux_voltage",
            label: "12V battery",
            unit: "V",
            decode: Decode::U8 { at: 29, scale: 0.1, offset: 0.0 },
        },
    ],
};

/// Battery ECU, frame 5: state of health and the dashboard SOC.
const BMS_0105: PidRequest = PidRequest {
    cmd: "220105",
    header: "7E4",
    marker: &[0x62, 0x01, 0x05],
    signals: &[
        Signal {
            key: "cell_v_deviation",
            label: "Cell deviation",
            unit: "V",
            decode: Decode::U8 { at: 20, scale: 0.02, offset: 0.0 },
        },
        Signal {
            key: "heater_temp",
            label: "Battery heater",
            unit: "°C",
            decode: Decode::I8 { at: 23, scale: 1.0, offset: 0.0 },
        },
        Signal {
            key: "soh",
            label: "State of health",
            unit: "%",
            decode: Decode::U16 { at: 25, scale: 0.1, offset: 0.0 },
        },
        Signal {
            key: "soc_display",
            label: "SOC (dash)",
            unit: "%",
            decode: Decode::U8 { at: 31, scale: 0.5, offset: 0.0 },
        },
    ],
};

// The legislated OBD-II PIDs `010D` (speed) and `0146` (ambient) were tried on
// this car and both answer `NO DATA` — like many EVs, the EV6 does not expose
// the mode 01 set. They are omitted rather than left to fail on every poll.

pub const KIA_EV6: VehicleProfile = VehicleProfile {
    name: "Kia EV6 (2024)",
    // ATSP6 = ISO 15765-4, 11-bit ids, 500 kbaud — the E-GMP diagnostic bus.
    // The ATFC* trio pre-loads a flow control frame so long BMS responses are
    // not truncated. This mirrors the init string MeatPi ships in their own
    // Hyundai/Kia vehicle profiles.
    init_commands: &["ATSP6", "ATFCSH7E4", "ATFCSD300000", "ATFCSM0"],
    requests: &[BMS_0101, BMS_0105],
};

/// Everything the app knows how to talk to.
pub const PROFILES: &[VehicleProfile] = &[KIA_EV6];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_soc_and_voltage_from_a_real_shaped_frame() {
        // payload indices:      0     1     2     3     4(soc) 5     6
        let payload: Vec<u8> = vec![
            0xFF, 0xF7, 0xE7, 0x00, 0xB4, 0x0F, 0xA0, 0x1D, 0x4C, 0x00, 0x00, 0x64, 0x0E, 0xC4,
            0x1A, 0x18, 0x19, 0x19, 0x19, 0x19, 0x0A, 0x00, 0x14, 0x0F, 0x00, 0x03, 0xE8, 0x00,
            0x00, 0x8C,
        ];
        let get = |key: &str| {
            BMS_0101
                .signals
                .iter()
                .find(|s| s.key == key)
                .and_then(|s| s.decode.apply(&payload))
        };
        assert_eq!(get("soc_bms"), Some(90.0)); // 0xB4 = 180, /2
        assert_eq!(get("pack_current"), Some(10.0)); // 0x0064 = 100, /10
        assert_eq!(get("pack_voltage"), Some(378.0)); // 0x0EC4 = 3780, /10
        assert_eq!(get("batt_max_temp"), Some(26.0));
        assert_eq!(get("aux_voltage"), Some(14.0)); // 0x8C = 140, *0.1
    }

    #[test]
    fn negative_current_is_signed() {
        let mut payload = vec![0u8; 32];
        payload[10] = 0xFF;
        payload[11] = 0x9C; // -100 -> -10.0 A (regen)
        let sig = BMS_0101
            .signals
            .iter()
            .find(|s| s.key == "pack_current")
            .unwrap();
        assert_eq!(sig.decode.apply(&payload), Some(-10.0));
    }

    #[test]
    fn short_payload_yields_none_rather_than_panicking() {
        let sig = BMS_0105
            .signals
            .iter()
            .find(|s| s.key == "soc_display")
            .unwrap();
        assert_eq!(sig.decode.apply(&[0x01, 0x02]), None);
    }
}
