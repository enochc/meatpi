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

    /// Whether the car is charging: more than 1 kW flowing into the pack
    /// while it is not moving (moving means regen). `None` if power is
    /// unknown. There is no reliable charging flag on the 2024 EV6; the one
    /// that looked like it means "HV system on" (docs/CHARGE-FLAGS.md).
    pub fn is_charging(&self) -> Option<bool> {
        let still = self.get("speed").map_or(true, |kmh| kmh < 2.0);
        self.power_kw().map(|kw| still && kw <= -1.0)
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
    fn regen_while_moving_is_not_charging() {
        let mut s = Sample::default();
        s.insert("pack_voltage", 750.0);
        s.insert("pack_current", -20.0);
        s.insert("speed", 40.0);
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
