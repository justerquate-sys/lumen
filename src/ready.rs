//! The ready Home response: YouTube's real answer to the Home request (`browse FEwhat_to_watch`,
//! its first page plus that page's first continuation), fetched ahead in idle time so the next
//! reload or logo-Home paints it on its first frame without a network wait. It stands in for the
//! request that reload would have made, so it is used only while it is still what YouTube would
//! answer now:
//!
//! - the same account (signed in or not, session index, visitor, language: `acct`);
//! - nothing personalizing happened since it was fetched (`taint`: a watch started, a like, a
//!   subscription, a playlist or Watch later change, …; see `personalizes`);
//! - younger than `MAX_AGE_MS` (YouTube's answer drifts with time: tools/bench/home-drift.cjs).
//!
//! It is shown once (taken out when used) and lives in memory plus this tab's sessionStorage
//! (`KEY`, parsed and re-validated like a snapshot; never chrome.storage or localStorage), the same
//! place as the feed snapshots (CONTRIBUTING, privacy rules).

use crate::actions::notify::Topbar;
use crate::config::Auth;
use crate::features::home_chips::Chip;
use crate::json::{self, Value as In};
use crate::parse::Item;
use crate::snap::{item_from, item_json};
use crate::util::safe_img;
use serde_json::{Value, json};
use std::cell::Cell;

pub const KEY: &str = "ytc:ready";
/// A ready response older than this is not used (it is what YouTube answered then, not now).
/// 10 min: see docs/architecture.md "Home reload" for the drift curve behind it.
pub const MAX_AGE_MS: f64 = 10.0 * 60_000.0;
const VERSION: u64 = 1;
const MAX_ITEMS: usize = 150;
const MAX_CHIPS: usize = 40;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ready {
    /// When YouTube answered (epoch ms).
    pub at: f64,
    /// Whose answer it is (`acct`).
    pub acct: String,
    /// The first page's items, then its continuation's.
    pub items: Vec<Item>,
    /// How many of `items` the first page had (the page shows those; the rest is preloaded).
    pub first: usize,
    /// The continuation after the last item.
    pub cont: Option<String>,
    /// YouTube's Home chips (and the key of its All chip).
    pub chips: Vec<Chip>,
    pub all: Option<String>,
    pub topbar: Option<Topbar>,
    pub logged_out: Option<bool>,
}

thread_local! {
    /// Bumped by every personalizing event of this document (`taint`).
    static TAINT: Cell<u32> = const { Cell::new(0) };
}

/// The current taint generation: a ready response fetched under another one is stale.
pub fn generation() -> u32 {
    TAINT.with(Cell::get)
}

/// Something that changes what YouTube recommends happened: the ready response (memory and this
/// tab's copy) is no longer what YouTube would answer.
pub fn taint() {
    TAINT.with(|t| t.set(t.get().wrapping_add(1)));
    crate::bridge::session_del(KEY);
}

/// Requests that change what YouTube recommends (endpoint path after `/youtubei/v1/`).
pub fn personalizes(ep: &str) -> bool {
    ["like/", "subscription/", "browse/edit_playlist", "playlist/create", "playlist/delete", "feedback", "history/"].iter().any(|p| ep.starts_with(p))
}

fn short_hash(s: &str) -> String {
    sha1_smol::Sha1::from(s).digest().to_string()[..12].to_string()
}

/// The visitor's id inside `visitorData` (protobuf field 1, 11 characters), which stays while the
/// rest (a timestamp, region) is refreshed by YouTube.
pub fn visitor_id(vd: &str) -> Option<String> {
    let s = vd.replace("%3D", "=").replace('-', "+").replace('_', "/");
    let s = s.trim_end_matches('=');
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits: u32 = 0;
    let mut n = 0;
    let mut out = Vec::new();
    for c in s.bytes().take(24) {
        let v = A.iter().position(|x| *x == c)? as u32;
        bits = (bits << 6) | v;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    if out.len() < 13 || out[0] != 0x0a || out[1] != 11 {
        return None;
    }
    let id = std::str::from_utf8(&out[2..13]).ok()?;
    id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_').then(|| id.to_string())
}

/// Whose Home a response is: signed in (a hash of the session cookie, never the cookie) with its
/// session index, the TV app's signed-in account, or the signed-out visitor; plus the language. Anything else in the request is
/// the same for every Home request of this browser.
pub fn acct(auth: &Auth, au: u32, visitor: Option<&str>, hl: &str) -> String {
    // TV app: one account at a time, signed in with the device flow; its access token changes every
    // hour, so the key is "signed in" (signing in or out clears the tab's session data anyway).
    if auth.bearer.is_some() {
        return format!("t:{hl}");
    }
    if let Some(sid) = auth.sapisid.as_deref().or(auth.p3.as_deref()) {
        return format!("s{}:{au}:{hl}", short_hash(sid));
    }
    let v = visitor.map(|v| visitor_id(v).unwrap_or_else(|| short_hash(v))).unwrap_or_default();
    format!("o{}:{hl}", short_hash(&v))
}

/// Usable for a reload now: whose it is and how old.
pub fn usable(r: &Ready, acct: &str, now: f64) -> bool {
    r.acct == acct && now >= r.at - 60_000.0 && now - r.at < MAX_AGE_MS && !r.items.is_empty()
}

pub fn to_json(r: &Ready) -> String {
    let chips: Vec<Value> = r.chips.iter().take(MAX_CHIPS).map(|c| json!({"l": c.label, "k": c.key, "t": c.token, "s": c.selected})).collect();
    let tb = r.topbar.as_ref().map(|t| json!({"n": t.count, "a": t.avatar, "o": t.signed_out}));
    json!({
        "v": VERSION,
        "t": r.at,
        "a": r.acct,
        "i": r.items.iter().take(MAX_ITEMS).map(item_json).collect::<Vec<_>>(),
        "f": r.first.min(MAX_ITEMS),
        "c": r.cont,
        "h": chips,
        "al": r.all,
        "b": tb,
        "lo": r.logged_out,
    })
    .to_string()
}

fn so<'a>(v: &'a In<'a>, k: &str) -> Option<&'a str> {
    v.get(k).and_then(In::as_str)
}

/// Parses and re-validates a stored ready response (like a snapshot: ids, links and image hosts
/// are checked again, texts are escaped when rendered).
pub fn from_json(s: &str) -> Option<Ready> {
    let v = json::parse_str(s)?;
    if v.get("v").and_then(In::as_u64) != Some(VERSION) {
        return None;
    }
    let items: Vec<Item> = v.get("i")?.as_array()?.iter().take(MAX_ITEMS).filter_map(item_from).collect();
    let first = (v.get("f").and_then(In::as_u64).unwrap_or(0) as usize).min(items.len());
    let chips = v
        .get("h")
        .and_then(In::as_array)
        .into_iter()
        .flatten()
        .take(MAX_CHIPS)
        .filter_map(|c| {
            let label: String = so(c, "l")?.chars().take(80).collect();
            let key = crate::features::home_chips::key_of(&label);
            let token = so(c, "t").filter(|t| t.len() < 4096)?.to_string();
            (!key.is_empty() && so(c, "k") == Some(key.as_str())).then(|| Chip { label, key, token, selected: c.get("s").and_then(In::as_bool).unwrap_or(false) })
        })
        .collect();
    let topbar = v.get("b").filter(|b| b.as_object().is_some()).map(|b| Topbar {
        count: b.get("n").and_then(In::as_u64).map(|n| n.min(999) as u32),
        avatar: so(b, "a").and_then(safe_img),
        signed_out: b.get("o").and_then(In::as_bool).unwrap_or(false),
    });
    Some(Ready {
        at: v.get("t").and_then(In::as_f64)?,
        acct: so(&v, "a")?.chars().take(80).collect(),
        items,
        first,
        cont: so(&v, "c").filter(|c| !c.is_empty() && c.len() < 8192).map(Into::into),
        chips,
        all: so(&v, "al").map(|s| s.chars().take(120).collect()),
        topbar,
        logged_out: v.get("lo").and_then(In::as_bool),
    })
}

/// The Home browse durations this tab measured (ms, newest last; numbers only), for the hedge.
pub const TIMES_KEY: &str = "ytc:bt";
const TIMES_MAX: usize = 20;
/// Before enough samples: the owner's measured p90 of `req.browse` (2 Nov 2026: 1.0–1.6 s, p90 ≈ 1.3 s).
pub const HEDGE_DEFAULT_MS: f64 = 1300.0;
const HEDGE_MIN_MS: f64 = 700.0;
const HEDGE_MAX_MS: f64 = 2500.0;
/// No answer from either request this long after the first went out: the page shows this tab's
/// old Home dimmed ("Updating…") until YouTube answers.
pub const SLOW_MS: u32 = 4000;

pub fn times_from(s: &str) -> Vec<f64> {
    s.split(',').filter_map(|x| x.parse::<f64>().ok()).filter(|x| x.is_finite() && *x > 0.0 && *x < 120_000.0).collect()
}

pub fn times_push(s: &str, ms: f64) -> String {
    let mut t = times_from(s);
    t.push(ms.round());
    let skip = t.len().saturating_sub(TIMES_MAX);
    t[skip..].iter().map(|x| format!("{x:.0}")).collect::<Vec<_>>().join(",")
}

/// When to send the second (hedge) request: the p90 of this tab's Home requests (with 5+ samples),
/// clamped to 0.7–2.5 s, else 1.3 s. At p90 about one request in ten is hedged.
pub fn hedge_ms(times: &[f64]) -> f64 {
    if times.len() < 5 {
        return HEDGE_DEFAULT_MS;
    }
    let mut s = times.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let i = ((s.len() as f64 * 0.9).ceil() as usize).clamp(1, s.len()) - 1;
    s[i].clamp(HEDGE_MIN_MS, HEDGE_MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Video;

    fn sample() -> Ready {
        Ready {
            at: 1000.0,
            acct: "oabc:en".into(),
            items: vec![
                Item::Video(Video { id: "abcdefghijk".into(), title: "<b>one</b>".into(), thumb: Some("https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg".into()), ..Default::default() }),
                Item::Video(Video { id: "bbcdefghijk".into(), title: "two".into(), ..Default::default() }),
            ],
            first: 1,
            cont: Some("tok".into()),
            chips: vec![Chip { label: "Gaming".into(), key: "gaming".into(), token: "4qmFsgtok".into(), selected: false }],
            all: Some("all".into()),
            topbar: Some(Topbar { count: Some(3), avatar: Some("https://yt3.ggpht.com/a.jpg".into()), signed_out: false }),
            logged_out: Some(false),
        }
    }

    #[test]
    fn roundtrip_and_validation() {
        let r = sample();
        assert_eq!(from_json(&to_json(&r)), Some(r.clone()));
        // Tampered storage: hostile ids, hosts and chip keys are dropped like in a snapshot.
        let bad = to_json(&r).replace("abcdefghijk", "abc\\\"><x").replace("https://yt3.ggpht.com", "https://evil.example").replace("\"k\":\"gaming\"", "\"k\":\"other\"");
        let b = from_json(&bad).unwrap();
        assert_eq!(b.items.len(), 1);
        assert!(b.topbar.unwrap().avatar.is_none());
        assert!(b.chips.is_empty());
        assert!(from_json("{\"v\":2}").is_none() && from_json("nope").is_none());
    }

    #[test]
    fn usable_only_for_the_same_account_and_while_young() {
        let r = sample();
        assert!(usable(&r, "oabc:en", 1000.0 + MAX_AGE_MS - 1.0));
        assert!(!usable(&r, "oabc:en", 1000.0 + MAX_AGE_MS), "too old");
        assert!(!usable(&r, "oabc:de", 2000.0), "another language");
        assert!(!usable(&r, "sxyz:0:en", 2000.0), "another account");
        assert!(!usable(&r, "oabc:en", -100_000.0), "from the future (clock change)");
        assert!(!usable(&Ready { items: vec![], ..r }, "oabc:en", 2000.0));
    }

    #[test]
    fn account_keys() {
        let out = Auth::default();
        // The visitor id (field 1) is the key: YouTube refreshes the rest of visitorData.
        let a = "CgtlMmVGYWtlVmlzMCjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D";
        let b = "CgtlMmVGYWtlVmlzMCiShPzWBjIKCgJERRIEEgAgYg%3D%3D";
        assert_eq!(visitor_id(a).as_deref(), Some("e2eFakeVis0"));
        assert_eq!(acct(&out, 0, Some(a), "en"), acct(&out, 0, Some(b), "en"));
        assert_ne!(acct(&out, 0, Some(a), "en"), acct(&out, 0, Some("CgtlMmVGYWtlVmlzMSjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D"), "en"));
        assert_ne!(acct(&out, 0, Some(a), "en"), acct(&out, 0, None, "en"));
        let (s1, s2) = (Auth::from_cookies("SAPISID=one"), Auth::from_cookies("SAPISID=two"));
        assert_ne!(acct(&s1, 0, None, "en"), acct(&s2, 0, None, "en"), "another account");
        assert_ne!(acct(&s1, 0, None, "en"), acct(&s1, 1, None, "en"), "another session index");
        assert_ne!(acct(&s1, 0, Some(a), "en"), acct(&out, 0, Some(a), "en"), "signing in");
        assert!(!acct(&s1, 0, None, "en").contains("one"), "never the cookie itself");
        // TV: the hourly access token is no part of the key; signed in vs out is.
        let (t1, t2) = (Auth { bearer: Some("ya29.one".into()), ..Auth::default() }, Auth { bearer: Some("ya29.two".into()), ..Auth::default() });
        assert_eq!(acct(&t1, 0, None, "en"), acct(&t2, 0, None, "en"));
        assert_ne!(acct(&t1, 0, None, "en"), acct(&out, 0, None, "en"));
    }

    #[test]
    fn personalizing_endpoints() {
        for ep in ["like/like", "like/removelike", "subscription/subscribe", "browse/edit_playlist", "playlist/create", "feedback"] {
            assert!(personalizes(ep), "{ep}");
        }
        for ep in ["browse", "next", "playlist/get_add_to_playlist", "notification/get_notification_menu", "account/account_menu", "search"] {
            assert!(!personalizes(ep), "{ep}");
        }
    }

    #[test]
    fn hedge_deadline() {
        assert_eq!(hedge_ms(&[100.0, 200.0]), HEDGE_DEFAULT_MS);
        let t: Vec<f64> = (1..=10).map(|i| i as f64 * 200.0).collect();
        assert_eq!(hedge_ms(&t), 1800.0, "p90 of 200..2000");
        assert_eq!(hedge_ms(&[50.0; 10]), 700.0, "never below 0.7 s");
        assert_eq!(hedge_ms(&[9000.0; 10]), 2500.0, "never above 2.5 s");
        let mut s = String::new();
        for i in 0..25 {
            s = times_push(&s, i as f64);
        }
        assert_eq!(times_from(&s).len(), 20);
        assert_eq!(times_from("1,x,-3,5"), vec![1.0, 5.0]);
    }
}
