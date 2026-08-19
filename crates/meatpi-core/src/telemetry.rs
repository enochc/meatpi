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

    /// Merge a newer sample over this one, keeping previous values for any
    /// request that failed, so a single dropped frame does not blank the UI.
    pub fn merge_from(&mut self, newer: &Sample) {
        for (k, v) in newer.iter() {
            self.values.insert(k, *v);
        }
        self.stale = newer.stale.clone();
    }
}
