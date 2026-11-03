//! Home's "shown" signals (`crate::impress` decides what is sent; here the page feeds it). Browser
//! extension only (YouTube's own TV app's signals were not captured: the TV app sends none).
//!
//! A Home answer from YouTube is painted (the request, the ready response, the New videos pill) → a new
//! screen; its cards are watched with an IntersectionObserver (`bridge::shown_watch`: the box comes with the
//! observer's entry, no layout read of ours, nothing in a hot path); a card ≥ half in view is logged once,
//! with its grid place; a scroll that comes to rest is logged; the events go out together `FLUSH_MS` after
//! the first one, at low priority (and with `keepalive` when the page is hidden or left).
//!
//! The next ready Home response waits for a screen's first batch (YouTube's next answer then knows what
//! was shown, as on its own page); a later batch with newly shown cards (scrolling) makes the waiting one
//! stale (`ready::taint`; refilled lazily).

use super::{App, Purpose, Timer};
use crate::util::next_id;
use crate::bridge;
use crate::impress::{self, BoxPx, Screen};
use crate::parse::Track;
use crate::route::Route;

/// The Home grid and its cards (`data-v` = the video id).
const LIST: &str = "ytc-list";
const CARDS: &str = ".c[data-v]";

#[derive(Default)]
pub(super) struct Imp {
    s: Screen,
    sched: bool,
    /// The screen's first batch is queued (not sent yet) or in flight (its request id).
    first: bool,
    first_id: Option<u32>,
}

impl App {
    fn imp_on(&self) -> bool {
        !crate::tv::on() && !bridge::local_flag("ytc-test-noimp")
    }

    fn imp_idle_ms(&self) -> f64 {
        bridge::now() - self.last_input.max(0.0)
    }

    /// A Home answer from YouTube is on screen (`eid`: its id in YouTube's logs; `track`: its cards').
    pub(super) fn imp_begin(&mut self, eid: Option<String>, track: Vec<Track>) {
        if !self.imp_on() || self.route != Route::Home {
            return;
        }
        let seed = format!("{}:{}:{}", bridge::epoch(), bridge::now(), eid.as_deref().unwrap_or(""));
        let csn = crate::player::stats::cpn(&seed);
        let idle = self.imp_idle_ms();
        self.imp.s.start(csn, eid, track, bridge::epoch(), idle);
        self.imp.first = true;
        self.imp.first_id = None;
        self.imp_schedule();
        bridge::shown_watch(LIST, CARDS, "v", impress::SHOWN_RATIO);
    }

    /// More cards of the screen (a continuation's): watched once they are written.
    pub(super) fn imp_more(&mut self, track: Vec<Track>) {
        if self.imp.s.active && self.route == Route::Home {
            self.imp.s.add(track);
        }
    }

    /// Cards were written into the grid: watch the new ones.
    pub(super) fn imp_scan(&mut self) {
        if self.imp.s.active && self.route == Route::Home {
            bridge::shown_scan();
        }
    }

    /// The screen is gone (another page): what is queued goes out now.
    pub(super) fn imp_end(&mut self) {
        if self.imp.s.active {
            self.imp.s.end();
            bridge::shown_stop();
        }
        self.imp_flush(false);
    }

    /// The screen's first batch has not reached YouTube yet (the next ready response waits for it).
    pub(super) fn imp_first_pending(&self) -> bool {
        self.imp.first && (self.imp.s.pending() || self.imp.first_id.is_some_and(|id| self.pending.contains_key(&id)))
    }

    /// A card came into view: `i` = its place among the grid's cards (document order).
    pub fn on_shown(&mut self, key: &str, i: u32, x: f64, y: f64, w: f64, h: f64) {
        if self.route != Route::Home || !self.imp.s.active {
            return;
        }
        let i = i as usize;
        let cols = self.cols.max(1);
        let idle = self.imp_idle_ms();
        if self.imp.s.shown(key, i % cols, i / cols, BoxPx { x, y, w, h }, bridge::epoch(), idle) {
            self.imp_schedule();
        }
    }

    pub fn on_scroll_end(&mut self, y: f64) {
        if self.route != Route::Home {
            return;
        }
        let idle = self.imp_idle_ms();
        if self.imp.s.scrolled(y, bridge::epoch(), idle) {
            self.imp_schedule();
        }
    }

    fn imp_schedule(&mut self) {
        if !self.imp.sched {
            self.imp.sched = true;
            let id = self.timer(Timer::ImpFlush);
            bridge::timer(id, impress::FLUSH_MS);
        }
    }

    /// Timer: the batch's time came.
    pub(super) fn imp_timer(&mut self) {
        self.imp.sched = false;
        self.imp_flush(false);
    }

    /// Sends what is queued (`keepalive`: the page is being hidden or left).
    pub(super) fn imp_flush(&mut self, keepalive: bool) {
        let vp = crate::api::Viewport::from_json(&bridge::viewport());
        let gl = self.hr.doc_gl.clone();
        loop {
            let client = impress::client(&self.cfg.version, &self.hl, gl.as_deref(), vp.w, vp.h);
            let Some((body, cards)) = self.imp.s.batch(client) else { break };
            let auth = crate::tv::auth();
            let h = crate::api::headers(&self.cfg, &auth, (bridge::epoch() / 1000.0) as u64);
            // gzip as YouTube's page sends them; a page being left sends at once (compressing is asynchronous).
            let opts = if keepalive { r#"{"priority":"low","keepalive":true}"# } else { r#"{"priority":"low","gzip":1}"# };
            let id = next_id();
            bridge::fetch_opts(id, "POST", impress::URL, &body, &h, opts);
            let first = self.imp.first && self.imp.first_id.is_none();
            if first {
                self.imp.first_id = Some(id);
            }
            self.pending.insert(id, super::Pending { purpose: Purpose::Imp { first }, sent: bridge::now(), ep: "log_event", unmasked: None });
            if cards > 0 && !first {
                // YouTube now knows of cards the waiting ready response was fetched without.
                crate::ready::taint();
                self.hr.slot = None;
                self.ready_lazy(false);
            }
        }
    }

    /// A batch answered.
    pub(super) fn imp_answered(&mut self, first: bool) {
        if first {
            self.imp.first = false;
            if std::mem::take(&mut self.hr.fill_wait) {
                self.ready_fill_now();
            }
        }
    }
}
