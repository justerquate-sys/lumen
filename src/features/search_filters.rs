//! Search filters: YouTube's own filter groups from the
//! search response (Type, Duration, Upload date, Features, Prioritize/Sort),
//! each option carrying the `params` YouTube's web client would send. Clicking
//! an option re-queries with exactly those `params`; nothing is fabricated.
//!
//! YouTube marks the selected options but gives them no endpoint, so removing
//! one needs the current `params` without it. `params` is a small protobuf
//! (base64, percent-encoded): field 1 = sort, field 2 = a message with one
//! field per filter. The field an option controls is learned from the
//! response itself (an unselected option's `params` differ from the current
//! ones in exactly that field); removal deletes that field from the current
//! bytes and keeps everything else byte for byte. Unknown cases offer only
//! "Clear filters" (the plain search).

use crate::parse::text;
use crate::route::valid_sp;
use crate::json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opt {
    pub label: String,
    /// The option's `searchEndpoint.params` (selected/disabled ones have none).
    pub params: Option<String>,
    pub selected: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub title: String,
    pub opts: Vec<Opt>,
}

/// Where a filter control leads: a search with these `params` (`None` = no filters).
pub type Target = Option<String>;

#[derive(Clone, Debug, PartialEq)]
pub struct OptView {
    pub label: String,
    pub selected: bool,
    /// `None` = not clickable (disabled, or a selected default / unknown removal).
    pub target: Option<Target>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupView {
    pub title: String,
    /// YouTube's Duration group (merged with the shared minimum-length filter).
    pub duration: bool,
    pub opts: Vec<OptView>,
}

/// A selected, non-default filter: shown as a chip; `remove` = its one-click removal.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub label: String,
    pub remove: Option<Target>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub groups: Vec<GroupView>,
    pub active: Vec<Active>,
}

/// A `params` field: (top-level number, sub-field number or 0).
type Path = (u32, u32);

/// Field paths learned per (group title, option label), session memory only.
#[derive(Default)]
pub struct Learned(Vec<(String, Path)>);

impl Learned {
    fn get(&self, k: &str) -> Option<Path> {
        self.0.iter().find(|e| e.0 == k).map(|e| e.1)
    }
    fn put(&mut self, k: String, p: Path) {
        if let Some(e) = self.0.iter_mut().find(|e| e.0 == k) {
            e.1 = p;
        } else {
            if self.0.len() >= LEARNED_MAX {
                self.0.clear();
            }
            self.0.push((k, p));
        }
    }
}

const LEARNED_MAX: usize = 300;
/// `params` field 2, sub-field 3: YouTube's duration filter.
const DURATION: Path = (2, 3);

// ------------------------------------------------------------- parsing

fn dialog_groups<'a>(v: &'a Value<'a>) -> Option<&'a Vec<Value<'a>>> {
    let p = |path: &[&str]| path.iter().try_fold(v, |o, k| o.get(*k)).and_then(Value::as_array);
    p(&["header", "searchHeaderRenderer", "searchFilterButton", "buttonRenderer", "command", "openPopupAction", "popup", "searchFilterOptionsDialogRenderer", "groups"])
        .or_else(|| p(&["contents", "twoColumnSearchResultsRenderer", "primaryContents", "sectionListRenderer", "subMenu", "searchSubMenuRenderer", "groups"]))
}

/// The filter groups of a search response (empty when it has none).
pub fn parse(v: &Value) -> Vec<Group> {
    let Some(groups) = dialog_groups(v) else { return Vec::new() };
    groups
        .iter()
        .filter_map(|g| g.get("searchFilterGroupRenderer"))
        .take(8)
        .map(|g| Group {
            title: g.get("title").and_then(text).unwrap_or_default(),
            opts: g
                .get("filters")
                .and_then(Value::as_array)
                .map(|fs| {
                    fs.iter()
                        .filter_map(|f| f.get("searchFilterRenderer"))
                        .take(20)
                        .filter_map(|f| {
                            let label = f.get("label").and_then(text)?.chars().take(60).collect::<String>();
                            let status = f.get("status").and_then(Value::as_str).unwrap_or("");
                            let params = f.pointer("/navigationEndpoint/searchEndpoint/params").and_then(Value::as_str).filter(|p| valid_sp(p)).map(String::from);
                            Some(Opt { label, params, selected: status == "FILTER_STATUS_SELECTED", disabled: status == "FILTER_STATUS_DISABLED" })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .filter(|g: &Group| !g.opts.is_empty())
        .collect()
}

// ------------------------------------------------- base64 + protobuf

fn b64_val(c: u8) -> Option<u8> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    })
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|&c| c != b'=') {
        acc = (acc << 6) | b64_val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn b64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len() * 4 / 3 + 4);
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= ch.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// `params` string → bytes ("EgIIAw%3D%3D" → base64 → protobuf bytes).
fn params_bytes(p: &str) -> Option<Vec<u8>> {
    b64_decode(&crate::util::pct_decode(p))
}

/// Bytes → `params` in YouTube's own form (base64, `+ / =` percent-encoded).
fn params_string(b: &[u8]) -> String {
    let mut out = String::new();
    for c in b64_encode(b).chars() {
        match c {
            '+' => out.push_str("%2B"),
            '/' => out.push_str("%2F"),
            '=' => out.push_str("%3D"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
struct Field {
    num: u32,
    wire: u8,
    /// Payload (varint bytes, fixed bytes, or length-delimited content).
    data: Vec<u8>,
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let x = *b.get(*i)?;
        *i += 1;
        v |= ((x & 0x7f) as u64) << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let x = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(x);
            return;
        }
        out.push(x | 0x80);
    }
}

fn fields(b: &[u8]) -> Option<Vec<Field>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let tag = varint(b, &mut i)?;
        let (num, wire) = ((tag >> 3) as u32, (tag & 7) as u8);
        if num == 0 {
            return None;
        }
        let start = i;
        let data = match wire {
            0 => {
                varint(b, &mut i)?;
                b[start..i].to_vec()
            }
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                i += n;
                b.get(start..i)?.to_vec()
            }
            2 => {
                let n = varint(b, &mut i)? as usize;
                let s = i;
                i = i.checked_add(n)?;
                b.get(s..i)?.to_vec()
            }
            _ => return None,
        };
        out.push(Field { num, wire, data });
        if out.len() > 64 {
            return None;
        }
    }
    Some(out)
}

fn put_field(out: &mut Vec<u8>, f: &Field) {
    put_varint(out, ((f.num as u64) << 3) | f.wire as u64);
    if f.wire == 2 {
        put_varint(out, f.data.len() as u64);
    }
    out.extend_from_slice(&f.data);
}

/// Decoded `params`: top-level fields; field 2 (the filter message) split into its fields.
#[derive(Clone, Debug, Default, PartialEq)]
struct Params {
    top: Vec<(Field, Option<Vec<Field>>)>,
}

impl Params {
    fn decode(p: Option<&str>) -> Option<Params> {
        let Some(p) = p else { return Some(Params::default()) };
        let top = fields(&params_bytes(p)?)?;
        Some(Params {
            top: top
                .into_iter()
                .map(|f| {
                    let sub = (f.num == 2 && f.wire == 2).then(|| fields(&f.data)).flatten();
                    (f, sub)
                })
                .collect(),
        })
    }

    /// (path, wire, payload) of every leaf field.
    fn leaves(&self) -> Vec<(Path, u8, &[u8])> {
        let mut out = Vec::new();
        for (f, sub) in &self.top {
            match sub {
                Some(s) => out.extend(s.iter().map(|c| ((f.num, c.num), c.wire, c.data.as_slice()))),
                None => out.push(((f.num, 0), f.wire, f.data.as_slice())),
            }
        }
        out
    }

    fn has(&self, path: Path) -> bool {
        self.leaves().iter().any(|l| l.0 == path)
    }

    /// The `params` without `path` (`None` when nothing is left = no filters).
    fn without(&self, path: Path) -> Target {
        let mut out = Vec::new();
        for (f, sub) in &self.top {
            match (sub, path) {
                (None, (n, 0)) if n == f.num => {}
                (Some(s), (n, c)) if n == f.num && c != 0 => {
                    let mut inner = Vec::new();
                    for x in s.iter().filter(|x| x.num != c) {
                        put_field(&mut inner, x);
                    }
                    if !inner.is_empty() {
                        put_field(&mut out, &Field { num: f.num, wire: 2, data: inner });
                    }
                }
                _ => put_field(&mut out, f),
            }
        }
        (!out.is_empty()).then(|| params_string(&out))
    }
}

/// The single field in which `opt` differs from `cur` (added, changed or removed).
fn delta(cur: &Params, opt: &Params) -> Option<Path> {
    let (a, b) = (cur.leaves(), opt.leaves());
    let mut diff: Vec<Path> = Vec::new();
    for l in b.iter().filter(|l| !a.contains(l)).chain(a.iter().filter(|l| !b.iter().any(|m| m.0 == l.0))) {
        if !diff.contains(&l.0) {
            diff.push(l.0);
        }
    }
    (diff.len() == 1).then(|| diff[0])
}

// ---------------------------------------------------------------- view

fn learn_key(g: &Group, o: &Opt) -> String {
    format!("{}\u{1f}{}", g.title, o.label)
}

/// What the panel and the active-filter chips show for the response `groups`
/// of the search with `sp`. Unselected Shorts options are left out (Shorts
/// are never shown).
pub fn view(groups: &[Group], sp: Option<&str>, learned: &mut Learned) -> View {
    let cur = Params::decode(sp);
    let mut out = View::default();
    for g in groups {
        // Field paths this group's options control (learned from their params).
        let gkey = format!("{}\u{1f}", g.title);
        let mut paths: Vec<Path> = Vec::new();
        for (_, p) in learned.0.iter().filter(|(k, _)| k.starts_with(&gkey)) {
            if !paths.contains(p) {
                paths.push(*p);
            }
        }
        if let Some(c) = &cur {
            for o in &g.opts {
                if let Some(path) = o.params.as_deref().and_then(|p| Params::decode(Some(p))).and_then(|p| delta(c, &p)) {
                    learned.put(learn_key(g, o), path);
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
        }
        let selected = g.opts.iter().filter(|o| o.selected).count();
        let mut gv = GroupView { title: g.title.clone(), duration: paths.contains(&DURATION), opts: Vec::new() };
        for o in &g.opts {
            if !o.selected && o.label.eq_ignore_ascii_case("shorts") {
                continue;
            }
            let target = if o.disabled {
                None
            } else if !o.selected {
                o.params.clone().map(Some)
            } else if let Some(p) = &o.params {
                Some(Some(p.clone()))
            } else {
                match &cur {
                    // Unreadable current params: one "Filters" chip clears them all (see below).
                    None => None,
                    Some(c) => {
                        let in_cur: Vec<Path> = paths.iter().copied().filter(|p| c.has(*p)).collect();
                        let path = learned.get(&learn_key(g, o)).filter(|p| c.has(*p)).or_else(|| (selected == 1 && in_cur.len() == 1).then(|| in_cur[0]));
                        match path {
                            Some(p) => {
                                let t = c.without(p);
                                out.active.push(Active { label: o.label.clone(), remove: Some(t.clone()) });
                                Some(t)
                            }
                            // Nothing of this group is in the current params: YouTube's default (e.g. Relevance).
                            None if in_cur.is_empty() && !paths.is_empty() => None,
                            None => {
                                out.active.push(Active { label: o.label.clone(), remove: None });
                                None
                            }
                        }
                    }
                }
            };
            gv.opts.push(OptView { label: o.label.clone(), selected: o.selected, target });
        }
        if !gv.opts.is_empty() {
            out.groups.push(gv);
        }
    }
    if cur.is_none() {
        out.active = vec![Active { label: "Filters".into(), remove: Some(None) }];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::util::test_fixture as fixture;

    #[test]
    fn codec_round_trip() {
        for p in ["EgIIAw%3D%3D", "EgQIBRAB", "CAMSAggF", "EgUIBdABAQ%3D%3D", "CAM%3D"] {
            let d = Params::decode(Some(p)).unwrap();
            let mut b = Vec::new();
            for (f, sub) in &d.top {
                match sub {
                    Some(s) => {
                        let mut inner = Vec::new();
                        s.iter().for_each(|x| put_field(&mut inner, x));
                        put_field(&mut b, &Field { num: f.num, wire: 2, data: inner });
                    }
                    None => put_field(&mut b, f),
                }
            }
            assert_eq!(params_string(&b), p, "byte-identical re-encoding");
        }
        // Removal gives exactly YouTube's own params for the smaller set.
        let both = Params::decode(Some("EgQIBRAB")).unwrap(); // This year + Videos
        assert_eq!(both.without((2, 2)), Some("EgIIBQ%3D%3D".into()));
        let sorted = Params::decode(Some("CAMSAggF")).unwrap(); // Popularity + This year
        assert_eq!(sorted.without((2, 1)), Some("CAM%3D".into()), "matches YouTube's Popularity-only params");
        assert_eq!(sorted.without((1, 0)), Some("EgIIBQ%3D%3D".into()));
        assert_eq!(Params::decode(Some("EgIIBQ%3D%3D")).unwrap().without((2, 1)), None, "last filter → plain search");
        assert_eq!(Params::decode(Some("%%%")), None);
        assert!(fields(&[0x0a, 0x7f]).is_none(), "truncated length");
    }

    #[test]
    fn unfiltered_response() {
        let g = parse(&fixture("search.json"));
        let titles: Vec<&str> = g.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Type", "Duration", "Upload date", "Features", "Prioritize"]);
        assert_eq!(g[3].opts.len(), 11, "Live, 4K, HD, Subtitles/CC, Creative Commons, 360°, VR180, 3D, HDR, Location, Purchased");
        let mut l = Learned::default();
        let v = view(&g, None, &mut l);
        assert!(v.active.is_empty(), "Relevance is YouTube's default, not an active filter");
        assert!(!v.groups[0].opts.iter().any(|o| o.label == "Shorts"), "Shorts type hidden");
        assert!(v.groups[1].duration && !v.groups[2].duration);
        let week = v.groups[2].opts.iter().find(|o| o.label == "This week").unwrap();
        assert_eq!(week.target, Some(Some("EgIIAw%3D%3D".into())), "YouTube's own params");
        let rel = v.groups[4].opts.iter().find(|o| o.label == "Relevance").unwrap();
        assert!(rel.selected && rel.target.is_none());
    }

    #[test]
    fn filtered_response_removal() {
        // Recorded signed out 1 Nov 2026 with params EgIIBRABGAI%3D; YouTube answered "This year" selected.
        let g = parse(&fixture("search_filtered.json"));
        let mut l = Learned::default();
        let v = view(&g, Some("EgIIBQ%3D%3D"), &mut l);
        assert_eq!(v.active, vec![Active { label: "This year".into(), remove: Some(None) }]);
        let types = &v.groups[0].opts;
        assert!(types.iter().any(|o| o.label == "Channels" && o.target.is_none()), "disabled option not clickable");
        assert_eq!(types.iter().find(|o| o.label == "Videos").unwrap().target, Some(Some("EgQIBRAB".into())));
        // Two filters: This year + Videos (learned paths from the earlier response).
        let mut g2 = g.clone();
        for o in g2[0].opts.iter_mut() {
            if o.label == "Videos" {
                o.selected = true;
                o.params = None;
            }
        }
        let v2 = view(&g2, Some("EgQIBRAB"), &mut l);
        assert_eq!(v2.active.len(), 2);
        assert_eq!(v2.active[0], Active { label: "Videos".into(), remove: Some(Some("EgIIBQ%3D%3D".into())) });
        assert_eq!(v2.active[1], Active { label: "This year".into(), remove: Some(Some("EgIQAQ%3D%3D".into())) });
        // Unreadable current params: one chip that clears all filters.
        let v3 = view(&g, Some("%%"), &mut Learned::default());
        assert_eq!(v3.active, vec![Active { label: "Filters".into(), remove: Some(None) }]);
    }

    #[test]
    fn hostile_input() {
        let v = crate::json::jv!({"header": {"searchHeaderRenderer": {"searchFilterButton": {"buttonRenderer": {"command": {"openPopupAction": {"popup": {"searchFilterOptionsDialogRenderer": {"groups": [
            {"searchFilterGroupRenderer": {"title": {"simpleText": "<b>T</b>"}, "filters": [{"searchFilterRenderer": {"label": {"simpleText": "x"}, "navigationEndpoint": {"searchEndpoint": {"params": "\"><script>"}}}}]}}]}}}}}}}}});
        let g = parse(&v);
        assert_eq!(g[0].opts[0].params, None, "unsafe params dropped");
        assert!(parse(&crate::json::jv!({})).is_empty());
    }
}
