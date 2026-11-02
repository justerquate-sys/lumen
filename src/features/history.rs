//! Search history: the last 100 submitted searches (
//! `yourTubeHistoryV1`, same record shape as the extension:
//! `{version:1, startedAt, entries:[{query, at}]}` newest first, queries
//! deduplicated case-insensitively, ≤ 256 characters). Shown in the search
//! box's dropdown together with YouTube's suggestions.

use crate::util::esc_into;
use crate::json::Value as In;
use serde_json::{Value, json};

pub const KEY: &str = "yourTubeHistoryV1";
pub const MAX: usize = 100;
const QUERY_MAX: usize = 256;
/// History rows shown in the dropdown (all matches are kept in storage).
pub const SHOWN: usize = 8;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub started_at: f64,
    /// Newest first.
    pub entries: Vec<(String, f64)>,
}

/// Whitespace collapsed and trimmed, control characters dropped, ≤ 256 chars.
pub fn clean(q: &str) -> String {
    let mut out = String::new();
    for w in q.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out.chars().take(QUERY_MAX).collect()
}

fn fold(q: &str) -> String {
    q.to_lowercase()
}

impl History {
    pub fn from_json(v: &In, now: f64) -> History {
        let mut entries: Vec<(String, f64)> = v
            .get("entries")
            .and_then(In::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        let q = clean(e.get("query")?.as_str()?);
                        let at = e.get("at")?.as_f64()?;
                        (!q.is_empty() && at.is_finite() && at > 0.0 && at <= now + 60_000.0).then_some((q, at))
                    })
                    .collect()
            })
            .unwrap_or_default();
        super::filters::sort_keyed(&mut entries, |e| -e.1);
        let mut seen = std::collections::HashSet::new();
        entries.retain(|(q, _)| seen.insert(fold(q)));
        entries.truncate(MAX);
        let started_at = v.get("startedAt").and_then(In::as_f64).filter(|t| t.is_finite() && *t > 0.0 && *t <= now).unwrap_or(now);
        History { started_at, entries }
    }

    pub fn to_json(&self) -> String {
        let entries: Vec<Value> = self.entries.iter().map(|(q, at)| json!({"query": q, "at": at})).collect();
        json!({"version": 1, "startedAt": self.started_at, "entries": entries}).to_string()
    }

    /// Records a submitted search (moves an existing one to the top).
    pub fn record(&mut self, q: &str, now: f64) -> bool {
        let q = clean(q);
        if q.is_empty() {
            return false;
        }
        if self.started_at == 0.0 {
            self.started_at = now;
        }
        let f = fold(&q);
        self.entries.retain(|(e, _)| fold(e) != f);
        self.entries.insert(0, (q, now));
        self.entries.truncate(MAX);
        true
    }

    pub fn remove(&mut self, q: &str) -> bool {
        let f = fold(&clean(q));
        let n = self.entries.len();
        self.entries.retain(|(e, _)| fold(e) != f);
        n != self.entries.len()
    }

    /// Entries starting with (then containing) the typed text; all recent ones when empty.
    pub fn matches(&self, typed: &str, max: usize) -> Vec<String> {
        let t = fold(&clean(typed));
        if t.is_empty() {
            return self.entries.iter().take(max).map(|e| e.0.clone()).collect();
        }
        let mut out: Vec<String> = self.entries.iter().filter(|(e, _)| fold(e).starts_with(&t) && fold(e) != t).map(|e| e.0.clone()).collect();
        for (e, _) in &self.entries {
            if out.len() >= max {
                break;
            }
            let fe = fold(e);
            if !fe.starts_with(&t) && fe.contains(&t) {
                out.push(e.clone());
            }
        }
        out.truncate(max);
        out
    }
}

const CLOCK: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M3 10a9 9 0 1 1 2.5 8.3M3 4v6h6\"/><path d=\"M12 7v5l3 2\"/></svg>";
const GLASS: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><circle cx=\"10.5\" cy=\"10.5\" r=\"6.5\"/><path d=\"M15.5 15.5 21 21\"/></svg>";

/// The dropdown rows: history first (with remove buttons), then suggestions
/// not already listed. `sel` is the keyboard-selected row (or none).
pub fn dropdown(hist: &[String], sugg: &[String], sel: Option<usize>) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    let mut row = |out: &mut String, q: &str, history: bool| {
        out.push_str("<li role=\"option\" class=\"sg");
        if history {
            out.push_str(" hs");
        }
        if sel == Some(i) {
            out.push_str(" on\" aria-selected=\"true");
        }
        out.push_str("\" id=\"ytc-sg");
        out.push_str(&i.to_string());
        out.push_str("\"><a href=\"/results?search_query=");
        esc_into(out, &crate::util::pct_encode(q));
        out.push_str("\" data-act=\"fx-q\" data-arg=\"");
        esc_into(out, q);
        out.push_str("\" tabindex=\"-1\">");
        out.push_str(if history { CLOCK } else { GLASS });
        out.push_str("<span>");
        esc_into(out, q);
        out.push_str("</span></a>");
        if history {
            out.push_str("<button type=\"button\" class=\"sgx\" data-act=\"fx-hdel\" data-arg=\"");
            esc_into(out, q);
            out.push_str("\" aria-label=\"Remove ");
            esc_into(out, q);
            out.push_str(" from search history\" tabindex=\"-1\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"m6 6 12 12M18 6 6 18\"/></svg></button>");
        }
        out.push_str("</li>");
        i += 1;
    };
    for q in hist {
        row(&mut out, q, true);
    }
    let folded: Vec<String> = hist.iter().map(|h| fold(h)).collect();
    for q in sugg {
        if !folded.contains(&fold(q)) {
            row(&mut out, q, false);
        }
    }
    out
}

/// The rows' queries in display order (for keyboard selection).
pub fn rows(hist: &[String], sugg: &[String]) -> Vec<String> {
    let folded: Vec<String> = hist.iter().map(|h| fold(h)).collect();
    hist.iter().cloned().chain(sugg.iter().filter(|q| !folded.contains(&fold(q))).cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_dedupe_cap_remove() {
        let mut h = History::default();
        for i in 0..120 {
            h.record(&format!("query {i}"), 1000.0 + i as f64);
        }
        assert_eq!(h.entries.len(), MAX);
        assert_eq!(h.entries[0].0, "query 119");
        assert!(h.record("  Query   119 ", 5000.0));
        assert_eq!(h.entries.len(), MAX, "case-insensitive duplicate replaced");
        assert_eq!(h.entries[0].0, "Query 119");
        assert!(!h.record("   ", 1.0));
        assert!(h.remove("QUERY 119"));
        assert!(!h.remove("nope"));
        assert_eq!(h.entries.len(), MAX - 1);
        let long = "x".repeat(400);
        h.record(&long, 6000.0);
        assert_eq!(h.entries[0].0.chars().count(), 256);
    }

    #[test]
    fn json_round_trip_and_validation() {
        let mut h = History::default();
        h.record("roblox", 10.0);
        h.record("lua", 20.0);
        let v: Value = serde_json::from_str(&h.to_json()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(History::from_json(&In::from_serde(&v), 100.0), h);
        let bad = crate::json::jv!({"entries": [{"query": "a", "at": 5}, {"query": "A", "at": 4}, {"query": 3, "at": 2}, {"query": "future", "at": 1e15}, {"query": "b", "at": -1}]});
        let h = History::from_json(&bad, 100.0);
        assert_eq!(h.entries, vec![("a".to_string(), 5.0)]);
        assert_eq!(History::from_json(&In::Null, 100.0).entries.len(), 0);
    }

    #[test]
    fn matching_and_dropdown() {
        let mut h = History::default();
        for (i, q) in ["roblox scripting", "lua tables", "Roblox animation", "advanced roblox"].iter().enumerate() {
            h.record(q, i as f64 + 1.0);
        }
        assert_eq!(h.matches("", 2), vec!["advanced roblox", "Roblox animation"]);
        assert_eq!(h.matches("rob", 8), vec!["Roblox animation", "roblox scripting", "advanced roblox"]);
        let html = dropdown(&h.matches("rob", 8), &["roblox <b>".to_string(), "roblox scripting".to_string()], Some(1));
        assert!(!html.contains("<b>"), "escaped");
        assert_eq!(html.matches("data-act=\"fx-hdel\"").count(), 3);
        assert_eq!(html.matches("class=\"sg\"").count() + html.matches("class=\"sg on").count(), 1, "duplicate suggestion dropped");
        assert!(html.contains("id=\"ytc-sg1\""));
        assert_eq!(rows(&h.matches("rob", 8), &["roblox scripting".into(), "x".into()]).len(), 4);
    }
}
