use std::collections::HashMap;

/// One pass over every request in a profile.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    values: HashMap<&'static str, f64>,
    /// Requests that failed this round, for surfacing partial reads.
    pub stale: Vec<&'static str>,
}

impl Sample {
    pub fn insert(&mut self, key: &'static str, value: f64) {
        self.values.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.values.get(key).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&&'static str, &f64)> {
        self.values.iter()
    }

    /// Instantaneous pack power. Positive means the car is drawing from the
    /// battery, negative means regen or charging.
    pub fn power_kw(&self) -> Option<f64> {
        Some(self.get("pack_voltage")? * self.get("pack_current")? / 1000.0)
    }

    /// Prefer the dashboard SOC, which is the buffered number the driver sees;
    /// fall back to the raw BMS value.
    pub fn soc(&self) -> Option<f64> {
        self.get("soc_display").or_else(|| self.get("soc_bms"))
    }

    /// Whether the car is charging. Prefers the charging flag and falls back
    /// to pack power: more than 1 kW flowing into a car that is parked.
    /// `None` when neither is known.
    pub fn is_charging(&self) -> Option<bool> {
        if let Some(flag) = self.get("charging") {
            return Some(flag >= 0.5);
        }
        self.power_kw().map(|kw| kw <= -1.0)
    }

    /// Whether a charging cable is plugged in, if the car reports it.
    pub fn is_plugged(&self) -> Option<bool> {
        self.get("plugged").map(|v| v >= 0.5)
    }

    /// Merge a newer sample over this one, keeping previous values for any
    /// request that failed, so a single dropped frame does not blank the UI.
    pub fn merge_from(&mut self, newer: &Sample) {
        for (k, v) in newer.iter() {
            self.values.insert(k, *v);
        }
        self.stale = newer.stale.clone();
    }
}

#[cfg(test)]
mod charge_tests {
    use super::Sample;

    #[test]
    fn flag_wins_over_power() {
        let mut s = Sample::default();
        s.insert("pack_voltage", 750.0);
        s.insert("pack_current", -5.0);
        s.insert("charging", 0.0);
        assert_eq!(s.is_charging(), Some(false));
    }

    #[test]
    fn falls_back_to_pack_power() {
        let mut s = Sample::default();
        s.insert("pack_voltage", 750.0);
        s.insert("pack_current", -5.0); // -3.75 kW: into the battery
        assert_eq!(s.is_charging(), Some(true));
        s.insert("pack_current", 2.0);
        assert_eq!(s.is_charging(), Some(false));
        assert_eq!(s.is_plugged(), None);
    }
}
