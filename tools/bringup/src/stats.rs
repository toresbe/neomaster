//! Percentiles over timing samples.

use serde_json::{json, Value};

#[derive(Clone, Debug, Default)]
pub struct Samples {
    us: Vec<f64>,
}

impl Samples {
    pub fn new() -> Samples {
        Samples::default()
    }

    pub fn push(&mut self, us: f64) {
        self.us.push(us);
    }

    pub fn len(&self) -> usize {
        self.us.len()
    }

    pub fn is_empty(&self) -> bool {
        self.us.is_empty()
    }

    fn sorted(&self) -> Vec<f64> {
        let mut v = self.us.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        v
    }

    /// The `p`th percentile (0-100), nearest rank.
    pub fn pct(&self, p: f64) -> f64 {
        let v = self.sorted();
        if v.is_empty() {
            return f64::NAN;
        }
        let i = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
        v[i.min(v.len() - 1)]
    }

    pub fn mean(&self) -> f64 {
        if self.us.is_empty() {
            return f64::NAN;
        }
        self.us.iter().sum::<f64>() / self.us.len() as f64
    }

    pub fn max(&self) -> f64 {
        self.us.iter().copied().fold(f64::NAN, f64::max)
    }

    /// How many samples are above `limit`.
    pub fn over(&self, limit: f64) -> usize {
        self.us.iter().filter(|&&x| x > limit).count()
    }

    pub fn json(&self) -> Value {
        let r = |x: f64| if x.is_finite() { json!((x * 10.0).round() / 10.0) } else { Value::Null };
        json!({
            "n": self.len(),
            "mean_us": r(self.mean()),
            "p50_us": r(self.pct(50.0)),
            "p95_us": r(self.pct(95.0)),
            "p99_us": r(self.pct(99.0)),
            "max_us": r(self.max()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let mut s = Samples::new();
        for i in 1..=101 {
            s.push(i as f64);
        }
        assert_eq!(s.pct(50.0), 51.0);
        assert_eq!(s.pct(0.0), 1.0);
        assert_eq!(s.pct(100.0), 101.0);
        assert_eq!(s.over(100.0), 1);
        assert_eq!(s.mean(), 51.0);
    }
}
