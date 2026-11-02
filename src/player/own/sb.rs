//! SponsorBlock glue for the player (`player::sponsor` holds the pure logic).
//! The lookup goes out with the player's own requests at start, at low
//! priority, and never holds anything back: the first frame, seeking and the
//! chapters do not wait for it, and a failure or a timeout leaves the video as
//! it was. Skips are checked on the player tick and on `timeupdate`, plus one
//! timer aimed at the exact start when a skip is closer than a tick.

use super::{Out, Own, Req, Tm, mv, ui};
use crate::bridge;
use crate::motion;
use crate::player::sponsor::{self, Cat, Seg};
use std::rc::Rc;

/// The toast stays this long.
const TOAST_MS: u32 = 5000;
/// A highlight further than this ahead is offered as a jump when the segments arrive.
const HINT_AHEAD_S: f64 = 2.0;

#[derive(Default)]
pub(super) struct SbSt {
    pub(super) segs: Rc<Vec<Seg>>,
    req: Option<u32>,
    /// Looked up (answer, cache, failure or timeout): no second request for this video.
    done: bool,
    status: String,
    /// Segments Undo exempted (indices into `segs`).
    undone: Vec<usize>,
    /// Where the user last seeked to (a segment containing it only shows).
    manual: Option<f64>,
    /// Our own skip seek (not a manual one).
    own: Option<f64>,
    /// The precise start timer is pending.
    armed: bool,
    /// What the toast's button undoes: the skipped span (start, end); None = the highlight Jump.
    undo: Option<(f64, f64)>,
    toast_gen: u32,
    toast_on: bool,
    /// Marks need (re)drawing on the seek bar; `drawn`: some are drawn now.
    pub(super) dirty: bool,
    pub(super) drawn: bool,
    /// The category the seek-bar tooltip shows.
    pub(super) tip: Option<Cat>,
    skips: u32,
}

#[derive(Clone, Copy)]
pub(super) enum SbTm {
    Timeout,
    /// The next skip starts within a tick.
    At,
    Toast(u32),
}

impl Own {
    /// Sends the k-anonymous lookup (or takes this tab's cached answer).
    pub(super) fn sb_lookup(&mut self) {
        if !self.prefs.sb_on || self.sb.done || self.sb.req.is_some() || self.failed {
            return;
        }
        if let Some(segs) = sponsor::cache_get(&self.v) {
            self.sb.done = true;
            self.sb_loaded(segs, "memory");
            return;
        }
        let Some(url) = sponsor::url(&self.v) else { return };
        // No cookies, no referrer (the watch URL), no HTTP cache (nothing on disk); low priority next to the streams.
        let id = self.get(Req::Sponsor, &url, r#"{"credentials":"omit","referrerPolicy":"no-referrer","cache":"no-store","priority":"low"}"#);
        self.sb.req = Some(id);
        self.sb.status = "asked".into();
        self.timer(Tm::Sb(SbTm::Timeout), sponsor::TIMEOUT_MS);
    }

    pub(super) fn on_sponsor(&mut self, status: u32, body: &[u8]) {
        self.sb.req = None;
        self.sb.done = true;
        // 404 = no video in this bucket has segments.
        let segs = match status {
            200 => sponsor::parse(body, &self.v),
            404 => Vec::new(),
            _ => {
                self.sb.status = format!("HTTP {status}: nothing skipped");
                return;
            }
        };
        let segs = Rc::new(segs);
        sponsor::cache_put(&self.v, segs.clone());
        self.sb_loaded(segs, "answer");
    }

    fn sb_loaded(&mut self, segs: Rc<Vec<Seg>>, from: &str) {
        self.note(format!("SponsorBlock: {} segment(s) ({from})", segs.len()));
        self.sb.status = format!("{} segment(s) from {from}", segs.len());
        self.sb.segs = segs;
        if self.sb.segs.is_empty() {
            return;
        }
        self.sb.dirty = true;
        if let Some(p) = sponsor::highlight(&self.sb.segs) {
            if self.live.is_none() && self.time() < p - HINT_AHEAD_S {
                self.sb_toast(None, format!("Highlight at {}", ui::clock(p)), Cat::Highlight);
            }
        }
        if self.mc.open == Some("sb") {
            self.render_menu();
        }
        self.sb_check();
        self.ui();
    }

    /// Ranges drawn on the seek bar (none while SponsorBlock is off).
    pub(super) fn sb_marks(&self) -> Rc<Vec<Seg>> {
        if self.prefs.sb_on { self.sb.segs.clone() } else { Rc::new(Vec::new()) }
    }

    /// Playback position check (tick, `timeupdate`, the precise timer): entering a chosen segment by
    /// playing seeks to its end.
    pub(super) fn sb_check(&mut self) {
        if !self.prefs.sb_on || self.sb.segs.is_empty() || self.live.is_some() || self.paused || self.drag.is_some() || self.first_frame.is_none() || self.failed {
            return;
        }
        let t = self.time();
        if let Some(m) = self.sb.manual {
            if !sponsor::keep_manual(&self.sb.segs, m, t) {
                self.sb.manual = None;
            }
        }
        if let Some((i, to)) = sponsor::skip_at(&self.sb.segs, self.prefs.sb_skip, &self.sb.undone, self.sb.manual, t) {
            let to = if self.duration > 0.0 { to.min(self.duration) } else { to };
            let s = self.sb.segs[i].clone();
            self.note(format!("SponsorBlock: skipped {} {t:.2} → {to:.2} s", s.cat.key()));
            self.sb.skips += 1;
            self.sb.own = Some(to);
            bridge::media_set(self.vid, "currentTime", to);
            self.snap = true;
            self.sb.undo = Some((s.start, to));
            self.sb_toast(Some((s.start, to)), sponsor::skipped_label(s.cat), s.cat);
            return;
        }
        if !self.sb.armed {
            let rate = bridge::media_get(self.vid, "playbackRate");
            if let Some(dt) = sponsor::until_next(&self.sb.segs, self.prefs.sb_skip, &self.sb.undone, t, rate) {
                if dt * 1000.0 < 300.0 {
                    self.sb.armed = true;
                    self.timer(Tm::Sb(SbTm::At), ((dt * 1000.0).ceil() as u32).max(1));
                }
            }
        }
    }

    /// A seek (the `seeking` event): ours is ignored, the user's marks where they went.
    pub(super) fn sb_seeking(&mut self) {
        let t = self.time();
        if let Some(to) = self.sb.own.take() {
            if (t - to).abs() < 0.3 {
                return;
            }
        }
        // The start position (resume, `t=`) is not a deliberate seek into a segment.
        if self.first_frame.is_some() {
            self.sb.manual = Some(t);
        }
    }

    pub(super) fn sb_timer(&mut self, k: SbTm) {
        match k {
            SbTm::Timeout => {
                if let Some(id) = self.sb.req.take() {
                    bridge::abort(id);
                    self.fetches.remove(&id);
                    self.sb.done = true;
                    self.sb.status = format!("no answer in {} s: nothing skipped", sponsor::TIMEOUT_MS / 1000);
                    self.note("SponsorBlock: no answer, nothing skipped".into());
                }
            }
            SbTm::At => {
                self.sb.armed = false;
                self.sb_check();
                self.ui();
            }
            SbTm::Toast(g) => {
                if g == self.sb.toast_gen {
                    self.sb_toast_hide();
                }
            }
        }
    }

    /// Slides the toast in from the player's left edge (a new message while it shows rises in place).
    fn sb_toast(&mut self, undo: Option<(f64, f64)>, label: String, cat: Cat) {
        if self.sb.toast_on && !bridge::reduced_motion() {
            bridge::nudge(ui::SBTL, 6.0 * mv(), motion::E_OUT, 220);
        }
        bridge::text(ui::SBTL, &label);
        bridge::text(ui::SBTB, if undo.is_some() { "Undo" } else { "Jump" });
        bridge::attr(ui::SBTB, "data-arg", if undo.is_some() { "undo" } else { "jump" });
        bridge::attr(ui::SBTB, "tabindex", "0");
        bridge::attr(ui::SBT, "class", &format!("psbt on k{}", cat.idx()));
        self.sb.undo = undo;
        self.sb.toast_on = true;
        self.sb.toast_gen = self.sb.toast_gen.wrapping_add(1);
        self.timer(Tm::Sb(SbTm::Toast(self.sb.toast_gen)), TOAST_MS);
    }

    fn sb_toast_hide(&mut self) {
        if self.sb.toast_on {
            bridge::class_all(&format!("#{}", ui::SBT), "on", false);
            bridge::attr(ui::SBTB, "tabindex", "-1");
            self.sb.toast_on = false;
        }
    }

    /// The toast's button: Undo (back to the skipped span's start; that span no longer auto-skips in this
    /// video) or Jump (to the highlight).
    pub(super) fn sb_act(&mut self, arg: &str) {
        match arg {
            "undo" => {
                let Some((a, b)) = self.sb.undo.take() else { return };
                for (i, s) in self.sb.segs.iter().enumerate() {
                    if !s.is_point() && s.start >= a - sponsor::TOL && s.start < b && !self.sb.undone.contains(&i) {
                        self.sb.undone.push(i);
                    }
                }
                self.sb_toast_hide();
                self.seek_to(a);
            }
            "jump" => {
                self.sb_toast_hide();
                if let Some(p) = sponsor::highlight(&self.sb.segs) {
                    self.seek_to(p);
                }
            }
            _ => {}
        }
    }

    /// Menu choice `sb:<category>` (auto-skip on/off) or `sb:toggle` (SponsorBlock on/off). Remembered.
    pub(super) fn sb_set(&mut self, v: &str) {
        if v == "toggle" {
            self.prefs.sb_on = !self.prefs.sb_on;
            self.sb.dirty = true;
            if self.prefs.sb_on {
                self.sb_lookup();
            } else {
                self.sb_toast_hide();
            }
        } else if let Some(c) = Cat::from_key(v).filter(|c| *c != Cat::Highlight) {
            self.prefs.sb_skip ^= c.bit();
            // Turned on while playing inside one: that one shows (it was not entered by an automatic skip).
            self.sb.manual = Some(self.time());
        } else {
            return;
        }
        self.out.push(Out::Prefs(self.prefs.clone()));
    }

    pub(super) fn sb_menu_html(&self) -> String {
        sponsor::menu_html(self.prefs.sb_on, self.prefs.sb_skip, &self.sb.segs)
    }

    /// A choice changes the open menu in place (class/attribute toggles).
    pub(super) fn sb_sync_menu(&self) {
        for c in sponsor::SKIPPABLE {
            let on = self.prefs.sb_skip & c.bit() != 0;
            let id = format!("ytc-m-sb:{}", c.key());
            bridge::attr(&id, "aria-pressed", if on { "true" } else { "false" });
            bridge::class_all(&format!("#{} [data-arg=\"sb:{}\"]", ui::MENU, c.key()), "on", on);
        }
        bridge::attr(sponsor::MENU_SW, "aria-checked", if self.prefs.sb_on { "true" } else { "false" });
    }

    /// The seek-bar tooltip's category label for time `t`.
    pub(super) fn sb_tip(&mut self, t: f64) {
        let k = if self.prefs.sb_on { sponsor::at(&self.sb.segs, t) } else { None };
        if k != self.sb.tip {
            bridge::text(ui::TIPK, k.map_or("", |c| c.label()));
            bridge::attr(ui::TIPK, "class", &k.map_or("ptk".into(), |c| format!("ptk k{}", c.idx())));
            self.sb.tip = k;
        }
    }

    pub(super) fn sb_debug(&self) -> String {
        format!(
            "       SponsorBlock {} · {} · skip {} · skipped {}{}\n",
            if self.prefs.sb_on { "on" } else { "off" },
            if self.sb.status.is_empty() { "not asked" } else { &self.sb.status },
            sponsor::mask_to_keys(self.prefs.sb_skip),
            self.sb.skips,
            if self.sb.undone.is_empty() { String::new() } else { format!(" · undone {}", self.sb.undone.len()) }
        )
    }
}
