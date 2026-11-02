//! Navigation motion (the motion law, `.claude/skills/yourtube-motion`):
//! the thumbnail → player morph on watch open (pattern (c), FLIP), the theater
//! switch morph, the short settle of watch content that replaces a skeleton,
//! and the `data-still` gate that keeps CSS entrances (`@starting-style`) off
//! the first frame of keyboard and back/forward paints.
//!
//! Every page is written synchronously exactly as without motion; these calls
//! run after the writes, in the same task, and only animate paint.

use super::{App, Timer};
use crate::route::Route;
use crate::{bridge, motion, player, render};

/// Content that replaces a visible skeleton fades in this fast.
const SETTLE_MS: u32 = 180;
/// Reduced motion: the player fades in instead of morphing.
const PLAYER_FADE_MS: u32 = 200;
/// Pointer watch open: the title and channel row settle in under the growing player (rise + fade, staggered).
const INFO_SEL: &str = ".wt,.wo";
const INFO_STEP: u32 = 40;
const INFO_DY: f64 = 8.0;
const INFO_MS: u32 = 300;
/// `next` fills the page after a pointer open: the description's lines and the related cards settle in as a
/// short wave instead of one fade (≤ 6 lines, ≤ 8 cards; nothing waits, no layout read).
const DESC_SEL: &str = ".ws,.dr";
const DESC_MAX: u32 = 6;
const DESC_STEP: u32 = 28;
const DESC_DY: f64 = 6.0;
const REL_MAX: u32 = 8;
const REL_STEP: u32 = 30;
const REL_DY: f64 = 8.0;
const WAVE_MS: u32 = 300;
/// A pointer click's source for a morph: a card's thumbnail, or a playlist row's.
const SRC_SCOPE: &str = ".c,.pli";
const SRC_THUMB: &str = ".th,.plth";

#[derive(Default)]
pub(super) struct Motion {
    /// The next navigation comes from the keyboard (set by its caller, taken by `navigate`).
    pub kbd_nav: bool,
    /// What the current navigation paints in this task is instant (keyboard,
    /// back/forward): no settle fades. Cleared after its first frame.
    pub instant: bool,
    /// The current navigation started from the keyboard or back/forward (like `instant`, but for
    /// the whole navigation: content that arrives later makes no entrance either).
    pub calm: bool,
    /// The watch page painted a title skeleton (no card data): `next` replaces the whole info.
    pub info_sk: bool,
    /// The source thumbnail's viewport rect for a pending morph.
    src: Option<Vec<f64>>,
    /// A pointer click toggles theater mode: morph the player into its new box.
    pub theater: bool,
    /// Springs from `motion.rs`, computed once: soft (the morph), ui (theater).
    springs: Option<[(String, u32); 2]>,
}

impl Motion {
    fn springs(&mut self) -> &[(String, u32); 2] {
        self.springs.get_or_insert_with(|| [motion::soft(), motion::ui()])
    }
}

impl App {
    /// A click is about to navigate: pointer clicks on a video's card remember
    /// its thumbnail (one layout read, before the swap); keyboard ones are instant.
    pub(super) fn mo_click(&mut self, kbd: bool, route: &Route) {
        self.mo.kbd_nav = kbd;
        self.mo.src = None;
        if !kbd && matches!(route, Route::Watch { .. }) {
            let r = bridge::click_rect(SRC_SCOPE, SRC_THUMB);
            if r.len() == 4 {
                self.mo.src = Some(r);
            }
        }
    }

    /// Start of a navigation (before anything is written).
    pub(super) fn mo_nav(&mut self, pop: bool) {
        self.mo.instant = pop || std::mem::take(&mut self.mo.kbd_nav);
        self.mo.calm = self.mo.instant;
        self.mo.info_sk = false;
        if self.mo.instant {
            bridge::attr("ytc", "data-still", "1");
            let id = self.timer(Timer::Still { nav: self.nav });
            bridge::frame(id);
        }
    }

    /// The instant navigation's first frame has rendered.
    pub(super) fn mo_still_done(&mut self, nav: u32) {
        if nav == self.nav {
            self.mo.instant = false;
            bridge::attr("ytc", "data-still", "0");
        }
    }

    /// End of the click's task: the watch page is written and final. Paint the
    /// player from the clicked thumbnail to its place (a gentle fade under
    /// reduced motion). Nothing waits for it.
    pub(super) fn mo_flush(&mut self) {
        let Some(r) = self.mo.src.take() else { return };
        if self.player.is_none() || !matches!(self.route, Route::Watch { .. }) {
            return;
        }
        let reduced = bridge::reduced_motion();
        bridge::stagger(render::INFO, INFO_SEL, "", 2, INFO_STEP, if reduced { 0.0 } else { INFO_DY }, motion::E_OUT, INFO_MS);
        if reduced {
            bridge::fade_in(player::ui::BOX, motion::E_OUT, PLAYER_FADE_MS);
            return;
        }
        let (curve, ms) = self.mo.springs()[0].clone();
        bridge::flip_from(player::ui::BOX, r[0], r[1], r[2], r[3], &curve, ms);
    }

    /// `next` replaced the watch page's skeletons, unless this paint is instant: after a pointer
    /// open the description's lines and the related cards settle in as a short staggered wave (the title
    /// a card already showed never moves); after a keyboard or back/forward open only a short fade.
    pub(super) fn mo_settle(&mut self) {
        if self.mo.instant {
            return;
        }
        if self.mo.calm {
            let info = if self.mo.info_sk { render::INFO } else { "ytc-desc" };
            for id in [info, render::PLAYLIST, render::UPNEXT, render::LIST] {
                bridge::fade_in(id, motion::E_OUT, SETTLE_MS);
            }
            return;
        }
        let mv = if bridge::reduced_motion() { 0.0 } else { 1.0 };
        if self.mo.info_sk {
            bridge::fade_in(render::INFO, motion::E_OUT, SETTLE_MS);
        }
        bridge::stagger("ytc-desc", DESC_SEL, "", DESC_MAX, DESC_STEP, DESC_DY * mv, motion::E_OUT, WAVE_MS);
        bridge::stagger(render::LIST, ".c", "", REL_MAX, REL_STEP, REL_DY * mv, motion::E_OUT, WAVE_MS);
        for id in [render::PLAYLIST, render::UPNEXT] {
            bridge::fade_in(id, motion::E_OUT, SETTLE_MS);
        }
    }

    /// The player switched theater mode: a pointer click morphs it from its
    /// old box (as painted now) into the new one; the `t` key is instant.
    pub(super) fn mo_theater(&mut self, on: bool) {
        let morph = std::mem::take(&mut self.mo.theater) && !bridge::is_fullscreen() && !bridge::reduced_motion();
        let from = if morph { bridge::rect(player::ui::BOX) } else { Vec::new() };
        bridge::attr("ytc-watch", "class", if on { "watch theater" } else { "watch" });
        if from.len() == 4 {
            let (curve, ms) = self.mo.springs()[1].clone();
            bridge::flip_from(player::ui::BOX, from[0], from[1], from[2], from[3], &curve, ms);
        }
    }
}
