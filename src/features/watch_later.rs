//! Watch later clock button on every video card: InnerTube
//! `browse/edit_playlist` add/remove for `WL` with the user's session, and a
//! lazy saved-state check (`playlist/get_add_to_playlist`) on the first hover
//! or focus of a button. Request shapes and outcome rules follow the
//! extension's `player/innertube.rs` (YourTube 1.0.7).

use crate::config::Config;
use crate::util::esc_into;
use crate::json::{self, Value as In};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub const EDIT: &str = "browse/edit_playlist";
pub const MEMBERSHIP: &str = "playlist/get_add_to_playlist";
pub const PAGE_URL: &str = "/playlist?list=WL";

/// Newest-save order from YouTube's own owned-playlist menu. Using the menu's order and params
/// also changes the order of continuations; reversing one loaded page cannot do that.
pub fn newest_order(v: &In<'_>) -> Option<crate::api::Req> {
    fn find<'a>(v: &'a In<'a>) -> Option<&'a In<'a>> {
        if let Some(menu) = v.get("sortFilterSubMenuRenderer") { return Some(menu); }
        match v {
            In::Object(m) => m.values().find_map(find),
            In::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    let menu = find(v)?;
    let items = menu.get("subMenuItems")?.as_array()?;
    let item = items.iter().find(|i| i.get("title").and_then(In::as_str).is_some_and(|t| t.eq_ignore_ascii_case("Date added (newest)")))?;
    if item.get("selected").and_then(In::as_bool) == Some(true) { return None; }
    let ep = item.pointer("/serviceEndpoint/playlistEditEndpoint")?;
    if ep.get("playlistId").and_then(In::as_str) != Some("WL") { return None; }
    let action = ep.get("actions")?.as_array()?.iter().find(|a| a.get("action").and_then(In::as_str) == Some("ACTION_SET_PLAYLIST_VIDEO_ORDER"))?;
    let order = action.get("playlistVideoOrder")?.as_u64().and_then(|n| u32::try_from(n).ok())?;
    Some(crate::api::Req::WatchLaterOrder { order, params: ep.get("params").and_then(In::as_str).map(str::to_string) })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    Add,
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not known to be saved.
    Idle,
    Saving(Edit),
    Saved,
    Error(Edit, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Changed,
    /// Already (or never) in Watch later: the requested state holds.
    Unchanged,
    Failed { reason: String, detail: String },
}

fn context(cfg: &Config) -> Value {
    let mut client = json!({"clientName": "WEB", "clientVersion": cfg.version, "hl": "en"});
    if let Some(v) = &cfg.visitor {
        client["visitorData"] = v.clone().into();
    }
    json!({"client": client, "user": {}})
}

pub fn edit_body(cfg: &Config, video_id: &str, edit: Edit) -> String {
    let action = match edit {
        Edit::Add => json!({"addedVideoId": video_id, "action": "ACTION_ADD_VIDEO"}),
        Edit::Remove => json!({"action": "ACTION_REMOVE_VIDEO_BY_VIDEO_ID", "removedVideoId": video_id}),
    };
    json!({"context": context(cfg), "playlistId": "WL", "actions": [action]}).to_string()
}

pub fn membership_body(cfg: &Config, video_id: &str) -> String {
    json!({"context": context(cfg), "videoIds": [video_id], "excludeWatchLater": false}).to_string()
}

pub fn url(endpoint: &str) -> String {
    format!("https://www.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false")
}

fn strip(body: &[u8]) -> Option<In<'_>> {
    let s = std::str::from_utf8(body).ok()?;
    json::parse_str(s.trim_start_matches(")]}'").trim())
}

pub fn classify(edit: Edit, http: u32, body: &[u8]) -> Outcome {
    let j = strip(body);
    let status = j.as_ref().and_then(|j| j.get("status")).and_then(In::as_str).unwrap_or("").to_string();
    let field = |k: &str| j.as_ref().and_then(|j| j.pointer(&format!("/error/{k}"))).and_then(In::as_str).unwrap_or("").chars().take(240).collect::<String>();
    let (estatus, message) = (field("status"), field("message"));
    let lower = format!("{status} {estatus} {message}").to_lowercase();
    if (200..300).contains(&http) && status == "STATUS_SUCCEEDED" {
        return Outcome::Changed;
    }
    let already = match edit {
        Edit::Add => lower.contains("already") || lower.contains("duplicate"),
        Edit::Remove => lower.contains("not found") || lower.contains("not in playlist") || lower.contains("does not exist"),
    };
    if already || (edit == Edit::Add && http == 409) {
        return Outcome::Unchanged;
    }
    let reason = match http {
        0 => "Network error".to_string(),
        200..=299 => "YouTube did not confirm the change".to_string(),
        401 => "Sign in required (401)".to_string(),
        403 => "YouTube rejected the request (403)".to_string(),
        429 => "Too many requests (429)".to_string(),
        500..=599 => format!("YouTube is unavailable ({http})"),
        _ => format!("YouTube rejected the request ({http})"),
    };
    let mut detail = format!("HTTP {http}");
    for p in [status.as_str(), estatus.as_str(), message.as_str()] {
        if !p.is_empty() {
            detail.push(' ');
            detail.push_str(p);
        }
    }
    Outcome::Failed { reason, detail }
}

/// Watch later membership from `get_add_to_playlist`: the option whose
/// `playlistId` is `WL`, by `containsSelectedVideos`. None if unfamiliar.
pub fn membership(body: &[u8]) -> Option<bool> {
    fn find(v: &In) -> Option<bool> {
        match v {
            In::Object(m) => {
                if m.get("playlistId").and_then(In::as_str) == Some("WL") {
                    match m.get("containsSelectedVideos").and_then(In::as_str) {
                        Some("ALL") | Some("SOME") => return Some(true),
                        Some("NONE") => return Some(false),
                        _ => {}
                    }
                }
                m.values().find_map(find)
            }
            In::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    find(&strip(body)?)
}

/// Per-video state for this tab (memory only).
#[derive(Default)]
pub struct Saves {
    pub state: HashMap<String, State>,
    /// Membership already asked (or being asked) for these.
    pub probed: HashMap<String, bool>,
    /// Request id → (video, edit or None for a membership probe).
    pub inflight: HashMap<u32, (String, Option<Edit>)>,
    /// Confirmed removals in this tab: an older response/snapshot cannot resurrect the card.
    removed: HashSet<String>,
}

impl Saves {
    pub fn get(&self, id: &str) -> State {
        self.state.get(id).cloned().unwrap_or(State::Idle)
    }
    /// The edit a click performs now (None while a change is in flight).
    pub fn click(&self, id: &str) -> Option<Edit> {
        match self.get(id) {
            State::Idle => Some(Edit::Add),
            State::Saved => Some(Edit::Remove),
            State::Saving(_) => None,
            State::Error(e, _) => Some(e),
        }
    }
    pub fn settle(&mut self, id: &str, edit: Edit, out: &Outcome) {
        if !matches!(out, Outcome::Failed { .. }) {
            match edit {
                Edit::Remove => { self.removed.insert(id.to_string()); }
                Edit::Add => { self.removed.remove(id); }
            }
        }
        let s = match (out, edit) {
            (Outcome::Failed { reason, .. }, e) => State::Error(e, reason.clone()),
            (_, Edit::Add) => State::Saved,
            (_, Edit::Remove) => State::Idle,
        };
        self.state.insert(id.to_string(), s);
    }
    pub fn hidden(&self, id: &str) -> bool {
        self.removed.contains(id) || matches!(self.state.get(id), Some(State::Saving(Edit::Remove)))
    }
    /// Cards on the Watch later playlist page are saved by definition.
    pub fn known_saved(&mut self, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            self.state.entry(id.clone()).or_insert(State::Saved);
            self.probed.insert(id, true);
        }
    }
}

pub fn btn_id(video: &str) -> String {
    format!("ytc-wl-{video}")
}

/// (class, aria-label, title) for a state.
pub fn view(s: &State) -> (&'static str, String, String) {
    match s {
        State::Idle => ("ca wl", "Save to Watch later".into(), "Save to Watch later".into()),
        // Optimistic look: the bookmark fills on the click (`on`) and empties on a remove; the
        // response confirms (`saved`) or rolls back (`error`).
        State::Saving(Edit::Add) => ("ca wl saving on", "Saving to Watch later".into(), "Saving to Watch later…".into()),
        State::Saving(Edit::Remove) => ("ca wl saving", "Removing from Watch later".into(), "Removing from Watch later…".into()),
        State::Saved => ("ca wl saved", "Saved to Watch later. Click to remove.".into(), "Saved to Watch later. Click to remove.".into()),
        State::Error(Edit::Add, r) => ("ca wl error", format!("Couldn’t save to Watch later: {r}. Click to retry."), format!("Couldn’t save to Watch later: {r}. Click to retry.")),
        State::Error(Edit::Remove, r) => ("ca wl error", format!("Couldn’t remove from Watch later: {r}. Click to retry."), format!("Couldn’t remove from Watch later: {r}. Click to retry.")),
    }
}

/// YouTube's bookmark (Save): the outline and a filled copy in one grid cell; CSS reveals the copy
/// (a clip-path sweep) once the video is in Watch later.
const CLOCK: &str = "<svg class=\"o\" viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M6.5 3.5h11a1 1 0 0 1 1 1v16l-6.5-4.2-6.5 4.2v-16a1 1 0 0 1 1-1z\"/></svg><svg class=\"f\" viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M6.5 3.5h11a1 1 0 0 1 1 1v16l-6.5-4.2-6.5 4.2v-16a1 1 0 0 1 1-1z\"/></svg>";

/// The card button (a `<button>`, so Tab/Enter/Space work; hover and focus
/// trigger the lazy membership check through `data-hover`/`data-focus`).
pub fn button(out: &mut String, video: &str, s: &State) {
    let (class, label, title) = view(s);
    out.push_str("<button type=\"button\" id=\"");
    out.push_str(&btn_id(video));
    out.push_str("\" class=\"");
    out.push_str(class);
    out.push_str("\" data-act=\"fx-wl\" data-arg=\"");
    out.push_str(video);
    out.push_str("\" data-hov=\"wl\" data-focus=\"wl\" aria-label=\"");
    esc_into(out, &label);
    out.push_str("\" title=\"");
    esc_into(out, &title);
    out.push_str("\" aria-busy=\"");
    out.push_str(if matches!(s, State::Saving(_)) { "true" } else { "false" });
    out.push_str("\">");
    out.push_str(CLOCK);
    out.push_str("</button>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_uses_the_owned_playlist_menu_command() {
        let mut raw = json!({"header":{"sortFilterSubMenuRenderer":{"subMenuItems":[
            {"title":"Date added (oldest)","selected":true},
            {"title":"Date added (newest)","selected":false,"serviceEndpoint":{"playlistEditEndpoint":{"playlistId":"WL","params":"provided-sort-context","actions":[{"action":"ACTION_SET_PLAYLIST_VIDEO_ORDER","playlistVideoOrder":7}]}}}
        ]}}});
        let read = |v: &Value| newest_order(&crate::json::parse_str(&v.to_string()).unwrap());
        let req = read(&raw).unwrap();
        assert_eq!(req.endpoint(), EDIT);
        let body: Value = serde_json::from_str(&req.body(&Config::default(), "de", PAGE_URL)).unwrap();
        assert_eq!(body["context"]["client"]["hl"], "en");
        assert_eq!(body["playlistId"], "WL");
        assert_eq!(body["params"], "provided-sort-context");
        assert_eq!(body["actions"][0]["playlistVideoOrder"], 7, "never infer an order number from the menu index");
        raw["header"]["sortFilterSubMenuRenderer"]["subMenuItems"][1]["selected"] = true.into();
        assert!(read(&raw).is_none(), "already newest makes no edit");
        raw["header"]["sortFilterSubMenuRenderer"]["subMenuItems"][1]["selected"] = false.into();
        raw["header"]["sortFilterSubMenuRenderer"]["subMenuItems"][1]["serviceEndpoint"]["playlistEditEndpoint"]["playlistId"] = "OTHER".into();
        assert!(read(&raw).is_none(), "never sort another playlist");
        assert!(newest_order(&crate::json::parse_str("{}").unwrap()).is_none());
    }

    #[test]
    fn removal_hides_immediately_rolls_back_and_survives_stale_pages() {
        let mut s = Saves::default();
        s.known_saved(["a".into(), "b".into()]);
        s.state.insert("a".into(), State::Saving(Edit::Remove));
        assert!(s.hidden("a"));
        assert!(!s.hidden("b"));
        s.settle("a", Edit::Remove, &Outcome::Failed { reason: "Unavailable".into(), detail: String::new() });
        assert!(!s.hidden("a"));
        s.state.insert("a".into(), State::Saving(Edit::Remove));
        s.settle("a", Edit::Remove, &Outcome::Changed);
        s.known_saved(["a".into()]);
        assert!(s.hidden("a"), "a stale continuation/snapshot cannot bring it back");
        s.settle("a", Edit::Add, &Outcome::Changed);
        assert!(!s.hidden("a"), "an explicit save adds it again");
    }

    #[test]
    fn request_shapes() {
        let mut cfg = Config::default();
        cfg.visitor = Some("VISIT".into());
        let b: Value = serde_json::from_str(&edit_body(&cfg, "dQw4w9WgXcQ", Edit::Add)).unwrap();
        assert_eq!(b["playlistId"], "WL");
        assert_eq!(b["actions"][0]["action"], "ACTION_ADD_VIDEO");
        assert_eq!(b["actions"][0]["addedVideoId"], "dQw4w9WgXcQ");
        assert_eq!(b["context"]["client"]["clientName"], "WEB");
        assert_eq!(b["context"]["client"]["visitorData"], "VISIT");
        let b: Value = serde_json::from_str(&edit_body(&cfg, "dQw4w9WgXcQ", Edit::Remove)).unwrap();
        assert_eq!(b["actions"][0]["action"], "ACTION_REMOVE_VIDEO_BY_VIDEO_ID");
        assert_eq!(b["actions"][0]["removedVideoId"], "dQw4w9WgXcQ");
        let m: Value = serde_json::from_str(&membership_body(&cfg, "dQw4w9WgXcQ")).unwrap();
        assert_eq!(m["videoIds"][0], "dQw4w9WgXcQ");
        assert_eq!(m["excludeWatchLater"], false);
        assert_eq!(url(EDIT), "https://www.youtube.com/youtubei/v1/browse/edit_playlist?prettyPrint=false");
    }

    #[test]
    fn outcomes() {
        assert_eq!(classify(Edit::Add, 200, br#"{"status":"STATUS_SUCCEEDED"}"#), Outcome::Changed);
        assert_eq!(classify(Edit::Add, 400, br#"{"error":{"code":400,"message":"Video already in playlist.","status":"FAILED_PRECONDITION"}}"#), Outcome::Unchanged);
        assert_eq!(classify(Edit::Add, 200, br#"{"status":"STATUS_ALREADY_EXISTS"}"#), Outcome::Unchanged);
        assert_eq!(classify(Edit::Remove, 404, br#"{"error":{"message":"Requested entity was not found.","status":"NOT_FOUND"}}"#), Outcome::Unchanged);
        match classify(Edit::Add, 401, br#"{"error":{"code":401,"message":"Request is missing required authentication credential.","status":"UNAUTHENTICATED"}}"#) {
            Outcome::Failed { reason, detail } => {
                assert_eq!(reason, "Sign in required (401)");
                assert!(detail.contains("UNAUTHENTICATED"));
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(classify(Edit::Remove, 200, br#"{"status":"STATUS_FAILED"}"#), Outcome::Failed { .. }));
        assert!(matches!(classify(Edit::Add, 0, b""), Outcome::Failed { reason, .. } if reason == "Network error"));
    }

    #[test]
    fn membership_reads_wl() {
        for (wl, want) in [("ALL", Some(true)), ("SOME", Some(true)), ("NONE", Some(false)), ("ODD", None)] {
            let body = format!(r#"{{"contents":[{{"addToPlaylistRenderer":{{"playlists":[{{"playlistAddToOptionRenderer":{{"playlistId":"PLx","containsSelectedVideos":"ALL"}}}},{{"playlistAddToOptionRenderer":{{"playlistId":"WL","containsSelectedVideos":"{wl}"}}}}]}}}}]}}"#);
            assert_eq!(membership(body.as_bytes()), want, "{wl}");
        }
        assert_eq!(membership(b"garbage"), None);
    }

    #[test]
    fn states_and_button() {
        let mut s = Saves::default();
        assert_eq!(s.click("a"), Some(Edit::Add));
        s.state.insert("a".into(), State::Saving(Edit::Add));
        assert_eq!(s.click("a"), None);
        s.settle("a", Edit::Add, &Outcome::Changed);
        assert_eq!(s.get("a"), State::Saved);
        assert_eq!(s.click("a"), Some(Edit::Remove));
        s.settle("a", Edit::Remove, &Outcome::Failed { reason: "Too many requests (429)".into(), detail: String::new() });
        assert_eq!(s.click("a"), Some(Edit::Remove), "retry repeats the failed edit");
        let mut out = String::new();
        button(&mut out, "abcdefghijk", &s.get("a"));
        assert!(out.contains("class=\"ca wl error\"") && out.contains("Couldn’t remove from Watch later: Too many requests (429). Click to retry."));
        assert!(out.contains("id=\"ytc-wl-abcdefghijk\"") && out.contains("data-act=\"fx-wl\""));
        s.known_saved(["b".to_string()]);
        assert_eq!(s.get("b"), State::Saved);
    }
}
