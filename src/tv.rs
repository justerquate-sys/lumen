//! TV mode: the Google TV / Android TV app (`tv/` at the repository root) runs this same client in
//! its WebView and sets the platform flag (`YTC_PLATFORM = "tv"`, read once at boot through
//! `bridge::platform`). Everything here is inert on the desktop extension.
//!
//! - **Remote control**: D-pad spatial navigation (`pick`, pure and unit-tested), OK = Enter,
//!   Back (the shell calls `on_back`), media keys.
//! - **Account**: Google blocks password sign-in inside WebViews, so the TV app signs in like
//!   YouTube's own TV app: the OAuth device flow of the TV client (the shell holds the refresh
//!   token in Android Keystore-backed storage; the page only ever sees short-lived access tokens).
//!   Account calls go out as YouTube's own TV client does them: `TVHTML5` with `Authorization:
//!   Bearer` (tokens of the TV OAuth client are meant for it; WEB + bearer is reported refused since
//!   late 2024 — not verified here: with an invalid token every client answers 401, so only a real
//!   sign-in tells). The shared parser reads the TV response shapes (`tileRenderer`,
//!   `tvBrowseRenderer`, ...).
//!   Anonymous calls (search, watch page, channel, comments, VISIONOS streams) stay as on desktop.

use crate::config::{Auth, Config, ORIGIN};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};

/// YouTube's TV client (`/tv`, read 2 Nov 2026: `INNERTUBE_CLIENT_NAME` TVHTML5, version
/// `7.20261027.15.00`). Kept as data so the version can be bumped.
pub const TV_NAME: &str = "TVHTML5";
pub const TV_ID: &str = "7";
pub const TV_VERSION: &str = "7.20261027.15.00";
/// sessionStorage (this WebView tab only) mirror of the current access token: a reload sends its
/// first account request without waiting for the shell. Never chrome.storage / localStorage.
pub const SESSION_KEY: &str = "ytc:tvauth";
/// Ask the shell for a fresh access token this long before the current one expires.
pub const REFRESH_BEFORE_MS: f64 = 10.0 * 60.0 * 1000.0;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    /// (access token, expiry epoch ms)
    static BEARER: RefCell<Option<(String, f64)>> = const { RefCell::new(None) };
}

/// Called once at boot (before anything renders).
pub fn set_on(on: bool) {
    ON.with(|c| c.set(on));
}

/// The TV app is running this client.
pub fn on() -> bool {
    ON.with(Cell::get)
}

pub fn set_bearer(token: Option<(String, f64)>) {
    BEARER.with(|b| *b.borrow_mut() = token.filter(|(t, _)| valid_token(t)));
}

/// The current access token while it is valid at `now_ms`.
pub fn bearer_at(now_ms: f64) -> Option<String> {
    BEARER.with(|b| b.borrow().as_ref().filter(|(_, exp)| *exp > now_ms).map(|(t, _)| t.clone()))
}

/// Expiry of the token held (0 = none).
pub fn bearer_exp() -> f64 {
    BEARER.with(|b| b.borrow().as_ref().map_or(0.0, |x| x.1))
}

/// An OAuth access token as Google issues them (`ya29.` + URL-safe characters); anything else is refused.
pub fn valid_token(t: &str) -> bool {
    (20..=4096).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}

/// The shell's `auth_token` answer: `{"token": "...", "exp": epoch_ms}` or `{"token": null}`.
pub fn parse_token(json: &str) -> Option<(String, f64)> {
    let v = crate::json::parse_str(json)?;
    let t = v.get("token")?.as_str()?;
    let exp = v.get("exp")?.as_f64()?;
    valid_token(t).then(|| (t.to_string(), exp))
}

pub fn token_json(t: &str, exp: f64) -> String {
    json!({"token": t, "exp": exp}).to_string()
}

/// The account of the TV app: bearer when signed in. Cookies are never used on TV (Google's sign-in
/// does not run in the WebView, so there are none worth reading).
pub fn auth_at(now_ms: f64) -> Auth {
    Auth { bearer: bearer_at(now_ms), ..Auth::default() }
}

/// The account for requests made now: on TV the bearer, elsewhere the browser's cookies.
pub fn auth() -> Auth {
    if on() { auth_at(crate::bridge::epoch()) } else { Auth::from_cookies(&crate::bridge::cookie()) }
}

// ------------------------------------------------------------------ requests

/// The TVHTML5 `context`.
pub fn context(hl: &str) -> Value {
    json!({"client": {"clientName": TV_NAME, "clientVersion": TV_VERSION, "hl": hl, "platform": "TV"}, "user": {"lockedSafetyMode": false}})
}

/// Turns a WEB InnerTube body into the same request from the TV client (only `context` changes;
/// WEB-only fields such as `playbackContext.contentPlaybackContext.currentUrl` are harmless).
pub fn tv_body(web_body: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(web_body) else { return web_body.to_string() };
    let hl = v.pointer("/context/client/hl").and_then(Value::as_str).unwrap_or("en").to_string();
    v["context"] = context(&hl);
    v.to_string()
}

/// Headers for a TV account request.
pub fn headers(token: &str) -> String {
    json!({"Content-Type": "application/json", "X-Youtube-Client-Name": TV_ID, "X-Youtube-Client-Version": TV_VERSION,
        "X-Origin": ORIGIN, "Authorization": format!("Bearer {token}")})
    .to_string()
}

/// Body and headers for an account request: TVHTML5 + bearer when `auth` carries a token, else the
/// WEB request unchanged.
pub fn finish(cfg: &Config, auth: &Auth, web_body: String, ts_secs: u64) -> (String, String) {
    match &auth.bearer {
        Some(t) => (tv_body(&web_body), headers(t)),
        None => (web_body, crate::api::headers(cfg, auth, ts_secs)),
    }
}

/// Browse pages that belong to the account (sent with the bearer on TV): the feeds, the library and
/// the account's playlists (`VL…`: Watch later, Liked, own private playlists).
pub fn account_browse(id: &str) -> bool {
    matches!(id, "FEwhat_to_watch" | "FEsubscriptions" | "FEhistory" | "FEplaylist_aggregation" | "FElibrary") || id.starts_with("VL")
}

/// Authorization header for history pings (`/api/stats/...`) of a TV session.
pub fn ping_headers(auth: &Auth) -> String {
    match &auth.bearer {
        Some(t) => json!({"Authorization": format!("Bearer {t}")}).to_string(),
        None => "{}".into(),
    }
}

// ------------------------------------------------------- spatial navigation

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

pub fn dir(key: &str) -> Option<Dir> {
    Some(match key {
        "ArrowUp" => Dir::Up,
        "ArrowDown" => Dir::Down,
        "ArrowLeft" => Dir::Left,
        "ArrowRight" => Dir::Right,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn r(&self) -> f64 {
        self.x + self.w
    }
    fn b(&self) -> f64 {
        self.y + self.h
    }
}

/// `[x, y, w, h]*n` from the bridge.
pub fn rects(flat: &[f64]) -> Vec<Rect> {
    flat.chunks_exact(4).map(|c| Rect { x: c[0], y: c[1], w: c[2], h: c[3] }).collect()
}

/// Gap between two intervals (0 when they overlap).
fn gap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (b0 - a1).max(a0 - b1).max(0.0)
}

/// Overlap of two intervals as a fraction of the smaller one.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    let o = (a1.min(b1) - a0.max(b0)).max(0.0);
    let m = (a1 - a0).min(b1 - b0);
    if m > 0.0 { o / m } else { 0.0 }
}

/// The element the remote moves to from `cur` in `dir` (index into `rects`), like a TV launcher:
/// only candidates that lie beyond `cur` in that direction; the nearest along the axis wins,
/// with sideways distance weighed three times and items that overlap `cur` sideways (the same row
/// or column) preferred, so ↓ from a card lands on the card below, not a closer one diagonally.
pub fn pick(cur: Rect, rects: &[Rect], d: Dir, skip: Option<usize>) -> Option<usize> {
    const EPS: f64 = 1.0;
    let mut best: Option<(f64, usize)> = None;
    for (i, c) in rects.iter().enumerate() {
        if Some(i) == skip || c.w <= 0.0 || c.h <= 0.0 {
            continue;
        }
        let (ahead, along, side, ov) = match d {
            Dir::Right => (c.x >= cur.r() - EPS || (c.x > cur.x + EPS && c.r() > cur.r() + EPS), (c.x - cur.r()).max(0.0), gap(cur.y, cur.b(), c.y, c.b()), overlap(cur.y, cur.b(), c.y, c.b())),
            Dir::Left => (c.r() <= cur.x + EPS || (c.r() < cur.r() - EPS && c.x < cur.x - EPS), (cur.x - c.r()).max(0.0), gap(cur.y, cur.b(), c.y, c.b()), overlap(cur.y, cur.b(), c.y, c.b())),
            Dir::Down => (c.y >= cur.b() - EPS || (c.y > cur.y + EPS && c.b() > cur.b() + EPS), (c.y - cur.b()).max(0.0), gap(cur.x, cur.r(), c.x, c.r()), overlap(cur.x, cur.r(), c.x, c.r())),
            Dir::Up => (c.b() <= cur.y + EPS || (c.b() < cur.b() - EPS && c.y < cur.y - EPS), (cur.y - c.b()).max(0.0), gap(cur.x, cur.r(), c.x, c.r()), overlap(cur.x, cur.r(), c.x, c.r())),
        };
        if !ahead {
            continue;
        }
        // Center offset across the axis breaks ties between items of the same row/column.
        let center = match d {
            Dir::Left | Dir::Right => ((c.y + c.h / 2.0) - (cur.y + cur.h / 2.0)).abs(),
            Dir::Up | Dir::Down => ((c.x + c.w / 2.0) - (cur.x + cur.w / 2.0)).abs(),
        };
        let score = along + side * 3.0 + center * 0.05 - if ov > 0.0 { 40.0 * ov } else { 0.0 } + if ov == 0.0 { 200.0 } else { 0.0 };
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, i));
        }
    }
    best.map(|b| b.1)
}

/// What a key means on the remote.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Remote {
    PlayPause,
    Play,
    Pause,
    Rewind,
    Forward,
    Next,
    Prev,
    Stop,
}

/// Media keys as the WebView reports them (`KeyboardEvent.key`); the shell forwards the ones the
/// WebView keeps for itself under the same names.
pub fn remote(key: &str) -> Option<Remote> {
    Some(match key {
        "MediaPlayPause" => Remote::PlayPause,
        "MediaPlay" => Remote::Play,
        "MediaPause" => Remote::Pause,
        "MediaRewind" | "MediaTrackPreviousSeek" => Remote::Rewind,
        "MediaFastForward" => Remote::Forward,
        "MediaTrackNext" => Remote::Next,
        "MediaTrackPrevious" => Remote::Prev,
        "MediaStop" => Remote::Stop,
        _ => return None,
    })
}

/// The player key a remote key stands for (YouTube's own keys, which the player handles).
pub fn remote_player_key(r: Remote, paused: bool) -> Option<&'static str> {
    match r {
        Remote::PlayPause => Some("k"),
        Remote::Play => paused.then_some("k"),
        Remote::Pause => (!paused).then_some("k"),
        Remote::Rewind => Some("j"),
        Remote::Forward => Some("l"),
        Remote::Stop => (!paused).then_some("k"),
        Remote::Next | Remote::Prev => None,
    }
}

// ------------------------------------------------------------- device sign-in

/// The shell's `auth_start` answer (the code the owner types on their phone).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCode {
    pub user_code: String,
    pub url: String,
    pub interval_ms: f64,
    pub expires_ms: f64,
}

pub fn parse_device(json: &str) -> Option<DeviceCode> {
    let v = crate::json::parse_str(json)?;
    let code = v.get("user_code")?.as_str()?;
    let url = v.get("verification_url")?.as_str()?;
    let ok_code = (4..=24).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let ok_url = url.starts_with("https://") && url.len() < 120 && !url.contains(['"', '<', '>', ' ']);
    if !ok_code || !ok_url {
        return None;
    }
    let interval = v.get("interval").and_then(|x| x.as_f64()).unwrap_or(5.0).clamp(1.0, 60.0);
    let expires = v.get("expires_in").and_then(|x| x.as_f64()).unwrap_or(1800.0).clamp(30.0, 3600.0);
    Some(DeviceCode { user_code: code.into(), url: url.into(), interval_ms: interval * 1000.0, expires_ms: expires * 1000.0 })
}

/// The shell's `auth_poll` answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Poll {
    /// Not entered yet (`authorization_pending`); `slow` = Google asked to poll less often.
    Pending { slow: bool },
    Done { token: String, exp: f64 },
    Denied,
    Expired,
    Failed(String),
}

pub fn parse_poll(json: &str) -> Poll {
    let Some(v) = crate::json::parse_str(json) else { return Poll::Failed("bad answer".into()) };
    match v.get("state").and_then(|s| s.as_str()).unwrap_or("") {
        "pending" => Poll::Pending { slow: v.get("slow").and_then(|s| s.as_bool()).unwrap_or(false) },
        "done" => match (v.get("token").and_then(|t| t.as_str()), v.get("exp").and_then(|e| e.as_f64())) {
            (Some(t), Some(e)) if valid_token(t) => Poll::Done { token: t.into(), exp: e },
            _ => Poll::Failed("bad token".into()),
        },
        "denied" => Poll::Denied,
        "expired" => Poll::Expired,
        _ => Poll::Failed(v.get("error").and_then(|e| e.as_str()).unwrap_or("unknown").chars().take(120).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// A 4-column card grid (200×180 cards, 20 px gaps) under a header row.
    fn grid() -> Vec<Rect> {
        let mut v = vec![r(20.0, 10.0, 100.0, 40.0), r(400.0, 10.0, 300.0, 40.0)];
        for row in 0..3 {
            for col in 0..4 {
                v.push(r(100.0 + col as f64 * 220.0, 80.0 + row as f64 * 200.0, 200.0, 180.0));
            }
        }
        v
    }

    #[test]
    fn grid_walks_like_a_launcher() {
        let g = grid();
        // Card (row 0, col 1) = index 3.
        let cur = g[3];
        assert_eq!(pick(cur, &g, Dir::Right, Some(3)), Some(4));
        assert_eq!(pick(cur, &g, Dir::Left, Some(3)), Some(2));
        assert_eq!(pick(cur, &g, Dir::Down, Some(3)), Some(7), "the card below, not a diagonal one");
        assert_eq!(pick(cur, &g, Dir::Up, Some(3)), Some(1), "the search box above it (overlaps its column)");
        // Last column: nothing further right.
        assert_eq!(pick(g[5], &g, Dir::Right, Some(5)), None);
        // Bottom row: nothing below.
        assert_eq!(pick(g[13], &g, Dir::Down, Some(13)), None);
        // Up from the first column reaches the logo.
        assert_eq!(pick(g[2], &g, Dir::Up, Some(2)), Some(0));
    }

    #[test]
    fn sidebar_and_grid() {
        // Sidebar links at the left edge, grid to the right.
        let mut v: Vec<Rect> = (0..6).map(|i| r(0.0, 80.0 + i as f64 * 44.0, 80.0, 40.0)).collect();
        v.push(r(100.0, 80.0, 200.0, 180.0));
        v.push(r(100.0, 280.0, 200.0, 180.0));
        // Left from a card in the second row goes to the sidebar link at its height.
        let i = pick(v[7], &v, Dir::Left, Some(7)).unwrap();
        assert!(i < 6 && v[i].y >= 280.0 - 44.0, "picked {i}");
        // Right from a link goes to the card beside it.
        assert_eq!(pick(v[0], &v, Dir::Right, Some(0)), Some(6));
        // Down walks the links.
        assert_eq!(pick(v[0], &v, Dir::Down, Some(0)), Some(1));
    }

    #[test]
    fn zero_size_and_self_skipped() {
        let v = vec![r(0.0, 0.0, 10.0, 10.0), r(20.0, 0.0, 0.0, 0.0), r(40.0, 0.0, 10.0, 10.0)];
        assert_eq!(pick(v[0], &v, Dir::Right, Some(0)), Some(2));
    }

    #[test]
    fn requests_switch_to_the_tv_client() {
        let cfg = Config::default();
        let web = crate::api::Req::Browse { id: "FEsubscriptions".into(), params: None }.body(&cfg, "de", "/feed/subscriptions");
        let tok = "ya29.a0AfB_byC-test_token-0123456789";
        let auth = Auth { bearer: Some(tok.into()), ..Auth::default() };
        let (b, h) = finish(&cfg, &auth, web.clone(), 1);
        let b: Value = serde_json::from_str(&b).unwrap();
        let h: Value = serde_json::from_str(&h).unwrap();
        assert_eq!(b["context"]["client"]["clientName"], "TVHTML5");
        assert_eq!(b["context"]["client"]["clientVersion"], TV_VERSION);
        assert_eq!(b["context"]["client"]["hl"], "de");
        assert_eq!(b["browseId"], "FEsubscriptions");
        assert_eq!(h["Authorization"], format!("Bearer {tok}"));
        assert_eq!(h["X-Youtube-Client-Name"], "7");
        assert!(h.get("X-Goog-AuthUser").is_none() && h.get("X-Goog-Visitor-Id").is_none());
        // No token: the WEB request is untouched.
        let (b2, h2) = finish(&cfg, &Auth::default(), web.clone(), 1);
        assert_eq!(b2, web);
        assert!(!h2.contains("Authorization"));
        assert!(account_browse("FEwhat_to_watch") && account_browse("VLWL") && account_browse("VLLL"));
        assert!(!account_browse("UCabcdefghijklmnopqrstuv"));
        assert_eq!(ping_headers(&auth), format!("{{\"Authorization\":\"Bearer {tok}\"}}"));
        assert_eq!(ping_headers(&Auth::default()), "{}");
    }

    #[test]
    fn tokens_and_answers() {
        assert!(valid_token("ya29.a0AfB_byC-abc.def_ghi-0123456789"));
        assert!(!valid_token("ya29.\"><script>"));
        assert!(!valid_token("short"));
        assert_eq!(parse_token(r#"{"token":"ya29.a0AfB_byC-abc.def_ghi-0123456789","exp":5000}"#), Some(("ya29.a0AfB_byC-abc.def_ghi-0123456789".into(), 5000.0)));
        assert_eq!(parse_token(r#"{"token":null}"#), None);
        set_bearer(Some(("ya29.a0AfB_byC-abc.def_ghi-0123456789".into(), 5000.0)));
        assert!(bearer_at(4999.0).is_some() && bearer_at(5000.0).is_none());
        assert!(auth_at(1.0).signed_in() && !auth_at(6000.0).signed_in());
        set_bearer(None);
        // Recorded 2 Nov 2026 (shape) from POST /o/oauth2/device/code via the shell.
        let d = parse_device(r#"{"user_code":"TBX-NZG-FMQW","verification_url":"https://www.google.com/device","expires_in":1800,"interval":5}"#).unwrap();
        assert_eq!((d.user_code.as_str(), d.url.as_str(), d.interval_ms, d.expires_ms), ("TBX-NZG-FMQW", "https://www.google.com/device", 5000.0, 1_800_000.0));
        assert_eq!(parse_device(r#"{"user_code":"<b>","verification_url":"https://x"}"#), None);
        assert_eq!(parse_device(r#"{"user_code":"ABC-DEF","verification_url":"javascript:alert(1)"}"#), None);
        assert_eq!(parse_poll(r#"{"state":"pending"}"#), Poll::Pending { slow: false });
        assert_eq!(parse_poll(r#"{"state":"pending","slow":true}"#), Poll::Pending { slow: true });
        assert_eq!(parse_poll(r#"{"state":"done","token":"ya29.a0AfB_byC-abc.def_ghi-0123456789","exp":9}"#), Poll::Done { token: "ya29.a0AfB_byC-abc.def_ghi-0123456789".into(), exp: 9.0 });
        assert_eq!(parse_poll(r#"{"state":"done","token":"x","exp":9}"#), Poll::Failed("bad token".into()));
        assert_eq!(parse_poll(r#"{"state":"denied"}"#), Poll::Denied);
        assert_eq!(parse_poll("nope"), Poll::Failed("bad answer".into()));
    }

    #[test]
    fn remote_keys() {
        assert_eq!(remote("MediaPlayPause"), Some(Remote::PlayPause));
        assert_eq!(remote_player_key(Remote::Forward, false), Some("l"));
        assert_eq!(remote_player_key(Remote::Rewind, true), Some("j"));
        assert_eq!(remote_player_key(Remote::Play, false), None, "already playing");
        assert_eq!(remote_player_key(Remote::Pause, false), Some("k"));
        assert_eq!(remote("a"), None);
        assert_eq!(dir("ArrowLeft"), Some(Dir::Left));
    }
}
