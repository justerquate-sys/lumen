//! Hedged InnerTube reads (tail latency). A read request that has not answered by its endpoint's
//! deadline is sent once more, identically (same URL, body, headers: the same session, account
//! and visitor); the first good answer is used and the other request is aborted. YouTube's
//! frontends pick a backend per request, so a second copy usually escapes a slow backend.
//!
//! Only reads are hedged (`browse`, `next`, `search`, `navigation/resolve_url` and their
//! continuations): an action (like, subscribe, comment, save…) never goes through here, so it is
//! never sent twice. Home's first `browse` is left to the Home path. Background (low-priority)
//! requests are not hedged while nobody waits for them (the bandwidth stays one response); a
//! background feed continuation the user then reaches the end of the list for is armed at that
//! moment (`arm`), and its copy goes out at normal priority.
//!
//! Deadline: about the session's p90 per endpoint (the originals' last `KEEP` good answers, also
//! those that beat their copy), clamped to `MIN_MS`..`MAX_MS`; the built-in values (the owner's
//! real p90s, 2 Nov 2026) until `MIN_SAMPLES` answers are in. When YouTube is slow for every
//! request a copy cannot win: after `LOSING` lost copies in a row an endpoint is not hedged for
//! `REST_MS`. Pure state; the app sends, aborts and times.

use std::collections::HashMap;

/// Answers kept per endpoint for its deadline.
const KEEP: usize = 40;
const MIN_SAMPLES: usize = 8;
pub const MIN_MS: f64 = 700.0;
pub const MAX_MS: f64 = 2500.0;
/// Requests with a stored copy (a lost answer must not grow the table).
const OPEN_MAX: usize = 48;
/// Hedge outcomes kept per endpoint; when the last `LOSING` copies all lost (YouTube is slow for
/// every request, so a second copy only costs bandwidth), that endpoint rests for `REST_MS`.
const OUTCOMES: usize = 8;
const LOSING: usize = 4;
pub const REST_MS: f64 = 60_000.0;

/// Endpoints that only read, and may go out twice.
pub fn readable(ep: &str) -> bool {
    matches!(ep, "browse" | "next" | "search" | "navigation/resolve_url")
}

/// The built-in deadline per endpoint (ms): the owner's measured p90 (2 Nov 2026).
pub fn default_ms(ep: &str) -> f64 {
    match ep {
        "next" => 1300.0,
        "browse" => 1300.0,
        "search" => 1200.0,
        _ => 1000.0,
    }
}

/// What the app needs to send the same request again.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub method: String,
    pub url: String,
    pub body: String,
    pub headers: String,
    pub opts: String,
    pub ep: &'static str,
    /// The second copy's request id, once sent, and when.
    pub copy: Option<u32>,
    pub fired_at: f64,
    /// The original failed while the copy was out (the copy's answer, good or bad, is final).
    pub failed: bool,
    /// Its deadline timer runs (a background continuation's starts only once the user waits for it).
    pub armed: bool,
}

/// How an answer is to be handled.
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// Not a hedged request: as usual.
    Plain,
    /// The original answered: use it (`abort` the copy if one went out); `fired` = a copy was sent.
    First { abort: Option<u32>, fired: bool },
    /// The copy answered first: handle it as the original `primary` (abort the original).
    Copy { primary: u32 },
    /// A failed answer while the other request still runs: wait for that one.
    Wait,
    /// A copy whose original is gone (aborted or answered): drop it.
    Drop,
}

#[derive(Default)]
pub struct Hedger {
    lat: Vec<(&'static str, Vec<f32>)>,
    open: HashMap<u32, Stored>,
    copies: HashMap<u32, u32>,
    /// Requests hedged / won by the copy in this document (debug overlay).
    pub fired: u32,
    pub won: u32,
    /// Hedging switched off (localStorage `ytc-test-nohedge` = 1: the benchmark's A/B runs).
    pub off: bool,
    /// Per endpoint: the last hedges' outcomes (true = the copy won) and a rest until (performance.now ms).
    outcomes: Vec<(&'static str, Vec<bool>, f64)>,
}

impl Hedger {
    pub fn new(off: bool) -> Self {
        Hedger { off, ..Default::default() }
    }

    /// The deadline for `ep` now.
    pub fn deadline(&self, ep: &str) -> f64 {
        let Some(v) = self.lat.iter().find(|x| x.0 == ep).map(|x| &x.1).filter(|v| v.len() >= MIN_SAMPLES) else {
            return default_ms(ep);
        };
        let mut s = v.clone();
        s.sort_unstable_by(f32::total_cmp);
        let p90 = s[((s.len() - 1) as f64 * 0.9).round() as usize] as f64;
        p90.clamp(MIN_MS, MAX_MS)
    }

    /// May a request to `ep` be hedged now?
    pub fn allowed(&self, ep: &str, now: f64) -> bool {
        !self.off && !self.outcomes.iter().any(|o| o.0 == ep && now < o.2)
    }

    /// A hedge of `ep` ended (`won` = the copy answered first).
    pub fn outcome(&mut self, ep: &'static str, won: bool, now: f64) {
        let i = match self.outcomes.iter().position(|o| o.0 == ep) {
            Some(i) => i,
            None => {
                self.outcomes.push((ep, Vec::new(), 0.0));
                self.outcomes.len() - 1
            }
        };
        let o = &mut self.outcomes[i];
        if o.1.len() >= OUTCOMES {
            o.1.remove(0);
        }
        o.1.push(won);
        if o.1.len() >= LOSING && o.1.iter().rev().take(LOSING).all(|w| !w) {
            o.2 = now + REST_MS;
            o.1.clear();
        }
    }

    /// The original's latency (a successful answer, also one that beat its copy).
    pub fn record(&mut self, ep: &'static str, ms: f64) {
        if !ms.is_finite() || ms <= 0.0 {
            return;
        }
        let v = match self.lat.iter().position(|x| x.0 == ep) {
            Some(i) => &mut self.lat[i].1,
            None => {
                self.lat.push((ep, Vec::new()));
                &mut self.lat.last_mut().expect("pushed").1
            }
        };
        if v.len() >= KEEP {
            v.remove(0);
        }
        v.push(ms as f32);
    }

    /// Keep `c` for request `id` (its deadline timer is the app's).
    pub fn watch(&mut self, id: u32, c: Stored) {
        if self.open.len() >= OPEN_MAX {
            return;
        }
        self.open.insert(id, c);
    }

    /// Arms request `id`'s deadline now; false when it is not open or armed already.
    pub fn arm(&mut self, id: u32) -> bool {
        match self.open.get_mut(&id) {
            Some(c) if !c.armed => {
                c.armed = true;
                true
            }
            _ => false,
        }
    }

    /// The deadline of request `id` passed: the copy to send (None when it answered, is gone or
    /// was hedged already).
    pub fn due(&mut self, id: u32) -> Option<Stored> {
        let c = self.open.get(&id).filter(|c| c.copy.is_none() && c.fired_at <= 0.0)?;
        Some(c.clone())
    }

    /// The copy of `id` went out as request `copy` at `now`.
    pub fn sent(&mut self, id: u32, copy: u32, now: f64) {
        if let Some(c) = self.open.get_mut(&id) {
            c.copy = Some(copy);
            c.fired_at = now;
            self.copies.insert(copy, id);
            self.fired += 1;
        }
    }

    /// Request `id` answered (`ok` = a usable answer); `primary_open` = the original is still
    /// waited for by the app.
    pub fn answer(&mut self, id: u32, ok: bool, primary_open: impl Fn(u32) -> bool) -> Answer {
        if let Some(&primary) = self.copies.get(&id) {
            self.copies.remove(&id);
            if !primary_open(primary) {
                self.open.remove(&primary);
                return Answer::Drop;
            }
            let failed = self.open.get(&primary).is_some_and(|c| c.failed);
            if !ok && !failed {
                // The original may still answer (and no third copy goes out).
                if let Some(c) = self.open.get_mut(&primary) {
                    c.copy = None;
                }
                return Answer::Wait;
            }
            self.open.remove(&primary);
            if ok {
                self.won += 1;
            }
            return Answer::Copy { primary };
        }
        let Some(c) = self.open.get(&id) else { return Answer::Plain };
        if !ok && c.copy.is_some() {
            // The copy is still out: it may answer well.
            if let Some(c) = self.open.get_mut(&id) {
                c.failed = true;
            }
            return Answer::Wait;
        }
        let c = self.open.remove(&id).expect("open");
        if let Some(x) = c.copy {
            self.copies.remove(&x);
        }
        Answer::First { abort: c.copy, fired: c.fired_at > 0.0 }
    }

    /// Request `id` was aborted or dropped by the app: the copy to abort, if one is out.
    pub fn forget(&mut self, id: u32) -> Option<u32> {
        let c = self.open.remove(&id)?;
        let x = c.copy?;
        self.copies.remove(&x);
        Some(x)
    }

    pub fn open_len(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy() -> Stored {
        Stored { method: "POST".into(), url: "u".into(), body: "b".into(), headers: "{}".into(), opts: "{}".into(), ep: "next", copy: None, fired_at: 0.0, failed: false, armed: true }
    }

    #[test]
    fn reads_only() {
        for ep in ["browse", "next", "search", "navigation/resolve_url"] {
            assert!(readable(ep));
        }
        for ep in ["like/like", "subscription/subscribe", "browse/edit_playlist", "comment/create_comment", "player", "playlist/get_add_to_playlist"] {
            assert!(!readable(ep), "{ep} is never hedged");
        }
    }

    #[test]
    fn deadline_follows_the_session() {
        let mut h = Hedger::default();
        assert_eq!(h.deadline("next"), 1300.0);
        assert_eq!(h.deadline("search"), 1200.0);
        for i in 0..20 {
            h.record("next", 300.0 + i as f64 * 20.0);
        }
        // p90 of 300..680 = 640 → the floor.
        assert_eq!(h.deadline("next"), MIN_MS);
        for _ in 0..KEEP {
            h.record("next", 1800.0);
        }
        assert_eq!(h.deadline("next"), 1800.0);
        for _ in 0..KEEP {
            h.record("next", 9000.0);
        }
        assert_eq!(h.deadline("next"), MAX_MS);
        h.record("next", f64::NAN);
    }

    #[test]
    fn copies_that_keep_losing_rest() {
        let mut h = Hedger::default();
        assert!(h.allowed("next", 0.0));
        h.outcome("next", true, 0.0);
        for _ in 0..LOSING - 1 {
            h.outcome("next", false, 1.0);
        }
        assert!(h.allowed("next", 1.0), "one win among them");
        h.outcome("next", false, 2.0);
        assert!(!h.allowed("next", 3.0), "every request is slow: copies only cost bandwidth");
        assert!(h.allowed("search", 3.0), "per endpoint");
        assert!(h.allowed("next", 2.0 + REST_MS));
        assert!(!Hedger::new(true).allowed("next", 0.0));
    }

    #[test]
    fn a_waited_background_request_is_armed_once() {
        let mut h = Hedger::default();
        h.watch(1, Stored { armed: false, ..copy() });
        assert!(h.arm(1));
        assert!(!h.arm(1), "once");
        assert!(!h.arm(2), "unknown");
    }

    #[test]
    fn first_good_answer_wins_and_the_other_is_aborted() {
        let mut h = Hedger::default();
        // Answered before the deadline: no copy.
        h.watch(1, copy());
        assert_eq!(h.answer(1, true, |_| true), Answer::First { abort: None, fired: false });
        assert!(h.due(1).is_none(), "answered: nothing to send");
        // Deadline passed: the copy goes out; the copy answers first.
        h.watch(2, copy());
        assert!(h.due(2).is_some());
        h.sent(2, 20, 100.0);
        assert!(h.due(2).is_none(), "one copy only");
        assert_eq!(h.answer(20, true, |p| p == 2), Answer::Copy { primary: 2 });
        assert_eq!(h.answer(2, true, |_| false), Answer::Plain, "the original is no longer ours");
        assert_eq!((h.fired, h.won), (1, 1));
        // The original answers first: the copy is aborted.
        h.watch(3, copy());
        h.sent(3, 30, 100.0);
        assert_eq!(h.answer(3, true, |_| true), Answer::First { abort: Some(30), fired: true });
        assert_eq!(h.answer(30, true, |_| true), Answer::Plain, "the aborted copy is unknown now");
        // A failure while the other is out waits for it.
        h.watch(4, copy());
        h.sent(4, 40, 100.0);
        assert_eq!(h.answer(40, false, |_| true), Answer::Wait);
        assert!(h.due(4).is_none(), "no third request");
        assert_eq!(h.answer(4, true, |_| true), Answer::First { abort: None, fired: true });
        h.watch(5, copy());
        h.sent(5, 50, 100.0);
        assert_eq!(h.answer(5, false, |_| true), Answer::Wait);
        assert_eq!(h.answer(50, true, |p| p == 5), Answer::Copy { primary: 5 });
        // Both fail: the second failure is the answer (the page shows its error, never hangs).
        h.watch(8, copy());
        h.sent(8, 80, 100.0);
        assert_eq!(h.answer(8, false, |_| true), Answer::Wait);
        assert_eq!(h.answer(80, false, |p| p == 8), Answer::Copy { primary: 8 });
        h.watch(9, copy());
        h.sent(9, 90, 100.0);
        assert_eq!(h.answer(90, false, |p| p == 9), Answer::Wait);
        assert_eq!(h.answer(9, false, |_| true), Answer::First { abort: None, fired: true });
        // The app dropped the original (navigation): its copy is aborted, a late copy dropped.
        h.watch(6, copy());
        h.sent(6, 60, 100.0);
        assert_eq!(h.forget(6), Some(60));
        assert_eq!(h.answer(60, true, |_| true), Answer::Plain);
        h.watch(7, copy());
        h.sent(7, 70, 100.0);
        assert_eq!(h.answer(70, true, |_| false), Answer::Drop);
        assert_eq!(h.open_len(), 0);
    }
}
