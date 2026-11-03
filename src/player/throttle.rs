//! When YouTube is slow (pure decisions, unit-tested): hedging the `player` request and the
//! googlevideo requests, recognising a throttled download early, and how far quality may drop
//! while it lasts. The glue is `own/hedge.rs`.
//!
//! Numbers (owner's data, 2 Nov 2026, development build telemetry):
//! - `req.player-visionos`: p50 92 ms, p90 205 ms, p95 265, p99 348, max 676 (158 answers).
//!   A second, identical request goes out at `VISION_HEDGE_MS` = 400 ms (≈ 2 × p90, above p99):
//!   it fires on ~1 % of starts (one extra ~100 KB request then) and turns a hung request into
//!   ~400 ms + one round trip instead of the 8 s timeout.
//! - `gv.head` (index + first segment): p50 8 ms, p90 455, max 639 ms (cold connections:
//!   DNS + TLS to a new googlevideo host). No response headers after `FIRST_TTFB_MS` = 700 ms
//!   (above every cold connection seen) means that host is slow or stuck: the same range goes
//!   to the next mirror at once, before the 2.5 s "no answer" refresh.

/// The `player` (VISIONOS) request is repeated after this long without an answer.
pub const VISION_HEDGE_MS: f64 = 400.0;
/// Before the first frame: no response headers from googlevideo after this long → hedge.
pub const FIRST_TTFB_MS: f64 = 700.0;
/// While playing: headers this late, with little buffered, → hedge.
pub const LATE_TTFB_MS: f64 = 1500.0;
/// "Little buffered" (seconds ahead of the playhead).
pub const LOW_AHEAD_S: f64 = 12.0;
/// The body rate is judged only after this much body time (TCP slow start settles).
pub const MIN_BODY_MS: f64 = 500.0;
/// A body this many times slower than the measured bandwidth is throttled.
pub const FAR: f64 = 6.0;
/// A download that ends within this anyway is left alone.
pub const NEAR_DONE_MS: f64 = 800.0;
/// A throttle episode ends after this long without a slow request.
pub const CALM_MS: f64 = 20_000.0;

/// One googlevideo request in flight, as the player sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Probe {
    /// ms since it was sent.
    pub elapsed: f64,
    /// ms from sending to the response headers (None: not yet).
    pub ttfb: Option<f64>,
    /// Body bytes so far and in total.
    pub got: f64,
    pub want: f64,
    /// The first frame has shown.
    pub started: bool,
    /// Seconds buffered ahead of the playhead (this track).
    pub ahead: f64,
    /// The media's own bit rate (what playback consumes), bits/s.
    pub need_bps: f64,
    /// Measured throughput (bits/s), if any.
    pub bw: Option<f64>,
}

/// Why a request counts as throttled: "ttfb" (headers late) or "rate" (body too slow).
pub fn judge(p: &Probe) -> Option<&'static str> {
    let low = !p.started || p.ahead < LOW_AHEAD_S;
    match p.ttfb {
        None => {
            let lim = if p.started { LATE_TTFB_MS } else { FIRST_TTFB_MS };
            (p.elapsed > lim && low).then_some("ttfb")
        }
        Some(t) => {
            let body = p.elapsed - t;
            if body < MIN_BODY_MS {
                return None;
            }
            let rate = p.got * 8000.0 / body;
            let left = (p.want - p.got).max(0.0);
            if left <= 0.0 || (rate > 0.0 && left * 8000.0 / rate < NEAR_DONE_MS) {
                return None;
            }
            let far = p.bw.is_some_and(|b| rate * FAR < b);
            // Under half of what playback consumes: it cannot keep up even with luck (a cold
            // connection still in slow start measured 0.6–0.9× on 2 Nov 2026, never below 0.5×).
            let behind = low && rate * 2.0 < p.need_bps;
            (far || behind).then_some("rate")
        }
    }
}

/// Body rate of a probe (bits/s), once there is enough body to tell.
pub fn rate(p: &Probe) -> Option<f64> {
    let body = p.elapsed - p.ttfb?;
    (body >= MIN_BODY_MS && p.got > 0.0).then(|| p.got * 8000.0 / body)
}

/// What to do about a throttled request, in the order the owner set (2 Nov 2026): the same range
/// on a mirror host, then fresh URLs (a new player response), then a lower quality. (A different
/// client is not a step: no other client returns direct URLs that play past ~60 s without a PO
/// token, docs/architecture.md "Bot check recovery".)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Mirror,
    Refresh,
    Drop,
}

/// `level`: how far this episode has gone (0 nothing yet, 1 mirrored, 2 refreshed, 3 dropped).
/// `paired`: the slow request already has a mirror twin (both are slow); `mirror`: another
/// mirror exists; `can_refresh`: a player refresh can be asked now.
pub fn step(level: u8, paired: bool, mirror: bool, can_refresh: bool) -> Step {
    if level < 2 && !paired && mirror {
        return Step::Mirror;
    }
    if level < 2 && can_refresh {
        return Step::Refresh;
    }
    Step::Drop
}

/// The highest rung (`(p, bits/s)`, highest first) whose bit rate fits 70 % of the throughput
/// actually seen; the lowest when none does. Never above `cur` (a drop never raises quality).
pub fn drop_to(rungs: &[(u32, u64)], bps: f64, cur: u32) -> Option<u32> {
    let fit = rungs.iter().filter(|r| r.0 < cur).find(|r| (r.1 as f64) <= bps * 0.7).or_else(|| rungs.iter().filter(|r| r.0 < cur).last());
    fit.map(|r| r.0)
}

/// How long a throttle cap holds before quality tries ≥ 1080p again: 15 s, then 30 s, 60 s, 120 s.
pub fn hold_ms(drops: u32) -> f64 {
    15_000.0 * f64::from(1u32 << drops.saturating_sub(1).min(3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Probe {
        Probe { elapsed: 0.0, ttfb: None, got: 0.0, want: 2_000_000.0, started: false, ahead: 0.0, need_bps: 2_500_000.0, bw: None }
    }

    #[test]
    fn late_headers() {
        assert_eq!(judge(&Probe { elapsed: 650.0, ..p() }), None, "a cold connection (≤ 639 ms seen) is not throttling");
        assert_eq!(judge(&Probe { elapsed: 720.0, ..p() }), Some("ttfb"));
        // Playing with a long buffer: a late answer is not worth a second request.
        assert_eq!(judge(&Probe { elapsed: 2000.0, started: true, ahead: 40.0, ..p() }), None);
        assert_eq!(judge(&Probe { elapsed: 1400.0, started: true, ahead: 3.0, ..p() }), None);
        assert_eq!(judge(&Probe { elapsed: 1600.0, started: true, ahead: 3.0, ..p() }), Some("ttfb"));
    }

    #[test]
    fn slow_body() {
        // 100 KB in 1 s of body = 0.8 Mb/s < half the 2.5 Mb/s the media needs, 1.9 MB left.
        let slow = Probe { elapsed: 1100.0, ttfb: Some(100.0), got: 100_000.0, ..p() };
        assert_eq!(judge(&slow), Some("rate"));
        assert_eq!(rate(&slow), Some(800_000.0));
        // Too early to tell (slow start).
        assert_eq!(judge(&Probe { elapsed: 500.0, ..slow }), None);
        // Nearly done anyway.
        assert_eq!(judge(&Probe { want: 150_000.0, ..slow }), None);
        // Fast enough for playback and not far below the measured bandwidth.
        let ok = Probe { got: 1_000_000.0, bw: Some(30_000_000.0), ..slow };
        assert_eq!(judge(&ok), None, "8 Mb/s with 30 Mb/s measured");
        assert_eq!(judge(&Probe { got: 200_000.0, ..slow }), None, "1.6 Mb/s: a slow start, not throttling");
        // Far below the measured bandwidth, even if playback could keep up.
        assert_eq!(judge(&Probe { got: 500_000.0, bw: Some(60_000_000.0), ..slow }), Some("rate"), "4 Mb/s vs 60 measured");
        // Playing with a long buffer: only "far below the measured bandwidth" counts.
        assert_eq!(judge(&Probe { started: true, ahead: 40.0, ..slow }), None);
        assert_eq!(judge(&Probe { started: true, ahead: 40.0, bw: Some(30_000_000.0), ..slow }), Some("rate"));
    }

    #[test]
    fn escalation_order() {
        assert_eq!(step(0, false, true, true), Step::Mirror);
        assert_eq!(step(1, true, true, true), Step::Refresh, "the mirror twin is slow too");
        assert_eq!(step(0, false, false, true), Step::Refresh, "no mirror listed");
        assert_eq!(step(2, false, true, true), Step::Drop, "after fresh URLs");
        assert_eq!(step(1, true, true, false), Step::Drop, "no refresh possible");
    }

    #[test]
    fn drop_and_hold() {
        let rungs = [(2160, 18_000_000), (1440, 9_000_000), (1080, 4_000_000), (720, 2_000_000), (480, 1_000_000), (360, 600_000)];
        assert_eq!(drop_to(&rungs, 2_000_000.0, 1080), Some(480), "1.4 Mb/s usable → 480p");
        assert_eq!(drop_to(&rungs, 3_000_000.0, 1080), Some(720));
        assert_eq!(drop_to(&rungs, 100_000.0, 1080), Some(360), "nothing fits: the lowest");
        assert_eq!(drop_to(&rungs, 50_000_000.0, 1080), Some(720), "never up, one rung down at least");
        assert_eq!(drop_to(&rungs, 1_000_000.0, 360), None);
        assert_eq!([1, 2, 3, 4, 9].map(hold_ms), [15_000.0, 30_000.0, 60_000.0, 120_000.0, 120_000.0]);
    }
}
