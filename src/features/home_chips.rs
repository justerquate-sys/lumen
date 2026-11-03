//! YouTube's own Home topic chips ("Gaming", "Podcasts", …: the feed filter
//! chip bar of the Home response), shown beside the user's custom topics.
//! A chip switches the feed through its own continuation token (exactly
//! YouTube's request). The user can reorder and hide them; the preference
//! is the `yourTubeNativeTopicsV1` record (at
//! most 20 `{key, name}` records, keys ≤ 120 and names ≤ 40 characters),
//! here with an optional `hide` flag. Chips themselves and their tokens are
//! never stored (beyond this tab's sessionStorage copy of the ready Home
//! response, `crate::ready`).

use crate::parse::text;
use crate::json::Value as In;
use serde_json::{Value, json};

pub const KEY: &str = "yourTubeNativeTopicsV1";
pub const MAX: usize = 20;
const KEY_LEN: usize = 120;
const NAME_LEN: usize = 40;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chip {
    pub label: String,
    pub key: String,
    /// Continuation token that loads this chip's feed.
    pub token: String,
    pub selected: bool,
}

/// Normalized preference key of a label (trimmed, single spaces, lower case).
pub fn key_of(label: &str) -> String {
    let k = label.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    k.chars().take(KEY_LEN).collect()
}

/// "All" in the languages the old extension knew (the client's own All replaces it).
fn all_like(key: &str) -> bool {
    matches!(key, "all" | "alle" | "alles" | "все" | "всё" | "tout" | "todos" | "todas" | "tutti" | "tudo" | "wszystko" | "すべて" | "全部")
}

fn chip(c: &In) -> Option<(String, Option<String>, bool)> {
    if let Some(r) = c.get("chipCloudChipRenderer") {
        let label = r.get("text").and_then(text)?;
        let token = r.pointer("/navigationEndpoint/continuationCommand/token").and_then(In::as_str).map(String::from);
        return Some((label, token, r.get("isSelected").and_then(In::as_bool).unwrap_or(false)));
    }
    let r = c.get("chipViewModel")?;
    let label = r.get("text").and_then(|t| t.as_str().map(String::from).or_else(|| text(t)))?;
    let token = r
        .pointer("/tapCommand/innertubeCommand/continuationCommand/token")
        .or_else(|| r.pointer("/tapCommand/innertubeCommand/commandExecutorCommand/commands/0/continuationCommand/token"))
        .and_then(In::as_str)
        .map(String::from);
    let sel = r.get("selected").and_then(In::as_bool).or_else(|| r.get("isSelected").and_then(In::as_bool)).unwrap_or(false);
    Some((label, token, sel))
}

fn bar_chips(bar: &In, all: &mut Option<String>) -> Vec<Chip> {
    let list = bar.get("contents").or_else(|| bar.get("chips")).and_then(In::as_array);
    let mut out: Vec<Chip> = Vec::new();
    for (i, c) in list.into_iter().flatten().take(40).enumerate() {
        let Some((label, token, selected)) = chip(c) else { continue };
        let label: String = label.trim().chars().take(NAME_LEN * 2).collect();
        let key = key_of(&label);
        if key.is_empty() {
            continue;
        }
        // The first chip of Home is YouTube's "All" (selected on a fresh Home).
        if all_like(&key) || all.as_deref() == Some(key.as_str()) || (i == 0 && all.is_none() && selected) {
            *all = Some(key);
            continue;
        }
        // The selected chip may come without an endpoint: kept (its earlier token is reused).
        let Some(token) = token.filter(|t| (8..4096).contains(&t.len())).or_else(|| selected.then(String::new)) else { continue };
        if out.iter().any(|c| c.key == key) {
            continue;
        }
        out.push(Chip { label, key, token, selected });
    }
    out
}

fn find_bar<'a>(v: &'a In<'a>) -> Option<&'a In<'a>> {
    v.get("feedFilterChipBarRenderer").or_else(|| v.get("chipBarViewModel"))
}

/// A new bar replaces `old`; chips that came without a token keep their old one.
pub fn adopt(old: &mut Vec<Chip>, mut new: Vec<Chip>) {
    for c in new.iter_mut().filter(|c| c.token.is_empty()) {
        if let Some(o) = old.iter().find(|o| o.key == c.key) {
            c.token = o.token.clone();
        }
    }
    *old = new;
}

/// The chip bar of a Home `browse` response (rich grid header) or of a chip's
/// continuation response (header slot). `all` remembers the All chip's key.
pub fn parse(v: &In, all: &mut Option<String>) -> Option<Vec<Chip>> {
    let tabs = v.pointer("/contents/twoColumnBrowseResultsRenderer/tabs").and_then(In::as_array);
    for t in tabs.into_iter().flatten() {
        if let Some(bar) = t.pointer("/tabRenderer/content/richGridRenderer/header").and_then(find_bar) {
            return Some(bar_chips(bar, all));
        }
    }
    for key in ["onResponseReceivedActions", "onResponseReceivedCommands"] {
        for a in v.get(key).and_then(In::as_array).into_iter().flatten() {
            let items = a.pointer("/reloadContinuationItemsCommand/continuationItems").and_then(In::as_array);
            for it in items.into_iter().flatten() {
                if let Some(bar) = find_bar(it) {
                    return Some(bar_chips(bar, all));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------- preferences

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pref {
    pub key: String,
    pub name: String,
    pub hide: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub list: Vec<Pref>,
}

impl Prefs {
    /// `{version: 1, topics: [{key, name, hide?}]}`; anything else → empty.
    pub fn from_json(v: &In) -> Prefs {
        let mut out = Prefs::default();
        if v.get("version").and_then(In::as_u64) != Some(1) {
            return out;
        }
        for t in v.get("topics").and_then(In::as_array).into_iter().flatten().take(MAX) {
            let (Some(k), Some(n)) = (t.get("key").and_then(In::as_str), t.get("name").and_then(In::as_str)) else { continue };
            let key = key_of(k);
            if key.is_empty() || all_like(&key) || out.list.iter().any(|p| p.key == key) {
                continue;
            }
            out.list.push(Pref { key, name: n.chars().take(NAME_LEN).collect(), hide: t.get("hide").and_then(In::as_bool).unwrap_or(false) });
        }
        out
    }

    pub fn to_json(&self) -> String {
        let topics: Vec<Value> = self
            .list
            .iter()
            .map(|p| if p.hide { json!({"key": p.key, "name": p.name, "hide": true}) } else { json!({"key": p.key, "name": p.name}) })
            .collect();
        json!({"version": 1, "topics": topics}).to_string()
    }

    fn get(&self, key: &str) -> Option<&Pref> {
        self.list.iter().find(|p| p.key == key)
    }
}

/// A chip as the bar and the editor show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub key: String,
    /// The user's name for it, else YouTube's label.
    pub label: String,
    /// YouTube's own label (what an empty rename restores).
    pub original: String,
    pub hidden: bool,
}

impl Shown {
    pub fn renamed(&self) -> bool {
        self.label != self.original
    }
}

/// Chips in the user's order (stored records first, then the rest in
/// YouTube's order), with their hidden state. A stored name is the label.
pub fn arrange(chips: &[Chip], prefs: &Prefs) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::with_capacity(chips.len());
    for p in &prefs.list {
        if let Some(c) = chips.iter().find(|c| c.key == p.key) {
            let label = if p.name.trim().is_empty() { c.label.clone() } else { p.name.clone() };
            out.push(Shown { key: c.key.clone(), label, original: c.label.clone(), hidden: p.hide });
        }
    }
    for c in chips {
        if !out.iter().any(|s| s.key == c.key) {
            out.push(Shown { key: c.key.clone(), label: c.label.clone(), original: c.label.clone(), hidden: false });
        }
    }
    out
}

/// A name as stored: control characters dropped, whitespace collapsed, ≤ 40 characters.
pub fn clean_name(s: &str) -> String {
    let words: Vec<&str> = s.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    words.join(" ").chars().take(NAME_LEN).collect()
}

/// Renames chip `key`; an empty name (or YouTube's own label) restores YouTube's label.
pub fn renamed(chips: &[Chip], prefs: &Prefs, key: &str, name: &str) -> Prefs {
    let name = clean_name(name);
    let mut order = arrange(chips, prefs);
    for s in order.iter_mut().filter(|s| s.key == key) {
        s.label = if name.is_empty() { s.original.clone() } else { name.clone() };
    }
    rebuild(order, prefs)
}

/// New preferences from a display order: available chips in that order,
/// then stored records of chips YouTube doesn't offer right now. Over the
/// bound, plain order records go first (unavailable ones before available
/// ones); hidden records are kept as long as possible.
fn rebuild(order: Vec<Shown>, prefs: &Prefs) -> Prefs {
    // The name is stored only when the user renamed the chip (else YouTube's label shows, even
    // when YouTube changes it later).
    let mut list: Vec<Pref> = order
        .iter()
        .map(|s| Pref { key: s.key.clone(), name: if s.renamed() { s.label.chars().take(NAME_LEN).collect() } else { String::new() }, hide: s.hidden })
        .collect();
    let avail = list.len();
    list.extend(prefs.list.iter().filter(|p| !order.iter().any(|s| s.key == p.key)).cloned());
    for range_start in [avail, 0] {
        let mut i = list.len();
        while list.len() > MAX && i > range_start {
            i -= 1;
            if !list[i].hide {
                list.remove(i);
            }
        }
    }
    list.truncate(MAX);
    Prefs { list }
}

/// Moves chip `key` one place earlier (`delta` < 0) or later.
pub fn moved(chips: &[Chip], prefs: &Prefs, key: &str, delta: i32) -> Prefs {
    let mut order = arrange(chips, prefs);
    if let Some(i) = order.iter().position(|s| s.key == key) {
        let j = i as i64 + delta.signum() as i64;
        if (0..order.len() as i64).contains(&j) {
            order.swap(i, j as usize);
        }
    }
    rebuild(order, prefs)
}

/// Hides or shows chip `key`.
pub fn toggled(chips: &[Chip], prefs: &Prefs, key: &str) -> Prefs {
    let mut order = arrange(chips, prefs);
    for s in order.iter_mut().filter(|s| s.key == key) {
        s.hidden = !s.hidden;
    }
    rebuild(order, prefs)
}

pub fn is_hidden(prefs: &Prefs, key: &str) -> bool {
    prefs.get(key).is_some_and(|p| p.hide)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud(label: &str, token: Option<&str>, sel: bool) -> Value {
        let mut c = json!({"chipCloudChipRenderer": {"text": {"runs": [{"text": label}]}, "isSelected": sel}});
        if let Some(t) = token {
            c["chipCloudChipRenderer"]["navigationEndpoint"] = json!({"continuationCommand": {"token": t, "request": "CONTINUATION_REQUEST_TYPE_BROWSE"}});
        }
        c
    }

    /// Hand-built signed-in Home (the signed-out fixture has no chip bar).
    pub fn home_with_chips() -> In<'static> {
        crate::json::jv!({"contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"selected": true, "content": {"richGridRenderer": {
            "header": {"feedFilterChipBarRenderer": {"contents": [cloud("All", None, true), cloud("Gaming", Some("TOKGAMING1"), false),
                cloud("Podcasts", Some("TOKPODCAST"), false), cloud("<b>Music</b>", Some("TOKMUSIC12"), false), cloud("Gaming", Some("TOKDUPLIC8"), false),
                cloud("New to you", None, false)]}},
            "contents": []}}}}]}}})
    }

    #[test]
    fn parse_both_formats() {
        let mut all = None;
        let c = parse(&home_with_chips(), &mut all).unwrap();
        let labels: Vec<&str> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Gaming", "Podcasts", "<b>Music</b>"], "All and token-less chips dropped, duplicates once");
        assert_eq!(all.as_deref(), Some("all"));
        assert_eq!(c[0].token, "TOKGAMING1");
        // A chip's continuation: the header slot carries the bar again (All now has a token).
        let cont = crate::json::jv!({"onResponseReceivedActions": [{"reloadContinuationItemsCommand": {"slot": "RELOAD_CONTINUATION_SLOT_HEADER", "continuationItems": [
            {"chipBarViewModel": {"chips": [{"chipViewModel": {"text": "All", "tapCommand": {"innertubeCommand": {"continuationCommand": {"token": "TOKALL0001"}}}}},
                {"chipViewModel": {"text": "Gaming", "selected": true, "tapCommand": {"innertubeCommand": {"continuationCommand": {"token": "TOKGAMING2"}}}}}]}}]}},
            {"reloadContinuationItemsCommand": {"slot": "RELOAD_CONTINUATION_SLOT_BODY", "continuationItems": []}}]});
        let c = parse(&cont, &mut all).unwrap();
        assert_eq!(c, vec![Chip { label: "Gaming".into(), key: "gaming".into(), token: "TOKGAMING2".into(), selected: true }]);
        let mut bar = parse(&home_with_chips(), &mut all).unwrap();
        let mut sel = c.clone();
        sel[0].token.clear();
        adopt(&mut bar, sel);
        assert_eq!(bar[0].token, "TOKGAMING1", "selected chip without endpoint keeps its token");
        // The signed-out Home fixture has no chips.
        assert_eq!(parse(&crate::util::test_fixture("home.json"), &mut None), None);
    }

    #[test]
    fn preferences() {
        let chips = parse(&home_with_chips(), &mut None).unwrap();
        let p = Prefs::default();
        let keys = |v: &[Shown]| v.iter().map(|s| s.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&arrange(&chips, &p)), ["gaming", "podcasts", "<b>music</b>"]);
        let p = moved(&chips, &p, "podcasts", -1);
        assert_eq!(keys(&arrange(&chips, &p)), ["podcasts", "gaming", "<b>music</b>"]);
        let p = toggled(&chips, &p, "gaming");
        assert!(is_hidden(&p, "gaming"));
        assert!(arrange(&chips, &p)[1].hidden);
        let p = moved(&chips, &p, "podcasts", -1);
        assert_eq!(keys(&arrange(&chips, &p))[0], "podcasts", "first stays first");
        // Round trip through storage; unavailable records survive edits.
        let stored: Value = serde_json::from_str(&p.to_json()).unwrap();
        let mut back = Prefs::from_json(&In::from_serde(&stored));
        assert_eq!(back, p);
        back.list.push(Pref { key: "cooking".into(), name: "Cooking".into(), hide: true });
        let after = moved(&chips, &back, "<b>music</b>", -1);
        assert!(after.list.iter().any(|x| x.key == "cooking"), "temporarily unavailable preference kept");
        // Bound: ≤ 20 records, hidden ones kept over plain order records.
        let mut many = Prefs::default();
        for i in 0..20 {
            many.list.push(Pref { key: format!("old{i}"), name: format!("Old {i}"), hide: i % 2 == 0 });
        }
        let b = toggled(&chips, &many, "gaming");
        assert_eq!(b.list.len(), MAX);
        assert_eq!(b.list.iter().filter(|x| x.hide).count(), 11, "10 old hidden + gaming");
        assert!(b.list.iter().any(|x| x.key == "gaming" && x.hide));
        // Rename: the new name shows in the bar and survives moves; empty restores YouTube's label.
        let r = renamed(&chips, &p, "gaming", "  My   games\u{7} ");
        let shown = arrange(&chips, &r);
        let g = shown.iter().find(|s| s.key == "gaming").unwrap();
        assert_eq!((g.label.as_str(), g.original.as_str(), g.renamed(), g.hidden), ("My games", "Gaming", true, true));
        let r = moved(&chips, &r, "gaming", 1);
        assert_eq!(arrange(&chips, &r).iter().find(|s| s.key == "gaming").unwrap().label, "My games", "kept across a move");
        let back: Value = serde_json::from_str(&r.to_json()).unwrap();
        assert_eq!(Prefs::from_json(&In::from_serde(&back)), r, "stored");
        assert!(r.list.iter().filter(|x| x.key != "gaming").all(|x| x.name.is_empty()), "only renamed chips store a name");
        let r = renamed(&chips, &r, "gaming", "   ");
        assert_eq!(arrange(&chips, &r).iter().find(|s| s.key == "gaming").unwrap().label, "Gaming", "empty restores YouTube's label");
        assert!(r.list.iter().any(|x| x.key == "gaming" && x.hide && x.name.is_empty()), "hidden state kept");
        let long = renamed(&chips, &p, "podcasts", &"x".repeat(80));
        assert_eq!(arrange(&chips, &long)[0].label.len(), NAME_LEN);
        // A stored name from the old extension equal to YouTube's label is not a rename.
        let same = Prefs { list: vec![Pref { key: "gaming".into(), name: "Gaming".into(), hide: false }] };
        assert!(!arrange(&chips, &same)[0].renamed());
        // Garbage and old-version input.
        assert_eq!(Prefs::from_json(&crate::json::jv!({"version": 2, "topics": []})), Prefs::default());
        let g = Prefs::from_json(&crate::json::jv!({"version": 1, "topics": [{"key": "All", "name": "x"}, {"key": 5, "name": "y"}, {"key": " Big   Topic ", "name": "n".repeat(99)}]}));
        assert_eq!(g.list.len(), 1);
        assert_eq!((g.list[0].key.as_str(), g.list[0].name.len()), ("big topic", 40));
    }
}
