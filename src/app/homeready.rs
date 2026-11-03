//! Home reload: YouTube's own Home, as fast as possible (`crate::ready` holds the rules). The same
//! on the TV app: there every request here goes through `App::send`, which makes the account's Home
//! (ready response, hedge) a TVHTML5 request with the access token (`tv::finish`).
//!
//! - **The request** (browser extension): YouTube's own Home document, as a reload of its page asks
//!   for it (`crate::homedoc`; `App::send` turns the Home `browse` into it). A document without Home
//!   data (a consent or sign-in page, a changed format) or another session index (`au`): the `browse`
//!   request, for the rest of this document. The TV app keeps the TV client's `browse` (YouTube's own TV
//!   app has no server-rendered Home).
//! - **Shared by the tabs** (extension): the ready response is one per browser in the memory-only
//!   `chrome.storage.session` (`ready::shared`); a document's first Home takes it from there (one
//!   asynchronous read, a few ms), every tab keeps the current one in memory (`on_shared_change`), and a
//!   tab that uses it, or a personalizing event anywhere, removes it for all.
//!
//! - **Ready response.** After a Home paint, in idle time and at low priority, the Home request is
//!   made once more and its answer (first page + first continuation) is kept for the next reload or
//!   logo-Home, which paints it on its first frame and uses it up. After a personalizing event
//!   (`ready::taint`) or expiry it is refilled lazily: on a page other than a watch page, in idle
//!   time, at most once a minute, only while the tab is in use (input in the last 5 min), never
//!   while it is hidden (at once when it is shown again).
//! - **Network path** (no usable ready response): the skeleton in the final geometry at once (never
//!   the old Home first), the request now, and a second identical request (the hedge) when the
//!   first has not answered by this tab's p90 (`ready::hedge_ms`); the first answer wins and the
//!   other is aborted. After `ready::SLOW_MS` without an answer, or when the network is down, the
//!   tab's old Home shows dimmed ("Updating…") and is replaced when YouTube answers; a dropped
//!   connection is probed every 3 s and the page continues by itself when it is back.

use super::{App, Purpose, Timer, MAIN};
use crate::api::Req;
use crate::bridge;
use crate::homedoc;
use crate::features::home_chips as hc;
use crate::json::{self, Value};
use crate::parse::{self, Feed};
use crate::ready::{self, Ready};
use crate::route::Route;
use serde_json::json;

/// Lazy refills (after a personalizing event or expiry) at most this often.
const LAZY_MIN_MS: f64 = 60_000.0;
/// A refill waits for an idle main thread at most this long.
const FILL_IDLE_MS: u32 = 2000;
/// The dimmed old Home while YouTube has not answered (client.css `#ytc-main[data-upd]`).
const UPD: &str = "data-upd";

#[derive(Default)]
pub(super) struct HomeReady {
    /// The ready response (also in this tab's sessionStorage) and its taint generation.
    pub(super) slot: Option<(Ready, u32)>,
    /// The refill in flight: its request id (page, then its continuation).
    fill: Option<u32>,
    /// When the last lazy refill went out (performance.now ms).
    last_lazy: f64,
    /// A refill is scheduled (`Timer::ReadyFill`).
    scheduled: bool,
    /// The current Home navigation's request(s): (navigation, first id, hedge id, when each went out).
    hedge: Option<(u32, u32, Option<u32>, f64, f64)>,
    /// The Home request failed on a dropped connection: probing until it is back.
    pub(super) offline: bool,
    /// This document's first Home waits for the shared ready response (`bridge::shared_take`).
    pub(super) taking: bool,
    /// The shared store has answered once (until then this tab does not know whether one is waiting).
    pub(super) known: bool,
    /// YouTube's Home document had no Home data: `browse` for the rest of this document.
    pub(super) doc_off: bool,
    /// The facts of the Home document whose data is being handled (its answer id for the "shown" signals),
    /// of a Home page prefetched (logo hover) and of a fresh Home held behind the New videos pill.
    pub(super) doc_meta: Option<homedoc::Meta>,
    pub(super) prefetch_meta: Option<homedoc::Meta>,
    pub(super) held_meta: Option<homedoc::Meta>,
    /// A refill waits until YouTube has this screen's first "shown" batch (so its answer knows them).
    pub(super) fill_wait: bool,
    /// The country YouTube placed this browser in (`INNERTUBE_CONTEXT_GL` of its Home document).
    pub(super) doc_gl: Option<String>,
}

impl App {
    fn ready_acct(&self) -> String {
        ready::acct(&crate::tv::auth(), self.cfg.auth_user, self.cfg.visitor.as_deref(), &self.hl)
    }

    /// The ready response, if it is still what YouTube would answer now (taken: shown once).
    pub(super) fn ready_take(&mut self) -> Option<Ready> {
        let shared = ready::shared();
        let r = match self.hr.slot.take() {
            Some((r, g)) if g == ready::generation() => Some(r),
            Some(_) => None,
            // (Extension: the tab's memory holds the shared one; a new document reads it in `boot`.)
            None if shared => None,
            None => ready::from_json(&bridge::session_get(ready::KEY)),
        };
        if shared {
            // Used here, so gone for every tab.
            if r.is_some() {
                bridge::shared_del(ready::KEY);
            }
        } else {
            bridge::session_del(ready::KEY);
            // (Used or not, it is gone: the TV shell's copy of it too.)
            crate::tv::ready_drop();
        }
        let r = r.filter(|r| ready::usable(r, &self.ready_acct(), bridge::epoch()));
        self.perf.hit("home-ready", r.is_some());
        r
    }

    /// A usable ready response is waiting (memory or this tab's copy). Extension: also while this
    /// document does not know yet (the shared store has not answered): no second one is fetched.
    pub(super) fn ready_waiting(&mut self) -> bool {
        let acct = self.ready_acct();
        let now = bridge::epoch();
        match &self.hr.slot {
            Some((r, g)) => *g == ready::generation() && ready::usable(r, &acct, now),
            None if ready::shared() => !self.hr.known,
            None => ready::from_json(&bridge::session_get(ready::KEY)).is_some_and(|r| ready::usable(&r, &acct, now)),
        }
    }

    /// Keeps a ready response for the next Home: the shared store (extension) or this tab's
    /// sessionStorage and the TV shell.
    fn ready_keep(&mut self, r: &Ready) {
        let rec = ready::to_json(r);
        if ready::shared() {
            bridge::shared_set(ready::KEY, &rec);
        } else {
            bridge::session_set(ready::KEY, &rec);
            crate::tv::ready_put(&rec);
        }
    }

    // --------------------------------------------------- shared by the tabs (extension)

    /// The shared store answered a read (`boot`): this document's first Home, or what is waiting.
    pub fn on_shared(&mut self, key: &str, value: &str) {
        if key != ready::KEY || !ready::shared() {
            return;
        }
        self.hr.known = true;
        let r = ready::from_json(value).filter(|r| ready::usable(r, &self.ready_acct(), bridge::epoch()));
        if !std::mem::take(&mut self.hr.taking) {
            if self.hr.slot.is_none() {
                self.hr.slot = r.map(|r| (r, ready::generation()));
            }
            return;
        }
        // This document's first Home (taken: gone for every tab).
        self.perf.hit("home-ready", r.is_some());
        if self.route == Route::Home && self.nav == 1 && !self.rendered {
            match r {
                Some(r) => self.ready_show(r),
                None => self.start_route(),
            }
        } else if let Some(r) = r {
            // (Navigated away within the few ms of the read: it stays for the next Home.)
            self.ready_keep(&r);
            self.hr.slot = Some((r, ready::generation()));
        }
    }

    /// Another tab (or this one) changed the shared store: the ready response it now holds, or a
    /// personalizing event elsewhere.
    pub fn on_shared_change(&mut self, key: &str, value: &str) {
        if !ready::shared() {
            return;
        }
        if key == ready::TAINT_KEY {
            if ready::taint_seen(value) {
                self.hr.slot = None;
            }
            return;
        }
        if key != ready::KEY || self.hr.taking {
            return;
        }
        self.hr.known = true;
        let acct = self.ready_acct();
        self.hr.slot = ready::from_json(value).filter(|r| ready::usable(r, &acct, bridge::epoch())).map(|r| (r, ready::generation()));
    }

    /// Paints a ready response as the Home page (its first page shown, the continuation preloaded).
    pub(super) fn ready_show(&mut self, r: Ready) {
        let age = (bridge::epoch() - r.at).max(0.0);
        crate::telemetry::nav_src("ready");
        crate::telemetry::nav_field("age_ms", json!(age.round()));
        bridge::mark_val("ytcHomeSrc", "ready");
        bridge::mark_val("ytcHomeAge", &format!("{age:.0}"));
        self.mark("request");
        self.mark("response");
        self.fx_home_chips(r.chips, r.all);
        self.acts.learn_bar(r.topbar);
        if r.logged_out.is_some() {
            self.logged_out = r.logged_out;
        }
        let mut items = r.items;
        let rest = items.split_off(r.first.min(items.len()));
        self.direct = true;
        self.show_home(Feed { items, cont: r.cont, ..Default::default() }, rest);
        self.imp_begin(r.eid, r.track);
    }

    /// A Home page from YouTube is on screen: the next one is fetched in idle time.
    pub(super) fn ready_after_home(&mut self) {
        self.ready_schedule(false);
    }

    /// Any other page painted, the background tick, the tab shown: refill if one is due.
    pub(super) fn ready_lazy(&mut self, now: bool) {
        if now {
            self.ready_fill(true);
        } else {
            self.ready_schedule(true);
        }
    }

    fn ready_schedule(&mut self, lazy: bool) {
        if self.hr.scheduled || self.hr.fill.is_some() {
            return;
        }
        self.hr.scheduled = true;
        let id = self.timer(Timer::ReadyFill { lazy });
        bridge::idle(id, FILL_IDLE_MS);
    }

    /// Timer: the refill's idle moment came.
    pub(super) fn ready_timer(&mut self, lazy: bool) {
        self.hr.scheduled = false;
        self.ready_fill(lazy);
    }

    /// The refill waited for the screen's first "shown" batch, which has landed.
    pub(super) fn ready_fill_now(&mut self) {
        self.ready_fill(false);
    }

    fn ready_fill(&mut self, lazy: bool) {
        // `ytc-test-noready` (localStorage, tests only): no ready responses.
        // TV: nothing ahead before the first page has its account (the token held at a cold start).
        if self.hr.fill.is_some() || self.tv.hold || !bridge::visible() || bridge::local_flag("ytc-test-noready") {
            return;
        }
        // Watching changes what YouTube recommends: nothing is fetched ahead on a watch page.
        if matches!(self.route, Route::Watch { .. } | Route::Unknown { .. }) || self.ready_waiting() {
            return;
        }
        let now = bridge::now();
        // Lazy refills: at most once a minute, and only while the tab is in use (an idle tab stops
        // asking after `BG_ACTIVE_MS`: no refetch loop for a page nobody looks at).
        if lazy && ((self.hr.last_lazy > 0.0 && now - self.hr.last_lazy < LAZY_MIN_MS) || now - self.last_input > super::BG_ACTIVE_MS) {
            return;
        }
        // The answer should know what this screen showed: after its first "shown" batch has landed.
        if self.imp_first_pending() {
            self.hr.fill_wait = true;
            return;
        }
        if lazy {
            self.hr.last_lazy = now;
        }
        let generation = ready::generation();
        let acct = self.ready_acct();
        let id = self.send(Req::Browse { id: "FEwhat_to_watch".into(), params: None }, Purpose::Ready { generation, acct }, true);
        self.hr.fill = Some(id);
    }

    /// The refill's page (or its continuation) answered.
    pub(super) fn ready_body(&mut self, id: u32, generation: u32, acct: String, cont: bool, status: u32, body: &[u8]) {
        if self.hr.fill == Some(id) {
            self.hr.fill = None;
        }
        let meta = self.hr.doc_meta.take();
        // Something personalizing happened while it travelled: not what YouTube would answer now (a
        // lazy refill follows).
        if generation != ready::generation() {
            self.ready_lazy(false);
            return;
        }
        if status != 200 {
            return;
        }
        let Some(j) = json::parse(body) else { return };
        if cont {
            let Some((r, _)) = self.hr.slot.as_mut().filter(|(r, g)| *g == generation && r.acct == acct) else { return };
            let f = parse::continuation(&j);
            let seen: std::collections::HashSet<String> = r.items.iter().map(|i| i.key().to_string()).collect();
            r.items.extend(f.items.into_iter().filter(|i| !seen.contains(i.key())));
            r.track.extend(f.track);
            r.cont = f.cont;
            let r = r.clone();
            self.ready_keep(&r);
            return;
        }
        let f = parse::browse(&j);
        if f.items.is_empty() {
            return;
        }
        let mut all = None;
        let chips = hc::parse(&j, &mut all).unwrap_or_default();
        let ctx = parse::ctx(&j);
        let logged_out = ctx.logged_out.or(meta.as_ref().and_then(|m| m.logged_in).map(|l| !l));
        let r = Ready {
            at: bridge::epoch(),
            acct: acct.clone(),
            first: f.items.len(),
            items: f.items,
            cont: f.cont.clone(),
            chips,
            all,
            topbar: crate::actions::notify::topbar(&j),
            logged_out,
            eid: meta.and_then(|m| m.event_id),
            track: f.track,
        };
        self.ready_keep(&r);
        self.hr.slot = Some((r, generation));
        // Its first continuation too (as the page would preload it), so a reload has both.
        if let Some(token) = f.cont {
            let id = self.send(Req::Cont { ep: "browse", token }, Purpose::ReadyCont { generation, acct }, true);
            self.hr.fill = Some(id);
        }
    }

    /// The tab was shown (the user is back): an expired or used-up ready response is refilled now.
    pub(super) fn ready_shown(&mut self) {
        self.last_input = bridge::now();
        self.ready_lazy(true);
    }

    // ------------------------------------------------------------- network path

    /// The Home request just went out (`id`): the hedge and the slow fallback are armed.
    pub(super) fn home_sent(&mut self, id: u32) {
        if self.route != Route::Home {
            return;
        }
        crate::telemetry::nav_field("path", json!("network"));
        bridge::mark_val("ytcHomeSrc", "network");
        bridge::mark_val("ytcHedge", "");
        self.hr.hedge = Some((self.nav, id, None, bridge::now(), 0.0));
        let times = ready::times_from(&bridge::session_get(ready::TIMES_KEY));
        // `ytc-test-nohedge` (localStorage, measurements only): no second request.
        if !bridge::local_flag("ytc-test-nohedge") {
            let t = self.timer(Timer::HomeHedge { nav: self.nav });
            bridge::timer(t, ready::hedge_ms(&times).round() as u32);
        }
        let t = self.timer(Timer::HomeSlow { nav: self.nav });
        bridge::timer(t, ready::SLOW_MS);
    }

    /// The first Home request is late: the identical request again; the first answer wins.
    pub(super) fn home_hedge(&mut self, nav: u32) {
        let Some((n, first, None, sent, _)) = self.hr.hedge else { return };
        if n != nav || nav != self.nav || !self.pending.contains_key(&first) || !self.home_waiting() {
            return;
        }
        let Some(req) = self.page_req(&self.route) else { return };
        let id = self.send(req, Purpose::Page { nav }, false);
        self.hr.hedge = Some((n, first, Some(id), sent, bridge::now()));
        crate::telemetry::nav_field("hedge_fired", json!(true));
        bridge::mark_val("ytcHedge", "fired");
    }

    /// Still waiting for YouTube's Home (not painted from it yet).
    fn home_waiting(&self) -> bool {
        self.route == Route::Home && (!self.rendered || self.swr)
    }

    /// A Home page request answered (`id`). False: drop this answer (the other request is still
    /// on its way after this one failed on the network).
    pub(super) fn home_answered(&mut self, id: u32, status: u32) -> bool {
        let Some((nav, first, second, sent, sent2)) = self.hr.hedge else { return true };
        if nav != self.nav || (id != first && second != Some(id)) {
            return true;
        }
        let other = if id == first { second } else { Some(first) };
        if status == 0 && other.is_some_and(|o| self.pending.contains_key(&o)) {
            // This one failed on the network; the other may still make it.
            let (keep, at) = if id == first { (second.unwrap_or(first), sent2) } else { (first, sent) };
            self.hr.hedge = Some((nav, keep, None, at, 0.0));
            return false;
        }
        self.hr.hedge = None;
        if let Some(o) = other {
            if self.pending.remove(&o).is_some() {
                bridge::abort(o);
            }
            let won = if id == first { "first" } else { "hedge" };
            crate::telemetry::nav_field("hedge_won", json!(won));
            bridge::mark_val("ytcHedge", won);
        }
        if status == 200 {
            // How long YouTube took to answer the request that won (its own time, so one stalled
            // request does not push later deadlines out).
            let ms = bridge::now() - if id == first { sent } else { sent2 };
            let t = ready::times_push(&bridge::session_get(ready::TIMES_KEY), ms);
            bridge::session_set(ready::TIMES_KEY, &t);
        }
        true
    }

    /// YouTube has not answered for `ready::SLOW_MS`: the tab's old Home, dimmed, until it does.
    pub(super) fn home_slow(&mut self, nav: u32) {
        if nav != self.nav || self.route != Route::Home || self.rendered {
            return;
        }
        self.home_stale_page();
    }

    fn home_stale_page(&mut self) {
        let Some(sn) = self.read_snap() else { return };
        let html = self.adopt_snap(sn, true);
        crate::telemetry::nav_src("snapshot");
        crate::telemetry::nav_field("path", json!("slow"));
        bridge::mark_val("ytcHomeSrc", "stale");
        bridge::html(MAIN, &html);
        bridge::attr(MAIN, UPD, "1");
        bridge::scroll_to(0.0);
        self.mark("repaint");
        // The fresh answer replaces it (or waits behind the New videos pill once scrolled).
        self.swr = true;
        self.after_paint();
    }

    /// The Home request failed on the network (both requests): the old Home dimmed if there is
    /// one (else the skeleton stays), a quiet "No connection" note, and the connection is probed
    /// until it is back; then the request goes out again by itself.
    pub(super) fn home_offline(&mut self) {
        if !self.rendered {
            self.home_stale_page();
        }
        bridge::attr(MAIN, UPD, "2");
        if !self.hr.offline {
            self.hr.offline = true;
            self.net_probe();
        }
    }

    /// A probe answered while Home waits for the connection: back → ask again; down → later.
    pub(super) fn home_probe(&mut self, status: u32) -> bool {
        if !self.hr.offline {
            return false;
        }
        if self.route != Route::Home {
            self.hr.offline = false;
            return true;
        }
        if status == 204 || status == 200 {
            self.hr.offline = false;
            bridge::attr(MAIN, UPD, "1");
            if let Some(req) = self.page_req(&Route::Home) {
                self.page_request(req);
            }
        } else {
            let id = self.timer(Timer::NetProbe);
            bridge::timer(id, super::NET_PROBE_MS);
        }
        true
    }

    /// The fresh Home replaced whatever was showing (or is held behind the New videos pill).
    pub(super) fn home_fresh_painted(&mut self) {
        bridge::attr(MAIN, UPD, "");
        self.hr.offline = false;
    }

    /// Home's own page markup for a parsed Home `browse` response (also used by `render_page`).
    pub(super) fn home_from_json(&mut self, json: &Value) {
        self.fx_home_response(json);
        let mut f = parse::browse(json);
        let track = std::mem::take(&mut f.track);
        let eid = self.hr.doc_meta.take().and_then(|m| m.event_id);
        self.show_home(f, Vec::new());
        self.imp_begin(eid, track);
    }

    // ------------------------------------------------------- YouTube's Home document

    /// The Home request goes out as YouTube's own Home document (`homedoc`): the extension, the first
    /// session index, while documents have had Home data.
    pub(super) fn doc_ok(&self) -> bool {
        !crate::tv::on() && self.cfg.auth_user == 0 && !self.hr.doc_off && !bridge::local_flag("ytc-test-nodoc")
    }

    /// The page's facts from a Home document: its visitor when the tab has none (a new tab needs no stored
    /// config for it) and YouTube's current web client version until `sw.js_data` has given one (that refresh
    /// keeps its own schedule).
    pub(super) fn doc_learn(&mut self, m: &homedoc::Meta) {
        let mut changed = m.visitor.as_deref().is_some_and(|v| self.cfg.learn_visitor(v));
        if let Some(v) = m.version.as_deref().filter(|v| *v != self.cfg.version && self.cfg.fetched_at <= 0.0) {
            self.cfg.version = v.to_string();
            changed = true;
        }
        if let Some(l) = m.logged_in {
            self.logged_out = Some(!l);
        }
        if m.gl.is_some() {
            self.hr.doc_gl = m.gl.clone();
        }
        if changed {
            self.save_cfg();
            self.kick_player();
        }
    }

    /// A Home document came without Home data (`status` 302: a redirect, e.g. a consent page): the
    /// `browse` request instead, now and for the rest of this document.
    pub(super) fn doc_failed(&mut self, id: u32, purpose: Purpose, status: u32) {
        self.hr.doc_off = true;
        self.incident("home-doc", &format!("YouTube's Home document had no Home data (HTTP {status}); using browse"), None);
        match purpose {
            Purpose::Page { nav } if nav == self.nav && self.route == Route::Home => {
                // Its hedge (the same document) goes too; the browse request gets its own.
                if let Some((_, first, second, _, _)) = self.hr.hedge.take() {
                    for o in [Some(first), second].into_iter().flatten() {
                        if o != id && self.pending.remove(&o).is_some() {
                            bridge::abort(o);
                        }
                    }
                }
                if let Some(req) = self.page_req(&Route::Home) {
                    self.page_request(req);
                }
            }
            Purpose::Ready { .. } => {
                if self.hr.fill == Some(id) {
                    self.hr.fill = None;
                }
                self.ready_fill(false);
            }
            Purpose::PagePrefetch { url, .. } => {
                self.page_prefetching.remove(&url);
                if self.await_page.as_deref() == Some(url.as_str()) {
                    self.await_page = None;
                    if let Some(req) = self.page_req(&self.route) {
                        self.page_request(req);
                    }
                }
            }
            _ => {}
        }
    }
}
