//! Speed telemetry of the player (`crate::telemetry`): the start breakdown (`video.start`:
//! input → first frame with VISIONOS, decodingInfo, MediaSource and the first googlevideo
//! requests), time to the target quality and to ≥ 1080p, every googlevideo index request and a
//! sample of segment requests (the first ones, slow and failed ones), seeks (input → playing),
//! quality switches (decision → the new resolution on screen), stalls, dropped frames per rung,
//! menu and toast latency, and a session summary when the player stops. Hooks in `own.rs` are
//! one line each; nothing here runs per frame or reads layout.

use super::{A, Inflight, Own, Req, V};
use crate::bridge;
use crate::telemetry as tel;
use serde_json::{Value, json};

/// googlevideo requests kept in the start breakdown.
const START_REQS: usize = 16;
/// Segment requests reported per video beyond the start (plus every slow or failed one).
const SEG_SAMPLE: u32 = 8;
const SLOW_MS: f64 = 1500.0;
/// Per-video caps on repeated events.
const SEEKS_MAX: u32 = 40;
const SWITCHES_MAX: u32 = 30;
const STALLS_MAX: u32 = 30;
/// Events the `<video>` also reports while telemetry runs.
pub(super) const EXTRA_EVENTS: &str = ",resize,seeked";

#[derive(Default)]
pub(super) struct PTel {
    on: bool,
    /// The input (or navigation) that started this video, and what it was.
    t0: f64,
    input: &'static str,
    vision_src: &'static str,
    vision_age: Option<f64>,
    bot: u32,
    caps_at: f64,
    caps_ms: Option<f64>,
    ms_open: Option<f64>,
    streams: Option<f64>,
    start_p: u32,
    target_p: u32,
    reqs: Vec<Value>,
    segs: u32,
    started: bool,
    target_at: Option<f64>,
    hd_at: Option<f64>,
    shown_p: u32,
    /// A quality switch decided: (when, from p, to p, why).
    switch: Option<(f64, u32, u32, &'static str)>,
    /// A seek in progress: (input time, what caused it, waited for data).
    seek: Option<(f64, &'static str, bool)>,
    seeks: u32,
    switches: u32,
    stalls: u32,
    stall_ms: f64,
    pub(super) stuck: u32,
    /// Dropped/total frames per rung (p) and the last sample.
    drops: Vec<(u32, f64, f64)>,
    dq: (f64, f64),
    done: bool,
}

/// The host a request went to (from the request log's description).
fn host(what: &str) -> &str {
    what.split(' ').find(|w| w.contains('.')).unwrap_or("")
}

impl Own {
    /// At creation: telemetry on? From which input?
    pub(super) fn tel_init(&mut self) {
        self.pt.on = tel::enabled();
        if !self.pt.on {
            return;
        }
        let now = self.created;
        let (k, t) = tel::cause();
        let (input, t0) = if k != "auto" {
            (k, t)
        } else {
            match tel::nav_t0().filter(|n| now - n < 5000.0) {
                Some(n) => ("navigation", n),
                None => ("auto", now),
            }
        };
        self.pt.input = input;
        self.pt.t0 = t0;
        self.pt.vision_src = "fetch";
    }

    pub(super) fn video_events(&self) -> String {
        if self.pt.on { format!("{}{EXTRA_EVENTS}", super::VIDEO_EVENTS) } else { super::VIDEO_EVENTS.to_string() }
    }

    /// The app's VISIONOS source for this start: "cache" (with its age), "prefetch" (in flight), "fetch".
    pub fn tel_vision(&mut self, src: &'static str, age: Option<f64>) {
        self.pt.vision_src = src;
        self.pt.vision_age = age;
    }

    pub(super) fn tel_caps_asked(&mut self) {
        self.pt.caps_at = bridge::now();
    }

    pub(super) fn tel_caps(&mut self) {
        if self.pt.on && self.pt.caps_ms.is_none() && self.pt.caps_at > 0.0 {
            self.pt.caps_ms = Some(bridge::now() - self.pt.caps_at);
        }
    }

    pub(super) fn tel_ms_open(&mut self) {
        if self.pt.on && self.pt.ms_open.is_none() {
            self.pt.ms_open = Some(bridge::now() - self.created);
        }
    }

    pub(super) fn tel_streams(&mut self, start_p: u32, target_p: u32) {
        if self.pt.on && self.pt.streams.is_none() {
            self.pt.streams = Some(bridge::now() - self.pt.t0);
            self.pt.start_p = start_p;
            self.pt.target_p = target_p;
        }
    }

    pub(super) fn tel_bot(&mut self) {
        self.pt.bot += 1;
    }

    /// Every answered request of this player.
    pub(super) fn tel_req(&mut self, f: &Inflight, status: u32, bytes: usize, ms: f64) {
        if !self.pt.on {
            return;
        }
        let ok = (200..300).contains(&status);
        let (m, gv, kind, itag): (&str, bool, &str, u32) = match f.req {
            Req::Vision(_) => ("req.player-visionos", false, "", 0),
            Req::Visitor => ("req.visitor_id", false, "", 0),
            Req::Web => ("req.player-web", false, "", 0),
            Req::Captions => ("req.timedtext", false, "", 0),
            Req::Sponsor => ("req.sponsorblock", false, "", 0),
            Req::Ping => return,
            Req::Head { itag, kind } => ("gv.head", true, if kind == V { "v" } else { "a" }, itag),
            Req::Seg { itag, kind, .. } => ("gv.seg", true, if kind == V { "v" } else { "a" }, itag),
            Req::LiveSeg { itag, kind, .. } => ("gv.live", true, if kind == V { "v" } else { "a" }, itag),
            Req::Master | Req::Playlist => ("gv.hls", true, "", 0),
        };
        let seg = matches!(f.req, Req::Seg { .. } | Req::LiveSeg { .. });
        let kbps = if ms > 0.0 { (bytes as f64 * 8.0 / ms).round() } else { 0.0 };
        if gv && !self.pt.started && self.pt.reqs.len() < START_REQS {
            self.pt.reqs.push(json!({"kind": kind, "itag": itag, "ms": tel::round1(ms), "bytes": bytes, "status": status, "mirror": self.mirror, "at": tel::round1(f.sent - self.pt.t0)}));
        }
        if seg {
            let sample = self.pt.segs < SEG_SAMPLE;
            if !(sample || !ok || ms > SLOW_MS) {
                return;
            }
            self.pt.segs += 1;
        }
        let mut d = json!({"vid": self.v, "status": status, "bytes": bytes});
        if gv {
            d["kind"] = kind.into();
            d["itag"] = itag.into();
            d["host"] = host(&f.what).into();
            d["mirror"] = self.mirror.into();
            d["kbps"] = kbps.into();
            d["started"] = self.pt.started.into();
        }
        if let Req::Vision(r) = &f.req {
            d["route"] = r.label().into();
            d["refresh"] = self.refreshing.into();
        }
        tel::emit(m, ms, d);
    }

    /// The first frame: `video.start` with the breakdown.
    pub(super) fn tel_first_frame(&mut self) {
        if !self.pt.on || self.pt.started {
            return;
        }
        self.pt.started = true;
        let now = bridge::now();
        let d = self.tel_breakdown(now);
        tel::emit("video.start", now - self.pt.t0, d);
    }

    fn tel_breakdown(&self, now: f64) -> Value {
        let watch = tel::nav_visible().filter(|t| *t >= self.pt.t0).map(|t| tel::round1(t - self.pt.t0));
        json!({
            "vid": self.v,
            "input": self.pt.input,
            "watch_ms": watch,
            "vision": {"src": self.pt.vision_src, "age_ms": self.pt.vision_age.map(|a| a.round()), "ms": self.vision_ms.map(tel::round1), "route": self.route.label(), "bot_checks": self.pt.bot},
            "caps_ms": self.pt.caps_ms.map(tel::round1),
            "mse_open_ms": self.pt.ms_open.map(tel::round1),
            "streams_ms": self.pt.streams.map(tel::round1),
            "start_p": self.pt.start_p,
            "target_p": self.pt.target_p,
            "gv": self.pt.reqs,
            "refreshes": self.refreshes,
            "mirror": self.mirror,
            "stuck": self.pt.stuck,
            "hidden_wait": self.hidden_wait,
            "mse_rebuilt": self.mse_rebuilt,
            "live": self.live.is_some(),
            "since_created_ms": tel::round1(now - self.created),
        })
    }

    /// A quality switch was decided (the new resolution shows at `resize`).
    pub(super) fn tel_switch(&mut self, itag: u32) {
        if !self.pt.on {
            return;
        }
        let from = self.format(self.tracks[V].rep, V).map(|f| f.p).unwrap_or(0);
        let to = self.format(itag, V).map(|f| f.p).unwrap_or(0);
        let why = if bridge::now() - self.last_input < 1000.0 { "user" } else { "auto" };
        self.pt.switch = Some((bridge::now(), from, to, why));
    }

    /// `<video>` events, before the player handles them.
    pub(super) fn tel_media(&mut self, ev: &str) {
        if !self.pt.on {
            return;
        }
        let now = bridge::now();
        match ev {
            "seeking" => {
                let own = self.own_seek.is_some_and(|(at, when)| (self.time() - at).abs() < 0.25 && now - when < 3000.0);
                if own || self.first_frame.is_none() {
                    return;
                }
                let (k, t) = tel::cause();
                self.pt.seek = Some((t, if k == "auto" { "auto" } else { k }, false));
            }
            "waiting" => {
                if let Some(s) = self.pt.seek.as_mut() {
                    s.2 = true;
                }
            }
            "seeked" => {
                if self.pt.seek.is_some_and(|s| !s.2) {
                    self.tel_seek_done(now);
                }
            }
            "playing" | "canplay" => {
                if self.pt.seek.is_some() && (ev == "playing" || self.paused) {
                    self.tel_seek_done(now);
                } else if ev == "playing" && self.waiting && self.first_frame.is_some() && self.pt.stalls < STALLS_MAX {
                    let ms = now - self.waiting_since;
                    self.pt.stalls += 1;
                    self.pt.stall_ms += ms;
                    let cause = if bridge::now() - self.last_switch < 2000.0 { "switch" } else if self.refreshing || self.stuck.is_some() { "network" } else { "underrun" };
                    let p = self.format(self.tracks[V].rep, V).map(|f| f.p).unwrap_or(0);
                    tel::emit("video.stall", ms, json!({"vid": self.v, "cause": cause, "p": p, "at_s": tel::round1(self.time()), "bw_kbps": self.bw.bps.map_or(0.0, |b| (b / 1000.0).round())}));
                }
            }
            "resize" => self.tel_resize(now),
            _ => {}
        }
    }

    fn tel_seek_done(&mut self, now: f64) {
        let Some((t0, why, waited)) = self.pt.seek.take() else { return };
        if self.pt.seeks >= SEEKS_MAX {
            return;
        }
        self.pt.seeks += 1;
        tel::emit("video.seek", now - t0, json!({"vid": self.v, "input": why, "waited": waited, "paused": self.paused, "at_s": tel::round1(self.time())}));
    }

    /// The picture's size changed: a switch became visible; target/1080p reached; drops per rung.
    fn tel_resize(&mut self, now: f64) {
        let (w, h) = (bridge::media_get(self.vid, "videoWidth"), bridge::media_get(self.vid, "videoHeight"));
        if !(w > 0.0 && h > 0.0) {
            return;
        }
        let p = self.formats.iter().find(|f| f.video && f.width as f64 == w && f.height as f64 == h).map(|f| f.p).unwrap_or(w.min(h) as u32);
        self.tel_drops_sample();
        self.pt.shown_p = p;
        if let Some((at, from, to, why)) = self.pt.switch.filter(|s| s.2 == p) {
            self.pt.switch = None;
            if self.pt.switches < SWITCHES_MAX {
                self.pt.switches += 1;
                tel::emit("video.switch", now - at, json!({"vid": self.v, "from_p": from, "to_p": to, "why": why}));
            }
        }
        if self.pt.target_at.is_none() && self.pt.target_p > 0 && p >= self.pt.target_p {
            self.pt.target_at = Some(now);
            tel::emit("video.target", now - self.pt.t0, json!({"vid": self.v, "p": p, "start_p": self.pt.start_p}));
        }
        if self.pt.hd_at.is_none() && p >= 1080 {
            self.pt.hd_at = Some(now);
            tel::emit("video.1080", now - self.pt.t0, json!({"vid": self.v, "p": p}));
        }
    }

    /// Frames since the last sample count for the rung shown until now.
    fn tel_drops_sample(&mut self) {
        let q = bridge::playback_quality(self.vid);
        if q.len() != 2 {
            return;
        }
        let (d, t) = (q[0], q[1]);
        let (pd, pt) = self.pt.dq;
        self.pt.dq = (d, t);
        if self.pt.shown_p == 0 || t < pt {
            return;
        }
        let p = self.pt.shown_p;
        match self.pt.drops.iter_mut().find(|x| x.0 == p) {
            Some(x) => {
                x.1 += d - pd;
                x.2 += t - pt;
            }
            None => self.pt.drops.push((p, d - pd, t - pt)),
        }
    }

    /// Menu panel opened (`kind`): input → the frame that shows it.
    pub(super) fn tel_menu(&self, kind: &str) {
        if self.pt.on {
            let t0 = tel::event_t();
            tel::frame("ui.menu", t0, json!({"menu": kind, "decide_ms": tel::round1(bridge::now() - t0)}));
        }
    }

    pub(super) fn tel_toast(&self) {
        if self.pt.on {
            tel::frame("ui.toast", tel::event_t(), json!({}));
        }
    }

    /// The player stops (left, replaced) or fails: the session summary (and why a start never showed).
    pub(super) fn tel_end(&mut self, failed: Option<&str>) {
        if !self.pt.on || self.pt.done {
            return;
        }
        self.pt.done = true;
        let now = bridge::now();
        if !self.failed && self.vid != 0 && self.pt.started {
            self.tel_drops_sample();
        }
        if !self.pt.started {
            let mut d = self.tel_breakdown(now);
            d["reason"] = failed.map(|r| r.chars().take(160).collect::<String>()).into();
            tel::emit(if failed.is_some() { "video.fail" } else { "video.abandon" }, now - self.pt.t0, d);
        }
        for (p, dropped, total) in &self.pt.drops {
            if *total >= 30.0 {
                tel::emit("video.drops", (dropped / total * 1000.0).round() / 10.0, json!({"vid": self.v, "p": p, "dropped": dropped, "frames": total}));
            }
        }
        tel::emit(
            "video.session",
            now - self.created,
            json!({
                "vid": self.v,
                "started": self.pt.started,
                "failed": failed.is_some(),
                "first_frame_ms": self.first_frame.map(tel::round1),
                "target_ms": self.pt.target_at.map(|t| tel::round1(t - self.pt.t0)),
                "hd_ms": self.pt.hd_at.map(|t| tel::round1(t - self.pt.t0)),
                "refreshes": self.refreshes,
                "mirror": self.mirror,
                "stuck": self.pt.stuck,
                "stalls": self.pt.stalls,
                "stall_ms": tel::round1(self.pt.stall_ms),
                "seeks": self.pt.seeks,
                "switches": self.pt.switches,
                "bad_itags": self.bad.len(),
                "video_itag": self.tracks[V].rep,
                "audio_itag": self.tracks[A].rep,
                "bw_kbps": self.bw.bps.map_or(0.0, |b| (b / 1000.0).round()),
                "live": self.live.is_some(),
            }),
        );
    }
}
