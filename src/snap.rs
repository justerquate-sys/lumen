//! Tab-lifetime feed snapshots (sessionStorage only, never chrome.storage or
//! localStorage). A snapshot is parsed data, not HTML: on load every field is
//! validated again exactly like a fresh InnerTube response and the page is
//! re-rendered through `render.rs`, so escaping stays in one place.

use crate::parse::{ChannelCard, Item, PlaylistCard, Video, anim_url};
use crate::render::Head;
use crate::util::{is_safe_id, is_video_id, safe_img, safe_rel_href};
use crate::json::{self, Value as In};
use serde_json::{Map, Value, json};

pub const PREFIX: &str = "ytc:snap:";
pub const INDEX: &str = "ytc:snaps";
/// Snapshots kept per tab (most recent pages).
pub const MAX_SNAPS: usize = 6;
/// Items stored per snapshot.
pub const MAX_ITEMS: usize = 150;
/// Serialized size cap per snapshot (sessionStorage is ~5 MB per origin and
/// shared with YouTube's own pages in this tab).
pub const MAX_BYTES: usize = 400_000;
const VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    pub url: String,
    pub head: Head,
    /// Rendered items first, then preloaded ones.
    pub items: Vec<Item>,
    /// How many of `items` were on screen.
    pub shown: usize,
    pub cont: Option<String>,
    pub cont_ep: &'static str,
    pub scroll: f64,
    /// Epoch ms when written.
    pub at: f64,
}

pub fn key(url: &str) -> String {
    format!("{PREFIX}{url}")
}

fn opt(m: &mut Map<String, Value>, k: &str, v: &Option<String>) {
    if let Some(v) = v {
        m.insert(k.into(), v.clone().into());
    }
}

fn nonempty(m: &mut Map<String, Value>, k: &str, v: &str) {
    if !v.is_empty() {
        m.insert(k.into(), v.into());
    }
}

fn item_json(it: &Item) -> Value {
    let mut m = Map::new();
    match it {
        Item::Video(v) => {
            m.insert("k".into(), "v".into());
            m.insert("i".into(), v.id.clone().into());
            nonempty(&mut m, "t", &v.title);
            nonempty(&mut m, "c", &v.channel);
            opt(&mut m, "h", &v.channel_href);
            opt(&mut m, "b", &v.channel_bid);
            nonempty(&mut m, "m", &v.meta);
            nonempty(&mut m, "d", &v.duration);
            if v.live {
                m.insert("l".into(), true.into());
            }
            if v.progress > 0 {
                m.insert("p".into(), v.progress.into());
            }
            if v.music {
                m.insert("u".into(), true.into());
            }
            if v.short {
                m.insert("r".into(), true.into());
            }
            opt(&mut m, "s", &v.thumb);
            if v.thumb_w > 0 {
                m.insert("sw".into(), v.thumb_w.into());
            }
            opt(&mut m, "x", &v.thumb2x);
            if v.thumb2x_w > 0 {
                m.insert("xw".into(), v.thumb2x_w.into());
            }
            opt(&mut m, "a", &v.anim);
        }
        Item::Playlist(p) => {
            m.insert("k".into(), "p".into());
            m.insert("i".into(), p.id.clone().into());
            nonempty(&mut m, "t", &p.title);
            nonempty(&mut m, "c", &p.channel);
            opt(&mut m, "h", &p.channel_href);
            nonempty(&mut m, "n", &p.count);
            opt(&mut m, "s", &p.thumb);
            opt(&mut m, "f", &p.first_video);
        }
        Item::Channel(c) => {
            m.insert("k".into(), "c".into());
            m.insert("h".into(), c.href.clone().into());
            opt(&mut m, "b", &c.bid);
            nonempty(&mut m, "t", &c.title);
            nonempty(&mut m, "m", &c.meta);
            opt(&mut m, "s", &c.avatar);
        }
    }
    Value::Object(m)
}

const TEXT_MAX: usize = 5000;

fn s(v: &In, k: &str) -> String {
    v.get(k).and_then(In::as_str).map(|x| x.chars().take(TEXT_MAX).collect()).unwrap_or_default()
}

fn so<'a>(v: &'a In<'a>, k: &str) -> Option<&'a str> {
    v.get(k).and_then(In::as_str)
}

fn w(v: &In, k: &str) -> u16 {
    v.get(k).and_then(In::as_u64).unwrap_or(0).min(4000) as u16
}

fn item_from(v: &In) -> Option<Item> {
    match so(v, "k")? {
        "v" => {
            let id = so(v, "i").filter(|x| is_video_id(x))?;
            Some(Item::Video(Video {
                id: id.into(),
                title: s(v, "t"),
                channel: s(v, "c"),
                channel_href: so(v, "h").and_then(safe_rel_href),
                channel_bid: so(v, "b").filter(|x| is_safe_id(x)).map(Into::into),
                meta: s(v, "m"),
                duration: s(v, "d"),
                live: v.get("l").and_then(In::as_bool).unwrap_or(false),
                progress: v.get("p").and_then(In::as_u64).unwrap_or(0).min(100) as u8,
                thumb: so(v, "s").and_then(safe_img),
                thumb_w: w(v, "sw"),
                thumb2x: so(v, "x").and_then(safe_img),
                thumb2x_w: w(v, "xw"),
                anim: anim_url(so(v, "a")),
                music: v.get("u").and_then(In::as_bool).unwrap_or(false),
                short: v.get("r").and_then(In::as_bool).unwrap_or(false),
            }))
        }
        "p" => {
            let id = so(v, "i").filter(|x| is_safe_id(x))?;
            Some(Item::Playlist(PlaylistCard {
                id: id.into(),
                title: s(v, "t"),
                channel: s(v, "c"),
                channel_href: so(v, "h").and_then(safe_rel_href),
                count: s(v, "n"),
                thumb: so(v, "s").and_then(safe_img),
                first_video: so(v, "f").filter(|x| is_video_id(x)).map(Into::into),
            }))
        }
        "c" => Some(Item::Channel(ChannelCard {
            href: so(v, "h").and_then(safe_rel_href)?,
            bid: so(v, "b").filter(|x| is_safe_id(x)).map(Into::into),
            title: s(v, "t"),
            meta: s(v, "m"),
            avatar: so(v, "s").and_then(safe_img),
        })),
        _ => None,
    }
}

fn head_json(h: &Head) -> Value {
    match h {
        Head::Feed { title } => json!({"k": "f", "t": title}),
        Head::Channel { title, avatar, meta, about } => json!({"k": "c", "t": title, "a": avatar, "m": meta, "ab": about}),
        Head::Playlist { title, meta } => json!({"k": "p", "t": title, "m": meta}),
    }
}

fn strings(v: &In, k: &str) -> Vec<String> {
    v.get(k).and_then(In::as_array).map(|a| a.iter().filter_map(In::as_str).take(8).map(|x| x.chars().take(TEXT_MAX).collect()).collect()).unwrap_or_default()
}

fn head_from(v: &In) -> Option<Head> {
    Some(match so(v, "k")? {
        "f" => Head::Feed { title: s(v, "t") },
        "c" => Head::Channel { title: s(v, "t"), avatar: so(v, "a").and_then(safe_img), meta: strings(v, "m"), about: s(v, "ab") },
        "p" => Head::Playlist { title: s(v, "t"), meta: strings(v, "m") },
        _ => return None,
    })
}

pub fn ep(s: &str) -> Option<&'static str> {
    ["browse", "search", "next"].into_iter().find(|e| *e == s)
}

pub fn to_json(sn: &Snap) -> String {
    let items: Vec<Value> = sn.items.iter().take(MAX_ITEMS).map(item_json).collect();
    json!({
        "v": VERSION,
        "u": sn.url,
        "h": head_json(&sn.head),
        "i": items,
        "n": sn.shown.min(MAX_ITEMS),
        "c": sn.cont,
        "e": sn.cont_ep,
        "y": sn.scroll.max(0.0).round(),
        "t": sn.at,
    })
    .to_string()
}

/// Parses and re-validates a snapshot written for `url`.
pub fn from_json(s: &str, url: &str) -> Option<Snap> {
    let v = json::parse_str(s)?;
    if v.get("v").and_then(In::as_u64) != Some(VERSION) || so(&v, "u") != Some(url) {
        return None;
    }
    let items: Vec<Item> = v.get("i")?.as_array()?.iter().take(MAX_ITEMS).filter_map(item_from).collect();
    let shown = (v.get("n").and_then(In::as_u64).unwrap_or(0) as usize).min(items.len());
    Some(Snap {
        url: url.to_string(),
        head: head_from(v.get("h")?)?,
        cont: so(&v, "c").filter(|c| !c.is_empty() && c.len() < 8192).map(Into::into),
        cont_ep: ep(so(&v, "e").unwrap_or(""))?,
        scroll: v.get("y").and_then(In::as_f64).unwrap_or(0.0).clamp(0.0, 1e7),
        at: v.get("t").and_then(In::as_f64).unwrap_or(0.0),
        items,
        shown,
    })
}

/// Most-recent-first list of snapshot URLs.
pub fn index_from(s: &str) -> Vec<String> {
    json::parse_str(s)
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(In::as_str).map(String::from).collect()))
        .unwrap_or_default()
}

pub fn index_json(urls: &[String]) -> String {
    Value::Array(urls.iter().map(|u| Value::from(u.as_str())).collect()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_validation() {
        let v = Video {
            id: "abcdefghijk".into(),
            title: "<b>t</b>".into(),
            channel_href: Some("/@x".into()),
            thumb: Some("https://i.ytimg.com/vi/abcdefghijk/hq720.jpg".into()),
            thumb_w: 360,
            anim: Some("https://i.ytimg.com/an_webp/abcdefghijk/mqdefault_6s.webp?du=3000".into()),
            ..Default::default()
        };
        // A Short keeps its mark through a snapshot (the Shorts switch hides it again after a reload).
        let short = Video { id: "shortshort1".into(), duration: "Shorts".into(), short: true, ..Default::default() };
        let sn = Snap {
            url: "/".into(),
            head: Head::Feed { title: "Home".into() },
            items: vec![Item::Video(v), Item::Channel(ChannelCard { href: "/@y".into(), ..Default::default() })],
            shown: 1,
            cont: Some("tok".into()),
            cont_ep: "browse",
            scroll: 120.0,
            at: 5.0,
        };
        let j = to_json(&sn);
        assert_eq!(from_json(&j, "/"), Some(sn.clone()));
        let mut sn2 = sn.clone();
        sn2.items.push(Item::Video(short));
        let back = from_json(&to_json(&sn2), "/").unwrap();
        assert!(back.items[2].is_short(), "the Short mark survives a snapshot");
        assert_eq!(back, sn2);
        assert!(from_json(&j, "/feed/subscriptions").is_none(), "keyed by URL");
        // Tampered storage: hostile ids/URLs are dropped exactly like in parse.rs.
        let bad = j.replace("\"i\":\"abcdefghijk\"", "\"i\":\"abc\\\"><x\"").replace("/@y", "javascript:alert(1)").replace("https://i.ytimg.com/an_webp", "https://evil.example/an_webp");
        let back = from_json(&bad, "/").unwrap();
        assert!(back.items.is_empty(), "{back:?}");
        let bad2 = j.replace("https://i.ytimg.com/vi/", "https://evil.example/vi/");
        let Item::Video(v) = &from_json(&bad2, "/").unwrap().items[0] else { panic!() };
        assert!(v.thumb.is_none());
        assert!(from_json("{\"v\":1}", "/").is_none());
        assert!(from_json("garbage", "/").is_none());
        assert_eq!(index_from(&index_json(&["/".into(), "/x".into()])), vec!["/".to_string(), "/x".to_string()]);
    }
}
