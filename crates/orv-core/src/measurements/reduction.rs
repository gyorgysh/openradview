// SPDX-License-Identifier: GPL-3.0-or-later
//! Identical stable reductions for each scalar or RGB channel.
pub(super) struct Moments {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    m2: f64,
}
impl Default for Moments {
    fn default() -> Self {
        Self {
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            mean: 0.,
            m2: 0.,
        }
    }
}
impl Moments {
    pub fn add(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }
    pub fn standard_deviation(&self) -> f64 {
        (self.m2 / self.count as f64).max(0.).sqrt()
    }
}
