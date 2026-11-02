//! TV mode wiring (`crate::tv` holds the pure parts). Only active when the TV app's shell set the
//! platform flag; on the desktop extension every entry point returns at once.
//!
//! - Remote: the arrows move focus spatially (`tv::pick` over the focusable elements the bridge
//!   measures, inside an open panel when one is open); on watch pages the player itself is a focus
//!   stop: ←/→ seek 10 s, OK plays/pauses, ↓ goes into the control bar, ↑ from the bar comes back.
//!   Media keys play/pause and seek. Back (the shell's button, `on_back`) closes what is open, then
//!   goes back; on Home it first returns to the top, then lets the app exit.
//! - Focus is never lost: after every paint the focus goes to the first card (feeds) or the player
//!   (watch) when nothing has it.
//! - Account: the device sign-in panel (`#ytc-tvsign`) and the access token from the shell, mirrored
//!   in this tab's sessionStorage; a cold start holds the first page until the shell answered.

use super::{App, Timer};
use crate::api::Req;
use crate::bridge;
use crate::render::{LIST, MAIN};
use crate::route::Route;
use crate::tv::{self, DeviceCode, Dir, Poll};
use std::collections::HashMap;

/// Focus stops for the remote (visible ones only; the bridge filters by visibility).
const SEL: &str = "a[href]:not([tabindex='-1']),button:not([disabled]):not([tabindex='-1']),input:not([type=hidden]):not([tabindex='-1']),select,textarea,[tabindex='0']";
/// Not stops on TV: the extra links/buttons inside a card (the card is one stop: its title link),
/// the player's inside (reached through the player itself), the mic (no voice search on TV) and
/// the search clear button.
const SKIP: &str = ".c a:not(.t),.c button,#ytc-player *,#ytc-mic,.qx,#ytc-debug *";
/// Panels that keep the remote inside while open (the innermost one around the focus wins).
const TRAPS: &str = "#ytc-tvsign.on,#ytc-keys.on,#ytc-imp.on,#ytc-player.menu #ytc-menu,.open[id^=ytc-fx-],#ytc-save-p.on,#ytc-share-p.on";
/// Something is open that Escape closes (Back then means Escape).
const OPEN: &str = "#ytc-tvsign.on,#ytc-keys.on,#ytc-imp.on,#ytc-player.menu #ytc-menu,.open[id^=ytc-fx-],#ytc [aria-expanded=true],#ytc-chap.open";
const PLAYER: &str = "ytc-player";
const PCTL: &str = "#ytc-player .pctl";
/// Hold the first page this long for the shell's token at a cold start, then go on signed out.
const HOLD_MS: u32 = 4000;
pub(super) const SIGN: &str = "ytc-tvsign";
const ACCT: &str = "ytc-acct";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Call {
    /// `auth_token`; `boot`: the first page waits for it.
    Token { boot: bool },
    Start,
    Poll { generation: u32 },
    SignOut,
}

#[derive(Default)]
pub(super) struct Tv {
    /// The first page request waits for the shell's token (cold start).
    pub(super) hold: bool,
    seq: u32,
    calls: HashMap<u32, Call>,
    code: Option<DeviceCode>,
    /// Device sign-in generation (a new code or a closed panel ends the old polling).
    generation: u32,
    poll_ms: f64,
    asked_at: f64,
    open: bool,
    token_asked: bool,
    /// The link last opened from each page (page URL → href): coming back focuses it again, as
    /// YouTube's TV app does.
    opened: HashMap<String, String>,
}

impl Tv {
    pub(super) fn new(hold: bool) -> Tv {
        Tv { hold, ..Default::default() }
    }
}

impl App {
    /// At boot, before the first page request. Returns true when the first page must wait for the
    /// shell's token (`tv_release` starts it).
    pub(super) fn tv_boot_flag() -> bool {
        let on = bridge::platform() == "tv";
        tv::set_on(on);
        if !on {
            return false;
        }
        bridge::root_attr("data-ytc-tv", Some("1".into()));
        let mirror = bridge::session_get(tv::SESSION_KEY);
        if mirror.is_empty() {
            return true;
        }
        tv::set_bearer(tv::parse_token(&mirror));
        false
    }

    /// After the document is written.
    pub(super) fn tv_boot(&mut self) {
        if !tv::on() {
            return;
        }
        bridge::back_hook();
        bridge::append("ytc", &format!("<div id=\"{SIGN}\" class=\"tvs\" role=\"dialog\" aria-modal=\"true\" aria-label=\"Sign in\"></div>"));
        self.tv_slot();
        self.tv_ask_token(self.tv.hold);
        if self.tv.hold {
            let id = self.timer(Timer::TvHold);
            bridge::timer(id, HOLD_MS);
        }
    }

    fn tv_call(&mut self, cmd: &str, arg: &str, what: Call) {
        self.tv.seq += 1;
        let seq = self.tv.seq;
        self.tv.calls.insert(seq, what);
        bridge::shell(seq, cmd, arg);
    }

    fn tv_ask_token(&mut self, boot: bool) {
        if self.tv.token_asked {
            return;
        }
        self.tv.token_asked = true;
        self.tv.asked_at = bridge::epoch();
        self.tv_call("auth_token", "{}", Call::Token { boot });
    }

    /// The held first page goes out (token answered, or the hold ran out).
    pub(super) fn tv_release(&mut self) {
        if std::mem::take(&mut self.tv.hold) {
            self.tv_slot();
            self.start_route();
        }
    }

    /// Account requests that go out with the bearer as the TV client.
    pub(super) fn tv_account_req(&self, req: &Req) -> bool {
        match req {
            Req::Browse { id, .. } => tv::account_browse(id),
            Req::Cont { ep: "browse", .. } => matches!(self.route, Route::Home | Route::Subs | Route::History | Route::Playlists | Route::Playlist { .. }),
            _ => false,
        }
    }

    /// Background tick: fetch a fresh access token before the current one runs out.
    pub(super) fn tv_tick(&mut self) {
        if !tv::on() {
            return;
        }
        let exp = tv::bearer_exp();
        if exp > 0.0 && exp - bridge::epoch() < tv::REFRESH_BEFORE_MS && bridge::epoch() - self.tv.asked_at > 30_000.0 {
            self.tv_ask_token(false);
        }
    }

    pub fn on_shell(&mut self, seq: u32, ok: bool, json: &str) {
        let Some(call) = self.tv.calls.remove(&seq) else { return };
        match call {
            Call::Token { boot: _ } => {
                self.tv.token_asked = false;
                let was = tv::bearer_at(bridge::epoch()).is_some();
                let tok = if ok { tv::parse_token(json) } else { None };
                match &tok {
                    Some((t, exp)) => {
                        bridge::session_set(tv::SESSION_KEY, &tv::token_json(t, *exp));
                    }
                    None if ok => {
                        bridge::session_set(tv::SESSION_KEY, "{\"token\":null}");
                    }
                    None => {}
                }
                if ok || tok.is_some() {
                    tv::set_bearer(tok);
                }
                let now = tv::bearer_at(bridge::epoch()).is_some();
                if self.tv.hold {
                    self.tv_release();
                } else if was != now {
                    self.tv_slot();
                }
            }
            Call::Start => {
                let code = if ok { tv::parse_device(json) } else { None };
                match code {
                    Some(c) => {
                        self.tv.poll_ms = c.interval_ms;
                        self.tv.code = Some(c);
                        self.tv.generation += 1;
                        self.tv_panel("");
                        self.tv_poll_later();
                    }
                    None => self.tv_panel(&format!("Couldn't get a sign-in code: {}", short(json))),
                }
            }
            Call::Poll { generation } => {
                if generation != self.tv.generation || !self.tv.open {
                    return;
                }
                match if ok { tv::parse_poll(json) } else { Poll::Failed(short(json)) } {
                    Poll::Pending { slow } => {
                        if slow {
                            self.tv.poll_ms += 5000.0;
                        }
                        self.tv_poll_later();
                    }
                    Poll::Done { token, exp } => {
                        tv::set_bearer(Some((token.clone(), exp)));
                        bridge::session_set(tv::SESSION_KEY, &tv::token_json(&token, exp));
                        self.tv.open = false;
                        self.tv.code = None;
                        // The whole client starts again signed in (snapshots of the signed-out
                        // pages are dropped with the tab's session data).
                        bridge::session_clear();
                        bridge::session_set(tv::SESSION_KEY, &tv::token_json(&token, exp));
                        bridge::replace_doc("/");
                    }
                    Poll::Denied => {
                        self.tv.code = None;
                        self.tv_panel("The sign-in was declined on the other device.");
                    }
                    Poll::Expired => {
                        self.tv.code = None;
                        self.tv_panel("That code has expired.");
                    }
                    Poll::Failed(why) => {
                        // A network hiccup: keep polling, show why.
                        self.tv_panel(&format!("Still waiting ({why})"));
                        self.tv_poll_later();
                    }
                }
            }
            Call::SignOut => {
                tv::set_bearer(None);
                bridge::session_clear();
                bridge::replace_doc("/");
            }
        }
    }

    fn tv_poll_later(&mut self) {
        let id = self.timer(Timer::TvPoll { generation: self.tv.generation });
        bridge::timer(id, self.tv.poll_ms.clamp(1000.0, 60_000.0) as u32);
    }

    pub(super) fn tv_poll(&mut self, generation: u32) {
        if generation == self.tv.generation && self.tv.open && self.tv.code.is_some() {
            self.tv_call("auth_poll", "{}", Call::Poll { generation });
        }
    }

    /// The account control in the top bar.
    fn tv_slot(&mut self) {
        let signed = tv::bearer_at(bridge::epoch()).is_some();
        bridge::html(ACCT, &crate::render::tv_slot(signed));
    }

    /// Writes the sign-in panel (`status`: a line under the code).
    fn tv_panel(&mut self, status: &str) {
        self.tv.open = true;
        let signed = tv::bearer_at(bridge::epoch()).is_some();
        bridge::html(SIGN, &crate::render::tv_panel(self.tv.code.as_ref().map(|c| (c.user_code.as_str(), c.url.as_str())), signed, status));
        bridge::attr(SIGN, "class", "tvs on");
        bridge::focus_sel(SIGN, "button");
    }

    fn tv_close(&mut self) {
        self.tv.open = false;
        self.tv.generation += 1;
        self.tv.code = None;
        bridge::attr(SIGN, "class", "tvs");
        bridge::focus_sel(ACCT, "button");
    }

    /// `tv-*` clicks.
    pub(super) fn tv_click(&mut self, act: &str) -> bool {
        match act {
            "tv-signin" | "tv-newcode" => {
                self.tv.code = None;
                self.tv.generation += 1;
                self.tv_panel("Getting a code…");
                self.tv_call("auth_start", "{}", Call::Start);
            }
            "tv-account" => self.tv_panel(""),
            "tv-signout" => {
                self.tv_panel("Signing out…");
                self.tv_call("auth_signout", "{}", Call::SignOut);
            }
            "tv-close" => self.tv_close(),
            _ => return false,
        }
        true
    }

    /// The platform Back button: true = handled here (else the shell goes back or leaves the app).
    pub fn on_back(&mut self) -> bool {
        if self.tv.open {
            self.tv_close();
            return true;
        }
        if bridge::any(OPEN) {
            self.on_key("Escape", false);
            return true;
        }
        // Inside the player's control bar: back to the player.
        if matches!(self.route, Route::Watch { .. }) && bridge::any(&format!("{PCTL} :focus")) {
            bridge::focus(PLAYER);
            return true;
        }
        if matches!(self.route, Route::Home) {
            // Home: first back to the top (focus on the first card), then the app may close.
            if bridge::scroll_y() > 4.0 || !bridge::any("#ytc-side :focus") {
                bridge::scroll_to(0.0);
                bridge::focus_sel("ytc-side", "a[data-nav=home]");
                return true;
            }
            return false;
        }
        false
    }

    /// TV keys (before the desktop handling). Some(handled) ends the key here.
    pub(super) fn tv_key(&mut self, key: &str, editable: bool) -> Option<bool> {
        if !tv::on() {
            return None;
        }
        let watch = matches!(self.route, Route::Watch { .. });
        if let Some(r) = tv::remote(key) {
            if !watch {
                return Some(false);
            }
            let paused = self.player.as_ref().is_none_or(|p| p.paused());
            let k = tv::remote_player_key(r, paused)?;
            return Some(self.on_key(k, false));
        }
        let d = tv::dir(key);
        if editable {
            // The search field: ↑/↓ leave it unless its suggestion list is open (that list owns them).
            return match d {
                Some(Dir::Up | Dir::Down) if !bridge::any("#ytc-q[aria-expanded=true]") => Some(self.tv_move(d.unwrap_or(Dir::Down))),
                _ => None,
            };
        }
        if watch {
            // A player menu opened from the remote owns the arrows (the player walks its rows).
            if bridge::any("#ytc-player.menu #ytc-menu") {
                return None;
            }
            let on_player = bridge::any("#ytc-player:focus") || !bridge::any(":focus");
            let in_bar = bridge::any(&format!("{PCTL} :focus"));
            if on_player {
                return match (key, d) {
                    ("Enter", _) | (" ", _) => {
                        bridge::focus(PLAYER);
                        Some(self.on_key("k", false))
                    }
                    (_, Some(Dir::Left)) => Some(self.on_key("j", false)),
                    (_, Some(Dir::Right)) => Some(self.on_key("l", false)),
                    (_, Some(Dir::Down)) => {
                        // Into the control bar at its play button (focus inside keeps the bar shown).
                        bridge::focus_sel(PLAYER, ".prow .pb");
                        Some(true)
                    }
                    (_, Some(Dir::Up)) => Some(self.tv_move(Dir::Up)),
                    _ => None,
                };
            }
            if in_bar {
                if let Some(d) = d {
                    if self.tv_move_in(PCTL, d) {
                        return Some(true);
                    }
                    return Some(match d {
                        Dir::Up => {
                            bridge::focus(PLAYER);
                            true
                        }
                        Dir::Down => self.tv_move(Dir::Down),
                        _ => true,
                    });
                }
                return None;
            }
        }
        d.map(|d| self.tv_move(d))
    }

    /// Moves focus inside `scope` only; false when nothing lies that way.
    fn tv_move_in(&mut self, scope: &str, d: Dir) -> bool {
        let flat = bridge::spatial_rects(scope, SEL, "");
        let Some((rects, cur)) = split(&flat) else { return false };
        let Some(c) = cur else { return false };
        match tv::pick(rects[c], &rects, d, Some(c)) {
            Some(i) => {
                bridge::spatial_focus(i as u32);
                true
            }
            None => false,
        }
    }

    /// Moves focus in direction `d` (inside an open panel when there is one).
    fn tv_move(&mut self, d: Dir) -> bool {
        let flat = bridge::spatial_rects(TRAPS, SEL, SKIP);
        let Some((rects, cur)) = split(&flat) else {
            self.tv_focus_default();
            return true;
        };
        let Some(c) = cur else {
            self.tv_focus_default();
            return true;
        };
        match tv::pick(rects[c], &rects, d, Some(c)) {
            Some(i) => bridge::spatial_focus(i as u32),
            None => {
                if d == Dir::Up {
                    bridge::scroll_to(0.0);
                }
            }
        }
        true
    }

    /// A link was opened (remote or pointer): remembered for coming back to this page.
    pub(super) fn tv_opened(&mut self, href: &str) {
        if tv::on() && !href.is_empty() && !href.contains(['"', '\\']) {
            if self.tv.opened.len() > 64 {
                self.tv.opened.clear();
            }
            self.tv.opened.insert(self.url.clone(), href.to_string());
        }
    }

    /// Nothing focused: the player on watch pages, else the link last opened from this page, else
    /// the first card in view, else the first stop.
    pub(super) fn tv_focus_default(&mut self) {
        if matches!(self.route, Route::Watch { .. }) {
            bridge::attr(PLAYER, "tabindex", "0");
            bridge::focus(PLAYER);
            return;
        }
        if let Some(h) = self.tv.opened.get(&self.url) {
            bridge::focus_sel(MAIN, &format!("a.t[href=\"{h}\"]"));
            if bridge::any(&format!("{} :focus", "#ytc-main")) {
                return;
            }
        }
        if !bridge::focus_grid(LIST, "a.t", 0) {
            bridge::focus_sel(MAIN, "a[href],button");
        }
    }

    /// After a page paint: keep a focus on screen (never lost).
    pub(super) fn tv_after_paint(&mut self) {
        if !tv::on() {
            return;
        }
        if matches!(self.route, Route::Watch { .. }) {
            bridge::attr(PLAYER, "tabindex", "0");
        }
        self.tv_focus_check();
        // The page that was left may still drop the focus while it is torn down (the player): look
        // again once that has run.
        let id = self.timer(Timer::TvFocus);
        bridge::timer(id, 300);
    }

    /// Focus is never lost on TV: nothing focused → the default stop.
    pub(super) fn tv_focus_check(&mut self) {
        if tv::on() && (!bridge::any(":focus") || bridge::any("body:focus")) {
            self.tv_focus_default();
        }
    }
}

/// `[x, y, w, h]*n, current` → rects and the focused index.
fn split(flat: &[f64]) -> Option<(Vec<tv::Rect>, Option<usize>)> {
    if flat.len() < 5 {
        return None;
    }
    let n = (flat.len() - 1) / 4;
    let cur = flat[flat.len() - 1];
    let cur = (cur >= 0.0 && (cur as usize) < n).then_some(cur as usize);
    Some((tv::rects(&flat[..n * 4]), cur))
}

fn short(json: &str) -> String {
    let s = crate::json::parse_str(json).and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| json.to_string());
    s.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_reads_the_bridge_answer() {
        let (r, c) = super::split(&[0.0, 0.0, 10.0, 10.0, 20.0, 0.0, 10.0, 10.0, 1.0]).unwrap();
        assert_eq!((r.len(), c), (2, Some(1)));
        let (_, c) = super::split(&[0.0, 0.0, 10.0, 10.0, -1.0]).unwrap();
        assert_eq!(c, None);
        assert!(super::split(&[-1.0]).is_none());
    }
}
