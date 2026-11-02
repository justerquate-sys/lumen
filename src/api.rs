//! InnerTube request construction (same-origin `fetch`, cookies included by
//! the browser). Response parsing lives in `parse.rs`.

use crate::config::{Auth, Config, ORIGIN};
use serde_json::{Map, Value, json};

/// Channel "Videos" tab params (stable for years: `EgZ2aWRlb3PyBgQKAjoA`).
pub const CHANNEL_VIDEOS_PARAMS: &str = "EgZ2aWRlb3PyBgQKAjoA";

#[derive(Clone, Debug, PartialEq)]
pub enum Req {
    Browse { id: String, params: Option<String> },
    /// `params` = YouTube's own search-filter value (verbatim).
    Search { q: String, params: Option<String> },
    Next { v: String },
    /// `next` inside a playlist (`list=`): the response adds the playlist panel.
    NextList { v: String, list: String },
    Resolve { url: String },
    Cont { ep: &'static str, token: String },
}

/// The WEB `context` object every InnerTube body carries.
pub fn context(cfg: &Config, hl: &str, page_url: &str) -> Value {
    let mut client = Map::new();
    client.insert("clientName".into(), "WEB".into());
    client.insert("clientVersion".into(), cfg.version.clone().into());
    client.insert("hl".into(), hl.into());
    client.insert("platform".into(), "DESKTOP".into());
    client.insert("originalUrl".into(), format!("{ORIGIN}{page_url}").into());
    if let Some(vd) = &cfg.visitor {
        client.insert("visitorData".into(), vd.clone().into());
    }
    json!({"client": Value::Object(client), "user": {"lockedSafetyMode": false}})
}

impl Req {
    pub fn endpoint(&self) -> &'static str {
        match self {
            Req::Browse { .. } => "browse",
            Req::Search { .. } => "search",
            Req::Next { .. } | Req::NextList { .. } => "next",
            Req::Resolve { .. } => "navigation/resolve_url",
            Req::Cont { ep, .. } => ep,
        }
    }

    pub fn url(&self) -> String {
        format!("{ORIGIN}/youtubei/v1/{}?prettyPrint=false", self.endpoint())
    }

    pub fn body(&self, cfg: &Config, hl: &str, page_url: &str) -> String {
        let mut body = Map::new();
        body.insert("context".into(), context(cfg, hl, page_url));
        match self {
            Req::Browse { id, params } => {
                body.insert("browseId".into(), id.clone().into());
                if let Some(p) = params {
                    body.insert("params".into(), p.clone().into());
                }
            }
            Req::Search { q, params } => {
                body.insert("query".into(), q.clone().into());
                if let Some(p) = params {
                    body.insert("params".into(), p.clone().into());
                }
            }
            Req::Next { v } => {
                body.insert("videoId".into(), v.clone().into());
                body.insert("racyCheckOk".into(), true.into());
                body.insert("contentCheckOk".into(), true.into());
            }
            Req::NextList { v, list } => {
                body.insert("videoId".into(), v.clone().into());
                body.insert("playlistId".into(), list.clone().into());
                body.insert("racyCheckOk".into(), true.into());
                body.insert("contentCheckOk".into(), true.into());
            }
            Req::Resolve { url } => {
                body.insert("url".into(), url.clone().into());
            }
            Req::Cont { token, .. } => {
                body.insert("continuation".into(), token.clone().into());
            }
        }
        Value::Object(body).to_string()
    }
}

/// The VISIONOS (Apple Vision Pro app) InnerTube client: its `player`
/// response has direct googlevideo URLs without cipher, `n` or PO token
/// (found by research on 1 Nov 2026; see docs/architecture.md "Player"). Kept as data so versions can be bumped.
pub const VISION_NAME: &str = "VISIONOS";
pub const VISION_ID: &str = "101";
pub const VISION_VERSION: &str = "1.02";
const VISION_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

pub fn player_url() -> String {
    format!("{ORIGIN}/youtubei/v1/player?prettyPrint=false")
}

/// VISIONOS `player` body and headers. `hl` is always "en" so playability
/// reasons (e.g. the bot check) are recognisable; metadata comes from WEB.
pub fn vision_player(cfg: &Config, video_id: &str) -> (String, String) {
    let mut client = json!({"clientName": VISION_NAME, "clientVersion": VISION_VERSION, "deviceMake": "Apple", "deviceModel": "RealityDevice17,1",
        "osName": "visionOS", "osVersion": "26.5.23O471", "userAgent": VISION_UA, "hl": "en"});
    let mut h = Map::new();
    h.insert("Content-Type".into(), "application/json".into());
    h.insert("X-Youtube-Client-Name".into(), VISION_ID.into());
    h.insert("X-Youtube-Client-Version".into(), VISION_VERSION.into());
    if let Some(vd) = &cfg.visitor {
        client["visitorData"] = vd.clone().into();
        h.insert("X-Goog-Visitor-Id".into(), vd.clone().into());
    }
    let body = json!({"context": {"client": client}, "videoId": video_id, "contentCheckOk": true, "racyCheckOk": true});
    (body.to_string(), Value::Object(h).to_string())
}

/// How the VISIONOS `player` call is sent. The default is anonymous with the
/// configured visitor; after a bot check the player tries the others and the
/// app keeps the one that worked for the rest of the session (memory only).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum VisionRoute {
    /// Credentials omitted, the configured visitor (the happy path).
    #[default]
    Anon,
    /// Credentials omitted, a freshly minted anonymous visitor (`visitor_id`).
    Fresh(String),
    /// With the account's session: cookies plus the SAPISIDHASH headers.
    Account,
}

impl VisionRoute {
    /// Short name for notes and incident reports (never the visitor itself).
    pub fn label(&self) -> &'static str {
        match self {
            VisionRoute::Anon => "anonymous",
            VisionRoute::Fresh(_) => "fresh visitor",
            VisionRoute::Account => "account session",
        }
    }
}

/// A VISIONOS `player` request for `route`: body, headers and fetch options.
/// `Anon` is byte-for-byte the plain `vision_player` request.
pub fn vision_request(cfg: &Config, auth: &Auth, ts_secs: u64, route: &VisionRoute, video_id: &str) -> (String, String, &'static str) {
    const OMIT: &str = "{\"credentials\":\"omit\"}";
    match route {
        VisionRoute::Anon => {
            let (b, h) = vision_player(cfg, video_id);
            (b, h, OMIT)
        }
        VisionRoute::Fresh(vd) => {
            let mut c = cfg.clone();
            c.visitor = Some(vd.clone());
            let (b, h) = vision_player(&c, video_id);
            (b, h, OMIT)
        }
        VisionRoute::Account => {
            let (b, h) = vision_player(cfg, video_id);
            let mut h: Value = serde_json::from_str(&h).unwrap_or_default();
            h["X-Origin"] = ORIGIN.into();
            h["X-Goog-AuthUser"] = cfg.auth_user.to_string().into();
            if let Some(a) = auth.header(ts_secs) {
                h["Authorization"] = a.into();
            }
            (b, h.to_string(), "{}")
        }
    }
}

/// `visitor_id`: mints a new anonymous visitor (`responseContext.visitorData`),
/// sent without cookies or visitor so it is not tied to this browser's.
pub fn visitor_id_url() -> String {
    format!("{ORIGIN}/youtubei/v1/visitor_id?prettyPrint=false")
}

pub fn visitor_id_request(cfg: &Config) -> (String, String) {
    let body = json!({"context": {"client": {"clientName": "WEB", "clientVersion": cfg.version, "hl": "en"}}});
    let h = json!({"Content-Type": "application/json", "X-Youtube-Client-Name": "1", "X-Youtube-Client-Version": cfg.version});
    (body.to_string(), h.to_string())
}

/// The visitor in a `visitor_id` (or any InnerTube) response.
pub fn parse_visitor(bytes: &[u8]) -> Option<String> {
    let v = crate::json::parse(bytes)?;
    v.pointer("/responseContext/visitorData").and_then(crate::json::Value::as_str).filter(|x| crate::config::is_visitor(x)).map(Into::into)
}

/// WEB `player` body (with the user's session headers from `headers`): used
/// for metadata, captions and `playbackTracking`; its SABR streams are ignored.
pub fn web_player(cfg: &Config, hl: &str, page_url: &str, video_id: &str, cpn: &str, sts: u32) -> String {
    let mut b: Value = crate::json::parse_str(&Req::Next { v: video_id.into() }.body(cfg, hl, page_url)).map(|v| v.to_serde()).unwrap_or_default();
    b["cpn"] = cpn.into();
    b["playbackContext"] = json!({"contentPlaybackContext": {"html5Preference": "HTML5_PREF_WANTS", "signatureTimestamp": sts,
        "referer": format!("{ORIGIN}{page_url}"), "currentUrl": page_url, "autonavState": "STATE_NONE", "vis": 0, "splay": false,
        "autoCaptionsDefaultOn": false, "lactMilliseconds": "-1"}});
    b.to_string()
}

/// Request headers as a JSON object for the bridge.
pub fn headers(cfg: &Config, auth: &Auth, ts_secs: u64) -> String {
    let mut h = Map::new();
    h.insert("Content-Type".into(), "application/json".into());
    h.insert("X-Youtube-Client-Name".into(), "1".into());
    h.insert("X-Youtube-Client-Version".into(), cfg.version.clone().into());
    h.insert("X-Origin".into(), ORIGIN.into());
    h.insert("X-Goog-AuthUser".into(), cfg.auth_user.to_string().into());
    if let Some(vd) = &cfg.visitor {
        h.insert("X-Goog-Visitor-Id".into(), vd.clone().into());
    }
    if let Some(a) = auth.header(ts_secs) {
        h.insert("Authorization".into(), a.into());
    }
    Value::Object(h).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_shape() {
        let cfg = Config::default();
        let b: Value = serde_json::from_str(&Req::Search { q: "a\"b".into(), params: None }.body(&cfg, "en", "/results?search_query=a")).unwrap();
        assert_eq!(b["query"], "a\"b");
        assert!(b.get("params").is_none());
        let b: Value = serde_json::from_str(&Req::Search { q: "a".into(), params: Some("EgIIAw%3D%3D".into()) }.body(&cfg, "en", "/")).unwrap();
        assert_eq!(b["params"], "EgIIAw%3D%3D", "YouTube's params verbatim");
        let b: Value = serde_json::from_str(&Req::NextList { v: "-Bxc4gJluC0".into(), list: "PLx".into() }.body(&cfg, "en", "/")).unwrap();
        assert_eq!((b["videoId"].as_str(), b["playlistId"].as_str()), (Some("-Bxc4gJluC0"), Some("PLx")));
        assert_eq!(b["context"]["client"]["clientName"], "WEB");
        assert_eq!(b["context"]["client"]["clientVersion"], crate::config::FALLBACK_VERSION);
        let h: Value = serde_json::from_str(&headers(&cfg, &Auth::from_cookies("SAPISID=x"), 1)).unwrap();
        assert_eq!(h["X-Youtube-Client-Name"], "1");
        assert!(h["Authorization"].as_str().unwrap().starts_with("SAPISIDHASH 1_"));
        let h: Value = serde_json::from_str(&headers(&cfg, &Auth::default(), 1)).unwrap();
        assert!(h.get("Authorization").is_none());
    }
    #[test]
    fn player_bodies() {
        let mut cfg = Config::default();
        cfg.visitor = Some("CgtlMmVGYWtlVmlzMCjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D".into());
        let (b, h) = vision_player(&cfg, "aqz-KE-bpKQ");
        let b: Value = serde_json::from_str(&b).unwrap();
        let h: Value = serde_json::from_str(&h).unwrap();
        assert_eq!(b["context"]["client"]["clientName"], "VISIONOS");
        assert_eq!(b["context"]["client"]["visitorData"], h["X-Goog-Visitor-Id"]);
        assert_eq!(h["X-Youtube-Client-Name"], "101");
        assert_eq!(b["videoId"], "aqz-KE-bpKQ");
        let w: Value = serde_json::from_str(&web_player(&cfg, "de", "/watch?v=aqz-KE-bpKQ", "aqz-KE-bpKQ", "abc", 20753)).unwrap();
        assert_eq!(w["context"]["client"]["clientName"], "WEB");
        assert_eq!(w["context"]["client"]["hl"], "de");
        assert_eq!(w["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"], 20753);
        assert_eq!(w["cpn"], "abc");
        assert_eq!(w["racyCheckOk"], true);
    }
    #[test]
    fn vision_routes() {
        let mut cfg = Config::default();
        cfg.visitor = Some("CgtlMmVGYWtlVmlzMCjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D".into());
        let auth = Auth::from_cookies("SAPISID=x");
        // The happy path is exactly the old request.
        let (b, h, o) = vision_request(&cfg, &auth, 1, &VisionRoute::Anon, "aqz-KE-bpKQ");
        assert_eq!((b, h), vision_player(&cfg, "aqz-KE-bpKQ"));
        assert_eq!(o, "{\"credentials\":\"omit\"}");
        // Fresh visitor: anonymous, the new visitor in body and header, nothing else changes.
        let fresh = "CgtlMmVGYWtlVmlzMSjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D";
        let (b, h, o) = vision_request(&cfg, &auth, 1, &VisionRoute::Fresh(fresh.into()), "aqz-KE-bpKQ");
        let (b, h): (Value, Value) = (serde_json::from_str(&b).unwrap(), serde_json::from_str(&h).unwrap());
        assert_eq!((b["context"]["client"]["visitorData"].as_str(), h["X-Goog-Visitor-Id"].as_str()), (Some(fresh), Some(fresh)));
        assert!(h.get("Authorization").is_none() && o.contains("omit"));
        assert_eq!(b["context"]["client"]["clientName"], "VISIONOS");
        // Account: cookies (default credentials) + SAPISIDHASH, the configured visitor.
        let (b, h, o) = vision_request(&cfg, &auth, 1, &VisionRoute::Account, "aqz-KE-bpKQ");
        let (b, h): (Value, Value) = (serde_json::from_str(&b).unwrap(), serde_json::from_str(&h).unwrap());
        assert_eq!(o, "{}");
        assert!(h["Authorization"].as_str().unwrap().starts_with("SAPISIDHASH 1_"));
        assert_eq!((h["X-Goog-AuthUser"].as_str(), h["X-Origin"].as_str(), h["X-Youtube-Client-Name"].as_str()), (Some("0"), Some(ORIGIN), Some("101")));
        assert_eq!(b["context"]["client"]["visitorData"], cfg.visitor.clone().unwrap().as_str());
        assert_eq!(VisionRoute::Fresh(fresh.into()).label(), "fresh visitor");
    }
    #[test]
    fn visitor_id() {
        let cfg = Config::default();
        let (b, h) = visitor_id_request(&cfg);
        let (b, h): (Value, Value) = (serde_json::from_str(&b).unwrap(), serde_json::from_str(&h).unwrap());
        assert_eq!(b["context"]["client"]["clientName"], "WEB");
        assert!(b["context"]["client"].get("visitorData").is_none(), "a new visitor, not ours");
        assert!(h.get("X-Goog-Visitor-Id").is_none() && h.get("Authorization").is_none());
        // Recorded 2 Nov 2026 (shape): responseContext.visitorData.
        let r = br#"{"responseContext":{"visitorData":"CgtlMmVGYWtlVmlzMSjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D","serviceTrackingParams":[]}}"#;
        assert_eq!(parse_visitor(r).as_deref(), Some("CgtlMmVGYWtlVmlzMSjAhPzWBjIKCgJaWhIEEgAgAA%3D%3D"));
        assert_eq!(parse_visitor(br#"{"responseContext":{"visitorData":"<x>"}}"#), None);
        assert_eq!(parse_visitor(b"not json"), None);
    }
}
