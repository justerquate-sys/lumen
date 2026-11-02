//! One-time import of the old YourTube extension's data (2 Nov 2026).
//!
//! The old extension keeps its records in its own `chrome.storage.local`,
//! which the client cannot read. The user copies it from the old
//! extension's service-worker console (`EXPORT_SNIPPET`) and pastes the JSON
//! here. Everything is validated as strictly as the client's own stores:
//! a wrong shape for any known key rejects the whole paste (nothing is
//! written); single bad records inside a valid store are skipped and
//! counted; the client's bounds apply (100 searches, 2,000 resume records,
//! 50 blocked websites, 20 topics, 20 YouTube-topic records).
//!
//! Merging: searches and resume positions merge newest-wins; lists (blocked
//! websites, topics, YouTube-topic order/names) are unions, the client's own
//! entries first; settings (Home filters, captions) are taken from the old
//! extension. Caption placement is converted (`yourTubeCaptionsV1` →
//! `ytcConfigV1.p.cs`, `yourTubeCaptionsEnabledV1` → `ytcConfigV1.p.cc`).
//! Statistics and the two legacy topic stores are left out. Nothing outside
//! the authorized records is kept (no descriptions, payloads, tokens or
//! verdicts are in this data).

use super::filters::{self, Prefs as FilterPrefs};
use super::history::{self, History};
use super::home_chips as hc;
use super::progress::{self, Store};
use super::topics;
use super::websites::{self, Rules};
use crate::config::CcStyle;
use crate::json::{self, Value as In};

/// What the user runs in the old extension's service-worker console.
pub const EXPORT_SNIPPET: &str = "chrome.storage.local.get(null, d => copy(JSON.stringify(d)))";
/// A paste larger than this is not the old extension's storage (2,000 resume records are ~200 KB).
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const KEY_CAPTIONS: &str = "yourTubeCaptionsV1";
const KEY_CAPTIONS_ON: &str = "yourTubeCaptionsEnabledV1";
/// Old keys deliberately not imported.
const DROPPED: [&str; 3] = ["yourTubeStatsV1", "yourTubeDiscoveryTopicsV1", "yourTubeHomeTopicsV1"];

/// The client's current records (the merge base).
pub struct Current<'a> {
    pub hist: &'a History,
    pub progress: &'a Store,
    pub rules: &'a Rules,
    pub topics: &'a [String],
    pub native: &'a hc::Prefs,
    pub filters: &'a FilterPrefs,
}

/// The records to write (None = untouched) and the summary for the user.
#[derive(Debug, Default)]
pub struct Plan {
    pub hist: Option<History>,
    pub progress: Option<Store>,
    pub rules: Option<Rules>,
    pub topics: Option<Vec<String>>,
    pub native: Option<hc::Prefs>,
    /// Home filter settings, with the keys that were imported.
    pub filters: Option<(FilterPrefs, Vec<&'static str>)>,
    pub cc_style: Option<CcStyle>,
    pub captions: Option<bool>,
    pub summary: String,
}

fn fail(msg: &str) -> String {
    msg.to_string()
}

/// Thousands separators: 1204 → "1,204".
pub fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", grouped(n), if n == 1 { one } else { many })
}

/// `{version: 1, <field>: <array or object>}`: the shape of every old store.
fn store<'v, 'a>(v: &'v In<'a>, key: &str, field: &str, object: bool) -> Result<&'v In<'a>, String> {
    let ok = v.is_object() && v.get("version").and_then(In::as_u64) == Some(1);
    let inner = v.get(field).filter(|x| if object { x.is_object() } else { x.is_array() });
    match inner {
        Some(x) if ok => Ok(x),
        _ => Err(format!("“{key}” isn't in the old extension's format; nothing was imported.")),
    }
}

/// Caption placement `{version: 1|2, position: {x, y} (0–1), scale?: 0.5–2}` → `p.cs`.
fn captions(v: &In) -> Option<CcStyle> {
    let version = v.get("version").and_then(In::as_u64)?;
    if !(1..=2).contains(&version) {
        return None;
    }
    let p = v.get("position")?;
    let unit = |x: Option<f64>| x.filter(|x| x.is_finite() && (0.0..=1.0).contains(x));
    let x = unit(p.get("x").and_then(In::as_f64))?;
    let y = unit(p.get("y").and_then(In::as_f64))?;
    let s = match v.get("scale") {
        None => 1.0,
        Some(s) => s.as_f64().filter(|s| s.is_finite() && (0.5..=2.0).contains(s))?,
    };
    Some(CcStyle { x, y, s }.clamped())
}

/// Parses, validates and merges a paste of the old extension's storage.
pub fn plan(text: &str, cur: &Current, now: f64) -> Result<Plan, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(fail("Paste the old extension's data first."));
    }
    if text.len() > MAX_BYTES {
        return Err(fail("That's more than the old extension ever stores (over 4 MB); nothing was imported."));
    }
    let Some(v) = json::parse_str(text) else {
        return Err(fail("That isn't the JSON the console line copies; nothing was imported."));
    };
    let Some(o) = v.as_object() else {
        return Err(fail("That isn't the old extension's storage (expected one JSON object); nothing was imported."));
    };
    let known = |k: &str| {
        filters::KEYS.contains(&k)
            || [history::KEY, progress::KEY, websites::KEY, topics::KEY, hc::KEY, KEY_CAPTIONS, KEY_CAPTIONS_ON].contains(&k)
    };
    if !o.keys().any(known) {
        return Err(fail("No YourTube data found in that text; nothing was imported."));
    }
    let mut p = Plan::default();
    let mut parts: Vec<String> = Vec::new();
    let mut skipped = 0usize;

    // ---- searches: newest wins, 100 newest kept
    if let Some(x) = o.get(history::KEY) {
        let raw = store(x, history::KEY, "entries", false)?.as_array().map_or(0, Vec::len);
        if raw > history::MAX * 10 {
            return Err(fail("The search history has far more entries than the old extension keeps; nothing was imported."));
        }
        let old = History::from_json(x, now);
        skipped += raw - old.entries.len().min(raw);
        let mut merged = cur.hist.clone();
        for (q, at) in old.entries.iter().rev() {
            let f = q.to_lowercase();
            match merged.entries.iter().position(|(e, _)| e.to_lowercase() == f) {
                Some(i) if merged.entries[i].1 >= *at => {}
                Some(i) => {
                    merged.entries.remove(i);
                    merged.entries.push((q.clone(), *at));
                }
                None => merged.entries.push((q.clone(), *at)),
            }
        }
        filters::sort_keyed(&mut merged.entries, |e| -e.1);
        merged.entries.truncate(history::MAX);
        if old.started_at > 0.0 && (merged.started_at == 0.0 || old.started_at < merged.started_at) {
            merged.started_at = old.started_at;
        }
        let n = old.entries.iter().filter(|(q, at)| merged.entries.iter().any(|(e, t)| e == q && t == at)).count();
        parts.push(count(n, "search", "searches"));
        p.hist = Some(merged);
    }

    // ---- resume positions: newest `updatedAt` wins, 2,000 newest kept
    if let Some(x) = o.get(progress::KEY) {
        let raw = store(x, progress::KEY, "entries", true)?.as_object().map_or(0, |m| m.len());
        if raw > progress::MAX * 10 {
            return Err(fail("The resume positions are far more than the old extension keeps; nothing was imported."));
        }
        let old = Store::from_json(x, now);
        skipped += raw.saturating_sub(old.map.len());
        let mut merged = Store { map: cur.progress.map.clone() };
        for (id, e) in &old.map {
            if merged.map.get(id).is_none_or(|c| c.updated_at < e.updated_at) {
                merged.map.insert(id.clone(), *e);
            }
        }
        merged.cap();
        let n = old.map.iter().filter(|(id, e)| merged.map.get(*id) == Some(e)).count();
        parts.push(count(n, "resume position", "resume positions"));
        p.progress = Some(merged);
    }

    // ---- blocked websites: union, the client's first, ≤ 50
    if let Some(x) = o.get(websites::KEY) {
        let raw = store(x, websites::KEY, "domains", false)?.as_array().map_or(0, Vec::len);
        let mut merged = cur.rules.clone();
        let mut n = 0;
        for d in x.get("domains").and_then(In::as_array).into_iter().flatten() {
            match d.as_str().and_then(websites::normalize) {
                Some(d) if merged.domains.contains(&d) => n += 1,
                Some(d) if merged.domains.len() < websites::MAX => {
                    merged.domains.push(d);
                    n += 1;
                }
                _ => {}
            }
        }
        skipped += raw.saturating_sub(n);
        parts.push(count(n, "blocked website", "blocked websites"));
        p.rules = Some(merged);
    }

    // ---- custom topics: union, the client's first, ≤ 20
    if let Some(x) = o.get(topics::KEY) {
        let list = store(x, topics::KEY, "topics", false)?.as_array().map_or(&[][..], |a| a.as_slice());
        let mut merged = cur.topics.to_vec();
        let mut n = 0;
        for t in list {
            match t.as_str().and_then(topics::clean_topic) {
                Some(t) if merged.iter().any(|o| o.to_lowercase() == t.to_lowercase()) => n += 1,
                Some(t) if merged.len() < topics::MAX_TOPICS => {
                    merged.push(t);
                    n += 1;
                }
                _ => {}
            }
        }
        skipped += list.len().saturating_sub(n);
        parts.push(count(n, "topic", "topics"));
        p.topics = Some(merged);
    }

    // ---- YouTube's own Home topics (order, names): union, the client's records first, ≤ 20
    if let Some(x) = o.get(hc::KEY) {
        let raw = store(x, hc::KEY, "topics", false)?.as_array().map_or(0, Vec::len);
        let old = hc::Prefs::from_json(x);
        let mut merged = cur.native.clone();
        let mut n = 0;
        for r in old.list {
            if merged.list.iter().any(|c| c.key == r.key) {
                n += 1;
            } else if merged.list.len() < hc::MAX {
                merged.list.push(r);
                n += 1;
            }
        }
        skipped += raw.saturating_sub(n);
        parts.push(count(n, "YouTube topic setting", "YouTube topic settings"));
        p.native = Some(merged);
    }

    // ---- Home filter settings: taken from the old extension
    let mut fp = cur.filters.clone();
    let mut fkeys = Vec::new();
    for k in filters::KEYS {
        let Some(x) = o.get(k) else { continue };
        let ok = match k {
            filters::KEY_DURATION => x.as_f64().is_some_and(|m| m.fract() == 0.0 && (0.0..=filters::MAX_MINUTES as f64).contains(&m)),
            filters::KEY_ORDER => matches!(x.as_str(), Some("popular" | "recommended")),
            _ => x.as_bool().is_some(),
        };
        if !ok {
            return Err(format!("“{k}” isn't in the old extension's format; nothing was imported."));
        }
        fp.apply(k, x);
        fkeys.push(k);
    }
    if !fkeys.is_empty() {
        parts.push("Home filter settings".into());
        p.filters = Some((fp, fkeys));
    }

    // ---- captions: placement/size and on/off, converted into the player's preferences
    if let Some(x) = o.get(KEY_CAPTIONS) {
        p.cc_style = Some(captions(x).ok_or_else(|| format!("“{KEY_CAPTIONS}” isn't in the old extension's format; nothing was imported."))?);
    }
    if let Some(x) = o.get(KEY_CAPTIONS_ON) {
        p.captions = Some(x.as_bool().ok_or_else(|| format!("“{KEY_CAPTIONS_ON}” isn't in the old extension's format; nothing was imported."))?);
    }
    match (p.cc_style.is_some(), p.captions.is_some()) {
        (true, true) => parts.push("caption size, position and on/off".into()),
        (true, false) => parts.push("caption size and position".into()),
        (false, true) => parts.push("captions on/off".into()),
        _ => {}
    }

    let mut s = String::from("Imported ");
    match parts.len() {
        1 => s.push_str(&parts[0]),
        n => {
            s.push_str(&parts[..n - 1].join(", "));
            s.push_str(" and ");
            s.push_str(&parts[n - 1]);
        }
    }
    s.push('.');
    if skipped > 0 {
        s.push_str(&format!(" {} skipped (invalid or over the limits).", count(skipped, "record", "records")));
    }
    if DROPPED.iter().any(|k| o.contains_key(k)) {
        s.push_str(" Viewing statistics and the old topic lists are not carried over.");
    }
    p.summary = s;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: f64 = 1_793_000_000_000.0;

    fn base() -> (History, Store, Rules, Vec<String>, hc::Prefs, FilterPrefs) {
        (History::default(), Store::default(), Rules::default(), Vec::new(), hc::Prefs::default(), FilterPrefs::default())
    }

    fn run(text: &str, b: &(History, Store, Rules, Vec<String>, hc::Prefs, FilterPrefs)) -> Result<Plan, String> {
        plan(text, &Current { hist: &b.0, progress: &b.1, rules: &b.2, topics: &b.3, native: &b.4, filters: &b.5 }, NOW)
    }

    /// The old extension's storage as `chrome.storage.local.get(null)` returns it (synthetic data).
    fn old() -> serde_json::Value {
        let entries: Vec<_> = (0..87).map(|i| json!({"query": format!("search number {i}"), "at": NOW - 1000.0 * i as f64})).collect();
        let mut progress = serde_json::Map::new();
        for i in 0..1204 {
            progress.insert(format!("vid{i:08}"), json!({"position": 30.5, "duration": 600, "completed": i % 7 == 0, "updatedAt": NOW - 5000.0 * i as f64}));
        }
        json!({
            "yourTubeHistoryV1": {"version": 1, "startedAt": NOW - 9e9, "entries": entries},
            "yourTubeProgressV1": {"version": 1, "entries": progress},
            "yourTubeBlockedWebsitesV1": {"version": 1, "domains": ["www.Example.com", "spam.example.org", "not a domain!"]},
            "yourTubeRecommendationTopicsV1": {"version": 1, "topics": ["Roblox scripting", "  Woodworking   joints "]},
            "yourTubeNativeTopicsV1": {"version": 1, "topics": [{"key": "Gaming", "name": "Games"}, {"key": "podcasts", "name": "Podcasts"}]},
            "yourTubeMinimumHomeDurationV1": 20,
            "yourTubeHomeOrderV1": "popular",
            "yourTubeHomePlaylistsOnlyV1": false,
            "yourTubeShowMusicV1": true,
            "yourTubeShowLiveV1": false,
            "shortsInFeeds": false,
            "yourTubeCaptionsV1": {"version": 2, "position": {"x": 0.1, "y": 0.8}, "scale": 1.25},
            "yourTubeCaptionsEnabledV1": false,
            "yourTubeStatsV1": {"version": 1, "days": {"2026-11-01": 3600}},
            "yourTubeDiscoveryTopicsV1": {"version": 1, "topics": ["x"]},
            "yourTubeHomeTopicsV1": {"version": 1, "topics": []}
        })
    }

    #[test]
    fn full_import_into_an_empty_client() {
        let b = base();
        let p = run(&old().to_string(), &b).unwrap();
        assert_eq!(
            p.summary,
            "Imported 87 searches, 1,204 resume positions, 2 blocked websites, 2 topics, 2 YouTube topic settings, Home filter settings and caption size, position and on/off. 1 record skipped (invalid or over the limits). Viewing statistics and the old topic lists are not carried over."
        );
        let h = p.hist.unwrap();
        assert_eq!(h.entries.len(), 87);
        assert_eq!(h.entries[0].0, "search number 0", "newest first");
        assert_eq!(p.progress.as_ref().unwrap().map.len(), 1204);
        assert_eq!(p.rules.unwrap().domains, ["example.com", "spam.example.org"]);
        assert_eq!(p.topics.unwrap(), ["Roblox scripting", "Woodworking joints"]);
        let n = p.native.unwrap();
        assert_eq!((n.list[0].key.as_str(), n.list[0].name.as_str()), ("gaming", "Games"), "renamed chip kept");
        let (f, keys) = p.filters.unwrap();
        assert_eq!((f.min_minutes, f.popular, f.show_music, f.show_live, f.playlists_only, f.show_shorts), (20, true, true, false, false, false));
        assert_eq!(keys.len(), 6);
        assert_eq!(p.cc_style, Some(CcStyle { x: 0.1, y: 0.8, s: 1.25 }));
        assert_eq!(p.captions, Some(false));
    }

    #[test]
    fn merges_newest_wins_and_unions_within_limits() {
        let mut b = base();
        // The client already has: a newer and an older copy of two searches, records, rules, topics.
        b.0.entries = vec![("Search Number 1".into(), NOW), ("search number 5".into(), NOW - 1e9), ("client only".into(), NOW - 10.0)];
        b.1.map.insert("vid00000000".into(), progress::Entry { position: 99.0, duration: 600.0, completed: false, updated_at: NOW + 1.0 });
        b.1.map.insert("vid00000001".into(), progress::Entry { position: 1.0, duration: 600.0, completed: false, updated_at: 1.0 });
        b.2.domains = (0..49).map(|i| format!("site{i}.example")).collect();
        b.3 = (0..19).map(|i| format!("Client topic {i}")).collect();
        b.3.push("roblox SCRIPTING".into());
        b.4.list.push(hc::Pref { key: "gaming".into(), name: "My gaming".into(), hide: true });
        let p = run(&old().to_string(), &b).unwrap();
        let h = p.hist.unwrap();
        assert_eq!(h.entries.len(), 88, "87 imported + 1 client-only, duplicates once");
        assert_eq!(h.entries[0], ("Search Number 1".to_string(), NOW), "the client's newer copy wins");
        assert!(h.entries.iter().any(|(q, at)| q == "search number 5" && *at == NOW - 5000.0), "the old extension's newer copy wins");
        let pr = p.progress.unwrap();
        assert_eq!(pr.map["vid00000000"].position, 99.0, "newer client record kept");
        assert_eq!(pr.map["vid00000001"].position, 30.5, "newer imported record wins");
        assert_eq!(pr.map.len(), 1204);
        let r = p.rules.unwrap();
        assert_eq!(r.domains.len(), 50, "≤ 50");
        assert_eq!(r.domains[49], "example.com");
        let t = p.topics.unwrap();
        assert_eq!(t.len(), 20, "≤ 20, the existing topic matched case-insensitively");
        let n = p.native.unwrap();
        assert_eq!(n.list[0], hc::Pref { key: "gaming".into(), name: "My gaming".into(), hide: true }, "the client's record wins");
        assert_eq!(n.list.len(), 2);
        assert!(p.summary.contains("1 blocked website") && p.summary.contains("3 records skipped"), "{}", p.summary);
    }

    #[test]
    fn caps_at_the_clients_bounds() {
        let b = base();
        let entries: Vec<_> = (0..150).map(|i| json!({"query": format!("q{i}"), "at": NOW - i as f64})).collect();
        let mut progress = serde_json::Map::new();
        for i in 0..2100 {
            progress.insert(format!("v{i}"), json!({"position": 1, "duration": 10, "updatedAt": NOW - i as f64}));
        }
        let j = json!({"yourTubeHistoryV1": {"version": 1, "entries": entries}, "yourTubeProgressV1": {"version": 1, "entries": progress}});
        let p = run(&j.to_string(), &b).unwrap();
        let h = p.hist.unwrap();
        assert_eq!((h.entries.len(), h.entries[99].0.as_str()), (100, "q99"), "100 newest");
        let pr = p.progress.unwrap();
        assert_eq!(pr.map.len(), 2000);
        assert!(pr.map.contains_key("v0") && !pr.map.contains_key("v2099"), "2,000 newest");
        assert_eq!(p.summary, "Imported 100 searches and 2,000 resume positions. 150 records skipped (invalid or over the limits).");
    }

    #[test]
    fn rejects_anything_else_whole() {
        let b = base();
        for (text, why) in [
            ("", "Paste"),
            ("not json", "isn't the JSON"),
            ("[1,2]", "one JSON object"),
            ("{\"ytcConfigV1\": {}}", "No YourTube data"),
            ("{\"yourTubeHistoryV1\": [\"q\"]}", "yourTubeHistoryV1"),
            ("{\"yourTubeHistoryV1\": {\"version\": 2, \"entries\": []}}", "yourTubeHistoryV1"),
            ("{\"yourTubeProgressV1\": {\"version\": 1, \"entries\": []}}", "yourTubeProgressV1"),
            ("{\"yourTubeBlockedWebsitesV1\": {\"version\": 1, \"domains\": \"a.com\"}}", "yourTubeBlockedWebsitesV1"),
            ("{\"yourTubeMinimumHomeDurationV1\": 999}", "yourTubeMinimumHomeDurationV1"),
            ("{\"yourTubeHomeOrderV1\": \"newest\"}", "yourTubeHomeOrderV1"),
            ("{\"shortsInFeeds\": \"yes\"}", "shortsInFeeds"),
            ("{\"yourTubeCaptionsV1\": {\"version\": 2, \"position\": {\"x\": 3, \"y\": 0}}}", "yourTubeCaptionsV1"),
            ("{\"yourTubeCaptionsV1\": {\"version\": 1, \"position\": {\"x\": 0.1, \"y\": 0.1}, \"scale\": 9}}", "yourTubeCaptionsV1"),
            ("{\"yourTubeCaptionsEnabledV1\": 1}", "yourTubeCaptionsEnabledV1"),
            // One bad store spoils the whole paste, even next to good ones.
            ("{\"yourTubeHistoryV1\": {\"version\": 1, \"entries\": []}, \"yourTubeNativeTopicsV1\": {\"version\": 1}}", "yourTubeNativeTopicsV1"),
        ] {
            let e = run(text, &b).unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
        let huge = format!("{{\"yourTubeHistoryV1\": {{\"version\": 1, \"entries\": [{}]}}}}", vec!["{\"query\":\"q\",\"at\":1}"; 1001].join(","));
        assert!(run(&huge, &b).unwrap_err().contains("far more"));
        assert!(run(&" ".repeat(MAX_BYTES + 10).replace(' ', "x"), &b).unwrap_err().contains("4 MB"));
    }

    #[test]
    fn version_one_captions_and_partial_data() {
        let b = base();
        let p = run("{\"yourTubeCaptionsV1\": {\"version\": 1, \"position\": {\"x\": 0.99, \"y\": 0.5}}, \"yourTubeStatsV1\": {}}", &b).unwrap();
        assert_eq!(p.cc_style, Some(CcStyle { x: 0.95, y: 0.5, s: 1.0 }), "default scale, clamped to the client's range");
        assert!(p.hist.is_none() && p.progress.is_none() && p.filters.is_none() && p.captions.is_none(), "absent keys untouched");
        assert_eq!(p.summary, "Imported caption size and position. Viewing statistics and the old topic lists are not carried over.");
        assert_eq!(grouped(1204), "1,204");
        assert_eq!(grouped(1_000_000), "1,000,000");
        assert_eq!(grouped(87), "87");
    }
}
