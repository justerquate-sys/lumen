//! Speed telemetry for the debug center (development builds): every speed metric of starting a
//! video and everything else, kept so it can be inspected at any time. Structured events, batched: every user-visible
//! path records one small JSON event (`m` metric, `v` value, `at` epoch ms, a breakdown); the
//! batch goes to the background worker as a `kind: "perf"`, `schema: 2` report at most once a
//! minute (or at 200 events, or when the tab is hidden or left). The browser's own
//! responsiveness entries (Long Animation Frames, Event Timing, layout shifts, storyboard
//! resource timings) are buffered by the bridge's observers and pulled once per batch.
//!
//! Cost rules: no per-frame bridge calls, no layout reads; one `performance.now()` per mark;
//! a "visible" time is one `requestAnimationFrame` + task after the DOM write (`tel_frame`).
//! The time spent in here is measured and reported with each batch (`overhead_ms`).
//!
//! Gate: decided at build time. Only the development build (`./build.sh --dev`, Cargo feature
//! `dev`) records anything; there chrome.storage.local `ytcTelemetryV1: false` turns it off.
//! The public build never records, stores or sends telemetry.
//!
//! Privacy: video ids and route kinds only (no search terms, URLs with tokens, cookies,
//! headers, visitor data, descriptions or keys).

use serde_json::{Map, Value, json};
use std::cell::RefCell;

/// Batch schema (`kind: "perf"` reports without `schema` are the older 10-minute summaries).
pub const SCHEMA: u32 = 2;
/// Events per batch; a full buffer is sent at once.
pub const BATCH_MAX: usize = 200;
/// Events kept while a batch cannot be sent (beyond: counted in `dropped`).
const BUF_MAX: usize = 400;
/// A batch with events goes out after this long.
pub const FLUSH_MS: f64 = 60_000.0;
/// An input older than this did not cause what happens now (an autoplay, a timer).
const INPUT_FRESH_MS: f64 = 1000.0;
/// Frame spans waiting for their frame (a lost callback must not grow the list).
const SPANS_MAX: usize = 32;
/// Requests kept in a navigation's breakdown.
const NAV_REQS: usize = 12;
/// Development build (Cargo feature `dev`): telemetry may run. Fixed at build time.
pub const DEV_BUILD: bool = cfg!(feature = "dev");
pub const SWITCH_KEY: &str = "ytcTelemetryV1";

/// What a navigation's breakdown records until its content is visible.
#[derive(Default, Clone)]
pub struct NavSpan {
    pub id: u32,
    pub route: &'static str,
    /// push / pop / boot.
    pub mode: &'static str,
    /// pointer / keyboard / history / auto / boot.
    pub input: &'static str,
    /// memory / snapshot / cache / prefetch / inflight / network.
    pub src: &'static str,
    /// Input time (performance.now ms).
    pub t0: f64,
    /// Navigation start in the app (after the input handler began).
    pub t_nav: f64,
    pub reqs: Vec<Value>,
}

#[derive(Default)]
struct Span {
    token: u32,
    m: String,
    t0: f64,
    fields: Map<String, Value>,
}

#[derive(Default)]
pub struct Tel {
    /// None: the gate's storage answers are not in yet (events are kept, not sent).
    on: Option<bool>,
    dev: bool,
    switch: Option<bool>,
    buf: Vec<Value>,
    dropped: u32,
    first_at: f64,
    seq: u32,
    page: String,
    pub client_version: String,
    pub signed_in: bool,
    /// Last input: kind and its event time.
    input: (&'static str, f64),
    nav: Option<NavSpan>,
    nav_seq: u32,
    /// The current page's content became visible (ms) and its navigation id.
    pub nav_visible: Option<(u32, f64)>,
    /// Input time of the current page's navigation.
    pub nav_t0: Option<f64>,
    /// User-visible waits begun (name, input time), ended by `wait_end`.
    waits: Vec<(&'static str, f64)>,
    spans: Vec<Span>,
    token: u32,
    overhead: f64,
}

impl Tel {
    pub fn enabled(&self) -> bool {
        self.on != Some(false)
    }

    fn decide(&mut self) {
        self.on = if !self.dev || self.switch == Some(false) { Some(false) } else { Some(true) };
        if self.on == Some(false) {
            self.buf.clear();
            self.spans.clear();
            self.nav = None;
        }
    }

    /// An event: metric, value (ms unless the metric says otherwise), epoch ms, breakdown.
    pub fn push(&mut self, m: &str, v: f64, at: f64, fields: Map<String, Value>) {
        if !self.enabled() || !v.is_finite() {
            return;
        }
        if self.buf.len() >= BUF_MAX {
            self.dropped += 1;
            return;
        }
        if self.buf.is_empty() {
            self.first_at = at;
        }
        // m, v and at first (and never overwritten by the breakdown).
        let mut o = Map::new();
        o.insert("m".into(), m.into());
        o.insert("v".into(), json!(round1(v)));
        o.insert("at".into(), json!(at.round()));
        for (k, x) in fields {
            if !matches!(k.as_str(), "m" | "v" | "at") {
                o.insert(k, x);
            }
        }
        self.buf.push(Value::Object(o));
    }

    /// A batch is due: full, or old enough.
    pub fn due(&self, epoch: f64) -> bool {
        self.on == Some(true) && (self.buf.len() >= BATCH_MAX || (!self.buf.is_empty() && epoch - self.first_at >= FLUSH_MS))
    }

    /// The batch report (up to `BATCH_MAX` events); None when off or empty.
    pub fn take(&mut self, epoch: f64, js_overhead: f64) -> Option<Value> {
        if self.on != Some(true) || self.buf.is_empty() {
            return None;
        }
        let n = self.buf.len().min(BATCH_MAX);
        let events: Vec<Value> = self.buf.drain(..n).collect();
        self.first_at = if self.buf.is_empty() { 0.0 } else { epoch };
        self.seq += 1;
        let r = json!({
            "kind": "perf",
            "schema": SCHEMA,
            "reason": "speed events",
            "at": epoch.round(),
            "client": env!("CARGO_PKG_VERSION"),
            "client_version": self.client_version,
            "signed_in_cookie": self.signed_in,
            "page": self.page,
            "seq": self.seq,
            "dropped": std::mem::take(&mut self.dropped),
            "overhead_ms": round1(std::mem::take(&mut self.overhead)),
            "js_overhead_ms": round1(js_overhead),
            "events": events,
        });
        Some(r)
    }

    pub fn input(&mut self, kind: &'static str, t: f64) {
        self.input = (kind, t);
    }

    /// The input that caused what starts at `now` (or `auto` when there was none just before).
    pub fn cause(&self, now: f64) -> (&'static str, f64) {
        let (k, t) = self.input;
        if k.is_empty() || now - t > INPUT_FRESH_MS || t > now { ("auto", now) } else { (k, t) }
    }

    pub fn nav_start(&mut self, route: &'static str, mode: &'static str, now: f64) -> u32 {
        let (input, t0) = if mode == "boot" { ("boot", 0.0) } else if mode == "pop" { ("history", self.cause(now).1) } else { self.cause(now) };
        self.nav_seq += 1;
        self.nav = Some(NavSpan { id: self.nav_seq, route, mode, input, src: "network", t0, t_nav: now, reqs: Vec::new() });
        self.nav_t0 = Some(t0);
        self.waits.clear();
        self.nav_visible = None;
        self.input = ("", 0.0);
        self.nav_seq
    }

    pub fn nav_src(&mut self, src: &'static str) {
        if let Some(n) = self.nav.as_mut() {
            n.src = src;
        }
    }

    pub fn nav_req(&mut self, r: Value) {
        if let Some(n) = self.nav.as_mut().filter(|n| n.reqs.len() < NAV_REQS) {
            n.reqs.push(r);
        }
    }

    /// The navigation's content is in the DOM at `now`: its event is completed by the next frame.
    /// `marks` are the app's marks (ms since its navigation start).
    pub fn nav_inserted(&mut self, now: f64, marks: &[(&'static str, f64)]) -> Option<(u32, NavSpan, Map<String, Value>)> {
        let n = self.nav.take()?;
        let mark = |k: &str| marks.iter().find(|m| m.0 == k).map(|m| m.1);
        let mut f = Map::new();
        f.insert("route".into(), n.route.into());
        f.insert("mode".into(), n.mode.into());
        f.insert("input".into(), n.input.into());
        f.insert("src".into(), n.src.into());
        f.insert("handler_ms".into(), json!(round1(n.t_nav - n.t0)));
        f.insert("insert_ms".into(), json!(round1(now - n.t0)));
        if let (Some(a), Some(b)) = (mark("request"), mark("response")) {
            f.insert("wait_ms".into(), json!(round1(b - a)));
        }
        if let (Some(a), Some(b)) = (mark("response"), mark("parsed")) {
            f.insert("parse_ms".into(), json!(round1(b - a)));
        }
        if let (Some(a), Some(b)) = (mark("parsed"), mark("rendered")) {
            f.insert("render_ms".into(), json!(round1(b - a)));
        }
        if !n.reqs.is_empty() {
            f.insert("reqs".into(), Value::Array(n.reqs.clone()));
        }
        Some((self.next_token(), n, f))
    }

    fn next_token(&mut self) -> u32 {
        self.token = self.token.wrapping_add(1).max(1);
        self.token
    }

    /// Starts a frame span: completed (an event `m` with `v` = frame − `t0`) by `frame_done`.
    pub fn span(&mut self, m: &str, t0: f64, fields: Map<String, Value>) -> Option<u32> {
        if !self.enabled() {
            return None;
        }
        if self.spans.len() >= SPANS_MAX {
            self.spans.remove(0);
        }
        let token = self.next_token();
        self.spans.push(Span { token, m: m.to_string(), t0, fields });
        Some(token)
    }

    pub fn span_with(&mut self, token: u32, m: &str, t0: f64, fields: Map<String, Value>) {
        if self.spans.len() >= SPANS_MAX {
            self.spans.remove(0);
        }
        self.spans.push(Span { token, m: m.to_string(), t0, fields });
    }

    /// The frame after a span's DOM write rendered at `t` (performance.now) / `epoch`.
    pub fn frame_done(&mut self, token: u32, t: f64, epoch: f64) -> Option<(String, f64)> {
        let i = self.spans.iter().position(|s| s.token == token)?;
        let s = self.spans.remove(i);
        let v = t - s.t0;
        self.push(&s.m, v, epoch, s.fields);
        Some((s.m, t))
    }

    /// Browser entries from the bridge's observers (`tel_take`): JSON array of
    /// `{k, t, ms, …}`, `t` in performance.now ms.
    pub fn browser_entries(&mut self, json: &str, now: f64, epoch: f64, route: &'static str) {
        let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else { return };
        for e in list {
            let Value::Object(mut o) = e else { continue };
            let k = o.remove("k").and_then(|k| k.as_str().map(str::to_string)).unwrap_or_default();
            if k == "drop" {
                self.dropped += o.get("n").and_then(Value::as_u64).unwrap_or(0) as u32;
                continue;
            }
            let t = o.remove("t").and_then(|t| t.as_f64()).unwrap_or(now);
            let v = o.remove("ms").and_then(|t| t.as_f64()).unwrap_or(0.0);
            let m = match k.as_str() {
                "loaf" => "lag.loaf",
                "longtask" => "lag.longtask",
                "event" => "lag.event",
                "res" => "res.storyboard",
                _ => continue,
            };
            for (_, x) in o.iter_mut() {
                if let Some(f) = x.as_f64().filter(|f| f.fract() != 0.0) {
                    *x = json!(round1(f));
                }
            }
            o.insert("route".into(), route.into());
            self.push(m, v, epoch - (now - t), o);
        }
    }

    pub fn add_overhead(&mut self, ms: f64) {
        if ms.is_finite() && ms > 0.0 {
            self.overhead += ms;
        }
    }
}

pub fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

// ------------------------------------------------------------------ glue (browser)

thread_local! {
    static T: RefCell<Tel> = RefCell::new(Tel::default());
}

pub fn with<R>(f: impl FnOnce(&mut Tel) -> R) -> Option<R> {
    T.with(|c| c.try_borrow_mut().ok().map(|mut t| f(&mut t)))
}

mod glue {
    use super::*;
    use crate::bridge;

    /// Measures the time spent in `f` as telemetry overhead.
    fn timed<R>(f: impl FnOnce(&mut Tel) -> R) -> Option<R> {
        let t = bridge::now();
        let r = with(|tel| {
            if !tel.enabled() {
                return None;
            }
            Some(f(tel))
        })
        .flatten();
        let dt = bridge::now() - t;
        with(|tel| tel.add_overhead(dt));
        r
    }

    /// Boot: the gate (the build flavor now; the off switch answers in `on_storage`) and the
    /// browser observers. A public build stops here: no storage read, no observers.
    pub fn boot(page: &str) {
        with(|t| {
            t.dev = DEV_BUILD;
            t.page = page.to_string();
            t.decide();
        });
        if !DEV_BUILD {
            return;
        }
        bridge::storage_get(SWITCH_KEY);
        if enabled() {
            bridge::tel_start();
        }
    }

    /// chrome.storage answers for the gate; true when the key was ours.
    pub fn on_storage(key: &str, json: &str) -> bool {
        if key != SWITCH_KEY {
            return false;
        }
        let v = json != "false";
        let was = enabled();
        with(|t| {
            t.switch = Some(v);
            t.decide();
        });
        if enabled() && !was {
            bridge::tel_start();
        }
        true
    }

    pub fn enabled() -> bool {
        with(|t| t.enabled()).unwrap_or(false)
    }

    pub fn emit(m: &str, v: f64, fields: Value) {
        let fields = match fields {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        let epoch = bridge::epoch();
        timed(|t| t.push(m, v, epoch, fields));
    }

    pub fn input(kind: &'static str) {
        let now = bridge::now();
        let ts = bridge::event_ts();
        let t = if ts > 0.0 && ts <= now && now - ts < INPUT_FRESH_MS { ts } else { now };
        timed(|tel| tel.input(kind, t));
    }

    /// The input (or decision) time for something starting now.
    pub fn cause() -> (&'static str, f64) {
        let now = bridge::now();
        with(|t| t.cause(now)).unwrap_or(("auto", now))
    }

    /// The time of the event being handled right now (falls back to now).
    pub fn event_t() -> f64 {
        let now = bridge::now();
        let ts = bridge::event_ts();
        if ts > 0.0 && ts <= now && now - ts < INPUT_FRESH_MS { ts } else { now }
    }

    /// Starts a span completed at the next rendered frame.
    pub fn frame(m: &str, t0: f64, fields: Value) {
        let fields = match fields {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        if let Some(token) = timed(|t| t.span(m, t0, fields)).flatten() {
            bridge::tel_frame(token);
        }
    }

    pub fn nav_start(route: &'static str, mode: &'static str) {
        let now = bridge::now();
        timed(|t| t.nav_start(route, mode, now));
    }

    pub fn nav_src(src: &'static str) {
        timed(|t| t.nav_src(src));
    }

    pub fn nav_req(r: Value) {
        timed(|t| t.nav_req(r));
    }

    /// The page's content was written to the DOM: the navigation event completes at the next frame.
    pub fn nav_inserted(marks: &[(&'static str, f64)]) {
        let now = bridge::now();
        let token = timed(|t| {
            let (token, n, f) = t.nav_inserted(now, marks)?;
            t.span_with(token, &format!("nav.{}", n.route), n.t0, f);
            Some(token)
        })
        .flatten();
        if let Some(token) = token {
            bridge::tel_frame(token);
        }
    }

    /// Bridge: the frame after a span's write rendered.
    pub fn on_frame(token: u32, t: f64) {
        let epoch = bridge::epoch();
        timed(|tel| {
            if let Some((m, at)) = tel.frame_done(token, t, epoch) {
                if m.starts_with("nav.") {
                    let id = tel.nav_seq;
                    tel.nav_visible = Some((id, at));
                }
            }
        });
    }

    /// Input time of the current page's navigation.
    pub fn nav_t0() -> Option<f64> {
        with(|t| t.nav_t0).flatten()
    }

    /// A wait the user sees begins now (e.g. the end of a feed reached before more cards are in).
    pub fn wait_start(name: &'static str) {
        let now = bridge::now();
        timed(|t| {
            if !t.waits.iter().any(|w| w.0 == name) && t.waits.len() < 8 {
                t.waits.push((name, now));
            }
        });
    }

    /// The wait `name` ends with a DOM write now: an event at the next frame.
    pub fn wait_end(name: &'static str, fields: Value) {
        let t0 = with(|t| t.waits.iter().position(|w| w.0 == name).map(|i| t.waits.remove(i).1)).flatten();
        if let Some(t0) = t0 {
            frame(name, t0, fields);
        }
    }

    /// When the current page's content became visible (performance.now), if it has.
    pub fn nav_visible() -> Option<f64> {
        with(|t| t.nav_visible.map(|v| v.1)).flatten()
    }

    /// Layout shift of the page being left (one event per page).
    pub fn page_left(route: &'static str) {
        if !enabled() {
            return;
        }
        let cls = bridge::tel_cls();
        if cls >= 0.0 {
            emit("lag.cls", (cls * 1000.0).round() / 1000.0, json!({"route": route}));
        }
    }

    /// Sends a batch now if one is due (`force`: whatever is buffered, e.g. the tab was hidden).
    pub fn flush(force: bool, client_version: &str, signed_in: bool, route: &'static str) {
        let epoch = bridge::epoch();
        let ready = with(|t| t.on == Some(true) && (force || t.due(epoch))).unwrap_or(false);
        if !ready {
            return;
        }
        let now = bridge::now();
        let js = bridge::tel_take();
        let js_cost = bridge::tel_cost();
        let report = with(|t| {
            t.client_version = client_version.to_string();
            t.signed_in = signed_in;
            t.browser_entries(&js, now, epoch, route);
            let dt = bridge::now() - now;
            t.add_overhead(dt);
            t.take(epoch, js_cost)
        })
        .flatten();
        if let Some(r) = report {
            bridge::perf_batch(&r.to_string());
        }
    }
}

pub use glue::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Tel {
        let mut t = Tel { dev: true, ..Default::default() };
        t.decide();
        t
    }

    #[test]
    fn gate() {
        // Public build: off from the start, whatever storage says.
        let mut t = Tel::default();
        t.decide();
        assert!(!t.enabled());
        t.push("nav.home", 5.0, 1.0, Map::new());
        assert!(t.take(2.0, 0.0).is_none());
        t.switch = Some(true);
        t.decide();
        assert_eq!(t.on, Some(false));
        // Development build: on, unless switched off.
        let mut d = on();
        assert_eq!(d.on, Some(true));
        d.push("x", 1.0, 1.0, Map::new());
        d.switch = Some(false);
        d.decide();
        assert_eq!(d.on, Some(false));
        assert!(d.buf.is_empty(), "switching off drops what was kept");
    }

    #[test]
    fn batches() {
        let mut t = on();
        assert!(!t.due(1e9));
        for i in 0..(BATCH_MAX + 5) {
            t.push("req.browse", i as f64 + 0.123, 1000.0, Map::new());
        }
        t.push("bad", f64::NAN, 1000.0, Map::new());
        assert!(t.due(1001.0), "full");
        let r = t.take(1001.0, 0.4).expect("batch");
        assert_eq!(r["kind"], "perf");
        assert_eq!(r["schema"], SCHEMA);
        assert_eq!(r["events"].as_array().map(Vec::len), Some(BATCH_MAX));
        assert_eq!(r["events"][1]["v"], 1.1);
        assert_eq!(r["events"][0]["m"], "req.browse");
        assert_eq!(t.buf.len(), 5);
        assert!(!t.due(1002.0), "the rest waits for the next minute");
        assert!(t.due(1001.0 + FLUSH_MS));
        for _ in 0..BUF_MAX {
            t.push("x", 1.0, 1.0, Map::new());
        }
        assert!(t.dropped > 0, "capped while unsent");
    }

    #[test]
    fn navigation_breakdown() {
        let mut t = on();
        t.input("pointer", 100.0);
        assert_eq!(t.nav_start("watch", "push", 103.0), 1);
        t.nav_src("cache");
        t.nav_req(json!({"ep": "next", "ms": 12.0}));
        let marks = [("navigate", 0.0), ("request", 1.0), ("response", 40.0), ("parsed", 43.5), ("rendered", 50.0)];
        let (tok, n, f) = t.nav_inserted(160.0, &marks).expect("open nav");
        assert_eq!((n.input, n.src, n.t0), ("pointer", "cache", 100.0));
        assert_eq!(f["parse_ms"], 3.5);
        assert_eq!(f["render_ms"], 6.5);
        assert_eq!(f["insert_ms"], 60.0);
        assert_eq!(f["handler_ms"], 3.0);
        t.span_with(tok, "nav.watch", n.t0, f);
        assert!(t.nav_inserted(170.0, &marks).is_none(), "one event per navigation");
        t.frame_done(tok, 180.0, 5000.0).expect("span");
        assert_eq!(t.buf[0]["m"], "nav.watch");
        assert_eq!(t.buf[0]["v"], 80.0);
        assert_eq!(t.buf[0]["reqs"][0]["ep"], "next");
        // An input long before: an automatic navigation (autoplay) from now.
        t.input("pointer", 0.0);
        t.nav_start("watch", "push", 5000.0);
        assert_eq!(t.nav.as_ref().map(|n| (n.input, n.t0)), Some(("auto", 5000.0)));
        // Boot: from the document's navigation start.
        t.nav_start("home", "boot", 300.0);
        assert_eq!(t.nav.as_ref().map(|n| (n.input, n.t0)), Some(("boot", 0.0)));
    }

    #[test]
    fn browser_entries_become_events() {
        let mut t = on();
        let js = r#"[{"k":"loaf","t":900,"ms":120.25,"block":70,"scripts":[{"ms":80,"inv":"DOMWindow.onclick","src":"content.js"}]},{"k":"event","t":950,"ms":64,"name":"click","delay":3},{"k":"drop","n":4},{"k":"other","t":1,"ms":1}]"#;
        t.browser_entries(js, 1000.0, 10_000.0, "home");
        assert_eq!(t.buf.len(), 2);
        assert_eq!(t.buf[0]["m"], "lag.loaf");
        assert_eq!(t.buf[0]["v"], 120.3);
        assert_eq!(t.buf[0]["at"], 9900.0);
        assert_eq!(t.buf[0]["route"], "home");
        assert_eq!(t.buf[1]["m"], "lag.event");
        assert_eq!(t.dropped, 4);
        t.browser_entries("not json", 0.0, 0.0, "");
        assert_eq!(t.buf.len(), 2);
    }

    #[test]
    fn spans_are_capped() {
        let mut t = on();
        for i in 0..(SPANS_MAX + 3) {
            t.span("ui.menu", i as f64, Map::new());
        }
        assert_eq!(t.spans.len(), SPANS_MAX);
        assert!(t.frame_done(1, 5.0, 1.0).is_none(), "the oldest was dropped");
    }
}
