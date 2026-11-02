//! Real-world timings for the debug center (`kind: "perf"` reports): per
//! endpoint request → response ms, navigation → content-visible ms, parse +
//! render ms, cache hit rates. Numbers and endpoint names only: no URLs,
//! tokens, ids or cookies. Sent at most every 10 minutes while new samples
//! exist, and when the tab is hidden.

use serde_json::{Map, Value, json};

/// Samples kept per series (the newest).
const KEEP: usize = 200;
pub const EVERY_MS: f64 = 10.0 * 60_000.0;
/// On tab hide, not more often than this.
pub const HIDE_MIN_MS: f64 = 60_000.0;

#[derive(Default)]
pub struct Perf {
    series: Vec<(&'static str, Vec<f32>)>,
    hits: Vec<(&'static str, u32, u32)>,
    errors: Vec<(&'static str, u32)>,
    /// Samples since the last report.
    pub fresh: u32,
    pub last_sent: f64,
}

impl Perf {
    fn push(&mut self, name: &'static str, ms: f64) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        let s = match self.series.iter().position(|s| s.0 == name) {
            Some(i) => &mut self.series[i].1,
            None => {
                self.series.push((name, Vec::new()));
                &mut self.series.last_mut().expect("pushed").1
            }
        };
        if s.len() >= KEEP {
            s.remove(0);
        }
        s.push(ms as f32);
        self.fresh += 1;
    }

    /// An InnerTube (or other) request answered after `ms`.
    pub fn req(&mut self, ep: &'static str, ms: f64, ok: bool) {
        self.push(ep, ms);
        if !ok {
            match self.errors.iter_mut().find(|e| e.0 == ep) {
                Some(e) => e.1 += 1,
                None => self.errors.push((ep, 1)),
            }
        }
    }

    /// Navigation → something visible (`what`, e.g. "nav:home-cards").
    pub fn nav(&mut self, what: &'static str, ms: f64) {
        self.push(what, ms);
    }

    pub fn hit(&mut self, cache: &'static str, hit: bool) {
        match self.hits.iter_mut().find(|h| h.0 == cache) {
            Some(h) => {
                if hit { h.1 += 1 } else { h.2 += 1 }
            }
            None => self.hits.push((cache, hit as u32, !hit as u32)),
        }
    }

    pub fn due(&self, now: f64) -> bool {
        self.fresh > 0 && now - self.last_sent >= EVERY_MS
    }

    /// The report body; marks it sent.
    pub fn take(&mut self, now: f64) -> Value {
        let mut series = Map::new();
        for (name, v) in &self.series {
            series.insert((*name).into(), stats(v));
        }
        let hits: Map<String, Value> = self.hits.iter().map(|(c, h, m)| ((*c).to_string(), json!({"hit": h, "miss": m, "rate": if h + m > 0 { (*h as f64 / (h + m) as f64 * 100.0).round() / 100.0 } else { 0.0 }}))).collect();
        let errors: Map<String, Value> = self.errors.iter().map(|(e, n)| ((*e).to_string(), json!(n))).collect();
        self.fresh = 0;
        self.last_sent = now;
        json!({"ms": series, "cache": hits, "errors": errors})
    }
}

/// {n, p50, p90, max} in whole ms.
pub fn stats(v: &[f32]) -> Value {
    if v.is_empty() {
        return json!({"n": 0});
    }
    let mut s = v.to_vec();
    s.sort_unstable_by(f32::total_cmp);
    let q = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize].round();
    json!({"n": s.len(), "p50": q(0.5), "p90": q(0.9), "max": s[s.len() - 1].round()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report() {
        let mut p = Perf::default();
        assert!(!p.due(1e9));
        for i in 1..=10 {
            p.req("browse", i as f64 * 100.0, i != 3);
        }
        p.nav("nav:watch-related", 12.0);
        p.hit("watch-next", true);
        p.hit("watch-next", false);
        p.hit("watch-next", true);
        p.req("next", f64::NAN, true);
        assert!(p.due(EVERY_MS));
        let r = p.take(EVERY_MS);
        assert_eq!(r["ms"]["browse"], json!({"n": 10, "p50": 600.0, "p90": 900.0, "max": 1000.0}));
        assert_eq!(r["ms"]["nav:watch-related"]["n"], 1);
        assert!(r["ms"].get("next").is_none(), "NaN ignored");
        assert_eq!(r["cache"]["watch-next"]["rate"], 0.67);
        assert_eq!(r["errors"]["browse"], 1);
        assert!(!p.due(EVERY_MS * 1.5), "nothing new");
        let s = r.to_string();
        assert!(!s.contains("http") && !s.contains("token"));
    }
}
