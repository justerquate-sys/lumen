//! Voice search (2 Nov 2026): dictate a search with Deepgram's streaming speech recognition, by hovering the
//! mic or holding a key, without sound effects. Hovering the mic expands it into a bigger hover area; moving
//! the pointer out runs the search.
//!
//! Pure logic, no bridge calls: when dictation starts and ends (the pointer resting on the mic, a held key,
//! the mic button from the keyboard), the transcript assembled from Deepgram's streaming results, and what
//! is searched. Every input returns the `Act`s the glue (`app/voiceglue.rs`) performs; stale events of an
//! earlier session (another handle `h`) are ignored.

use crate::json::{self, Value};

/// Resting the pointer on the mic this long starts dictation (leaving earlier does nothing).
pub const DWELL_MS: f64 = 200.0;
/// The push-to-talk key must be held alone this long before the microphone opens, so Alt+← (Back) and
/// Option+letter typing never start it.
pub const HOLD_MS: f64 = 250.0;
/// After the end, wait at most this long for Deepgram's last words (Finalize / CloseStream answers).
pub const FLUSH_MS: f64 = 1500.0;
/// The microphone keeps running this long after the end so the last syllable (still in the audio
/// pipeline) reaches the recognizer.
pub const TAIL_MS: u32 = 60;
/// A dictation never runs longer than this.
pub const MAX_MS: f64 = 60_000.0;
/// A pointer click on a listening mic before anything was heard keeps listening (a click meant "start").
pub const CLICK_GRACE_MS: f64 = 1500.0;
/// chrome.storage.local key of the user's Deepgram API key. Only the background worker reads its value
/// (it appends it to the WebSocket subprotocols); the page learns only whether it is set.
pub const KEY: &str = "ytcVoiceKeyV1";
/// The held key (macOS Option): no shortcut of the client uses it, and Ctrl/Cmd+Space are system shortcuts.
pub const HOLD_KEY: &str = "Alt";
/// Deepgram streaming: nova-3 multilingual (mixed-language dictation, e.g. Russian and English), interim results, 300 ms
/// endpointing, raw 16 kHz mono 16-bit PCM (no container: the lowest latency Deepgram documents).
pub const URL: &str = "wss://api.deepgram.com/v1/listen?model=nova-3&language=multi&smart_format=true&interim_results=true&endpointing=300&encoding=linear16&sample_rate=16000&channels=1";
/// Fetched with the key only when the WebSocket handshake fails, to tell a rejected key from a network error.
pub const PROBE: &str = "https://api.deepgram.com/v1/projects";
pub const PROTOCOLS: &str = "[\"token\"]";
pub const FINALIZE: &str = "{\"type\":\"Finalize\"}";
pub const CLOSE: &str = "{\"type\":\"CloseStream\"}";

/// What started a dictation: the pointer (morphs, ends when the pointer leaves), the held key or the
/// focused mic button from the keyboard (instant look, ends on release / a second press).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    Hover,
    Key,
    Button,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Idle,
    /// Waiting for the dwell / hold threshold since `at`.
    Arming { src: Src, at: f64 },
    /// Microphone and socket of session `h` are open.
    Live { src: Src, h: u32, at: f64 },
    /// Ended by the user: waiting (until `until`) for the recognizer's last words, then search.
    Ending { h: u32, until: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    /// getUserMedia refused (the exception name decides the line).
    Blocked,
    NoMic,
    MicOther,
    /// Deepgram rejected the key (handshake failed, the probe answered 401/403).
    BadKey,
    /// Deepgram could not be reached.
    Net,
    /// The socket closed while listening, or the extension was reloaded.
    Dropped,
}

impl Problem {
    pub fn line(self) -> &'static str {
        match self {
            Problem::Blocked => "The microphone is blocked for youtube.com: allow it from the address bar, then try again.",
            Problem::NoMic => "No microphone was found.",
            Problem::MicOther => "The microphone could not be opened. Try again.",
            Problem::BadKey => "Deepgram rejected this key. Paste a new one.",
            Problem::Net => "Couldn't reach Deepgram. Check the connection and try again.",
            Problem::Dropped => "The connection to Deepgram closed. Try again.",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Act {
    /// Call `tick` after this many ms.
    Arm(f64),
    /// Open the socket and the microphone for session `h`; `src` chooses the look.
    Start(u32, Src),
    /// The user ended session `h`: stop the microphone (after `TAIL_MS`), ask Deepgram to flush.
    Flush(u32),
    /// Close session `h` without searching; restore the field.
    Stop(u32),
    /// Close session `h` and search for this.
    Search(u32, String),
    /// No key stored: show the settings panel with its one-line explanation.
    NeedKey,
    /// The transcript changed: repaint the field.
    Text,
    /// A settled phrase: warm the results for it (the search will most likely be this).
    Warm(String),
    /// Close session `h` and show why.
    Fail(u32, Problem),
}

/// Deepgram's results so far: final segments, and the interim words of the segment being spoken.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Transcript {
    pub finals: Vec<String>,
    pub interim: String,
    /// The last result ended an utterance (speech_final, or the answer to Finalize).
    pub settled: bool,
}

/// One `Results` message.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Res {
    pub text: String,
    pub is_final: bool,
    pub speech_final: bool,
    pub from_finalize: bool,
}

/// A Deepgram message: `Some(Results)`, `None` for anything else (Metadata, SpeechStarted, UtteranceEnd,
/// unknown or malformed).
pub fn parse(msg: &str) -> Option<Res> {
    let v = json::parse_str(msg)?;
    if v.get("type").and_then(Value::as_str) != Some("Results") {
        return None;
    }
    let text = v.get("channel").and_then(|c| c.get("alternatives")).and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|a| a.get("transcript")).and_then(Value::as_str).unwrap_or("");
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(Res { text: clean_spaces(text), is_final: flag("is_final"), speech_final: flag("speech_final"), from_finalize: flag("from_finalize") })
}

fn clean_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Transcript {
    pub fn apply(&mut self, r: &Res) {
        if r.is_final {
            if !r.text.is_empty() {
                self.finals.push(r.text.clone());
            }
            self.interim.clear();
        } else {
            self.interim = r.text.clone();
        }
        self.settled = r.is_final && (r.speech_final || r.from_finalize);
    }

    pub fn finals_text(&self) -> String {
        self.finals.join(" ")
    }

    /// Everything heard, the interim words included.
    pub fn text(&self) -> String {
        let f = self.finals_text();
        match (f.is_empty(), self.interim.is_empty()) {
            (_, true) => f,
            (true, false) => self.interim.clone(),
            _ => format!("{f} {}", self.interim),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.finals.is_empty() && self.interim.is_empty()
    }

    /// Nothing is pending: the last result settled an utterance and no interim words wait.
    pub fn ready(&self) -> bool {
        self.settled && self.interim.is_empty() && !self.finals.is_empty()
    }

    /// The search text: what was heard, without smart_format's closing punctuation, at most 500 chars.
    pub fn query(&self) -> String {
        query_of(&self.text())
    }
}

pub fn query_of(s: &str) -> String {
    let s = clean_spaces(s);
    let t = s.trim_end_matches(['.', '?', '!', '…', ',', ';', ':', '。', '？', '！']).trim();
    t.chars().take(500).collect()
}

#[derive(Debug)]
pub struct Voice {
    pub phase: Phase,
    pub t: Transcript,
    /// chrome.storage has a key (None: not answered yet; dictation then tries and learns).
    pub has_key: Option<bool>,
    /// The pointer is on the mic.
    pub over: bool,
    /// The pointer must leave the mic before it can start dictation again (after a click ended one, or the
    /// key panel opened from a dwell).
    pub block_hover: bool,
    next: u32,
}

impl Default for Voice {
    fn default() -> Self {
        Voice { phase: Phase::Idle, t: Transcript::default(), has_key: None, over: false, block_hover: false, next: 0 }
    }
}

impl Voice {
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// The live session's handle (Live or Ending).
    pub fn session(&self) -> Option<u32> {
        match self.phase {
            Phase::Live { h, .. } | Phase::Ending { h, .. } => Some(h),
            _ => None,
        }
    }

    pub fn src(&self) -> Option<Src> {
        match self.phase {
            Phase::Arming { src, .. } | Phase::Live { src, .. } => Some(src),
            _ => None,
        }
    }

    fn start(&mut self, src: Src, now: f64) -> Vec<Act> {
        if self.has_key == Some(false) {
            self.phase = Phase::Idle;
            self.block_hover = src == Src::Hover;
            return vec![Act::NeedKey];
        }
        self.next = self.next.wrapping_add(1).max(1);
        let h = self.next;
        self.t = Transcript::default();
        self.phase = Phase::Live { src, h, at: now };
        vec![Act::Start(h, src), Act::Arm(MAX_MS)]
    }

    /// The user ended session `h` (pointer left, key released, button pressed again).
    fn end(&mut self, h: u32, now: f64) -> Vec<Act> {
        if self.t.ready() {
            return self.finish(h);
        }
        self.phase = Phase::Ending { h, until: now + FLUSH_MS };
        vec![Act::Flush(h), Act::Arm(FLUSH_MS)]
    }

    /// Search with what was heard (nothing heard: nothing happens).
    fn finish(&mut self, h: u32) -> Vec<Act> {
        self.phase = Phase::Idle;
        let q = self.t.query();
        if q.is_empty() { vec![Act::Stop(h)] } else { vec![Act::Search(h, q)] }
    }

    fn stop(&mut self) -> Vec<Act> {
        let out = match self.phase {
            Phase::Live { h, .. } | Phase::Ending { h, .. } => vec![Act::Stop(h)],
            _ => vec![],
        };
        self.phase = Phase::Idle;
        out
    }

    /// The pointer entered (`on`) or left the mic. `held`: a button is down (a drag is not intent).
    pub fn hover(&mut self, on: bool, held: bool, now: f64) -> Vec<Act> {
        let was = self.over;
        self.over = on;
        if !on {
            self.block_hover = false;
        }
        if on == was {
            return vec![];
        }
        match self.phase {
            Phase::Idle if on && !held && !self.block_hover => {
                self.phase = Phase::Arming { src: Src::Hover, at: now };
                vec![Act::Arm(DWELL_MS)]
            }
            Phase::Arming { src: Src::Hover, .. } if !on => {
                self.phase = Phase::Idle;
                vec![]
            }
            Phase::Live { src: Src::Hover, h, .. } if !on => self.end(h, now),
            // Back on the mic while the last words are still awaited, nothing heard: start over.
            Phase::Ending { h, .. } if on && !held && self.t.is_empty() => {
                self.phase = Phase::Arming { src: Src::Hover, at: now };
                vec![Act::Stop(h), Act::Arm(DWELL_MS)]
            }
            _ => vec![],
        }
    }

    /// The held key: 1 pressed alone, 0 released, 2 another key or a button joined (cancel), 3 focus lost.
    pub fn hold(&mut self, code: u32, now: f64) -> Vec<Act> {
        match (self.phase, code) {
            (Phase::Idle, 1) => {
                self.phase = Phase::Arming { src: Src::Key, at: now };
                vec![Act::Arm(HOLD_MS)]
            }
            (Phase::Arming { src: Src::Key, .. }, _) if code != 1 => {
                self.phase = Phase::Idle;
                vec![]
            }
            (Phase::Live { src: Src::Key, h, .. }, 0) => self.end(h, now),
            (Phase::Live { src: Src::Key, .. }, 2 | 3) => self.stop(),
            _ => vec![],
        }
    }

    /// A timer armed by `Act::Arm` fired (stale ones are harmless: everything is decided by time).
    pub fn tick(&mut self, now: f64) -> Vec<Act> {
        match self.phase {
            Phase::Arming { src, at } => {
                let need = if src == Src::Key { HOLD_MS } else { DWELL_MS };
                if now - at >= need - 1.0 { self.start(src, now) } else { vec![Act::Arm(need - (now - at))] }
            }
            Phase::Live { h, at, .. } if now - at >= MAX_MS - 1.0 => self.end(h, now),
            Phase::Ending { h, until } if now >= until - 1.0 => self.finish(h),
            _ => vec![],
        }
    }

    /// The mic button was activated: `kbd` Enter/Space toggles a dictation (instant look); a pointer click
    /// starts at once, or ends a listening one that already heard something.
    pub fn click(&mut self, kbd: bool, now: f64) -> Vec<Act> {
        match self.phase {
            Phase::Idle | Phase::Arming { .. } => self.start(if kbd { Src::Button } else { Src::Hover }, now),
            Phase::Live { h, at, src } => {
                if !kbd && self.t.is_empty() && now - at < CLICK_GRACE_MS {
                    return vec![];
                }
                if src == Src::Hover {
                    self.block_hover = true;
                }
                self.end(h, now)
            }
            Phase::Ending { .. } => vec![],
        }
    }

    /// Escape: cancel without searching. Empty when nothing was running (the key is not ours).
    pub fn escape(&mut self) -> Vec<Act> {
        if self.phase == Phase::Idle {
            return vec![];
        }
        if matches!(self.phase, Phase::Arming { .. }) {
            self.phase = Phase::Idle;
            // Handled (so the key does nothing else), nothing to close.
            return vec![Act::Text];
        }
        if self.src() == Some(Src::Hover) {
            self.block_hover = true;
        }
        self.stop()
    }

    /// The mic's context menu (the key panel): cancel what the resting pointer started, no new dwell until it
    /// leaves.
    pub fn context(&mut self) -> Vec<Act> {
        self.block_hover = self.over;
        if matches!(self.phase, Phase::Arming { .. }) {
            self.phase = Phase::Idle;
        }
        self.stop()
    }

    /// Navigation, tab hidden, page left: stop without searching.
    pub fn leave(&mut self) -> Vec<Act> {
        if matches!(self.phase, Phase::Arming { .. }) {
            self.phase = Phase::Idle;
        }
        self.stop()
    }

    /// A text message from the socket of session `h`.
    pub fn message(&mut self, h: u32, msg: &str) -> Vec<Act> {
        if self.session() != Some(h) {
            return vec![];
        }
        let Some(r) = parse(msg) else { return vec![] };
        let before = self.t.clone();
        self.t.apply(&r);
        let mut out = Vec::new();
        if self.t != before {
            out.push(Act::Text);
        }
        if self.t.settled && r.speech_final && !self.t.finals.is_empty() {
            let q = query_of(&self.t.finals_text());
            if q.chars().count() >= 2 {
                out.push(Act::Warm(q));
            }
        }
        if matches!(self.phase, Phase::Ending { .. }) && r.from_finalize {
            out.extend(self.finish(h));
        }
        out
    }

    /// The socket of session `h` closed after it had opened (`CloseStream` answered, or Deepgram dropped it).
    pub fn closed(&mut self, h: u32) -> Vec<Act> {
        if self.session() != Some(h) {
            return vec![];
        }
        match self.phase {
            Phase::Ending { .. } => self.finish(h),
            _ => {
                self.phase = Phase::Idle;
                vec![Act::Fail(h, Problem::Dropped)]
            }
        }
    }

    /// The relay could not open session `h`'s socket: `nokey`, `key` (rejected), `net`, `ext`, `url`.
    pub fn failed(&mut self, h: u32, why: &str) -> Vec<Act> {
        if self.session() != Some(h) {
            return vec![];
        }
        self.phase = Phase::Idle;
        match why {
            "nokey" => {
                self.has_key = Some(false);
                vec![Act::Stop(h), Act::NeedKey]
            }
            "key" => vec![Act::Fail(h, Problem::BadKey)],
            "net" => vec![Act::Fail(h, Problem::Net)],
            _ => vec![Act::Fail(h, Problem::Dropped)],
        }
    }

    /// The microphone of session `h`: `on`, or `error:<DOMException name>`.
    pub fn mic(&mut self, h: u32, state: &str) -> Vec<Act> {
        if self.session() != Some(h) || state == "on" {
            return vec![];
        }
        // An error after the user already ended it: the words heard so far still count.
        if matches!(self.phase, Phase::Ending { .. }) {
            return vec![];
        }
        self.phase = Phase::Idle;
        let p = match state.strip_prefix("error:").unwrap_or(state) {
            "NotAllowedError" | "SecurityError" => Problem::Blocked,
            "NotFoundError" | "OverconstrainedError" => Problem::NoMic,
            _ => Problem::MicOther,
        };
        vec![Act::Fail(h, p)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(text: &str, is_final: bool, speech_final: bool, from_finalize: bool) -> String {
        format!(
            "{{\"type\":\"Results\",\"channel_index\":[0,1],\"duration\":1.0,\"start\":0.0,\"is_final\":{is_final},\"speech_final\":{speech_final},\"from_finalize\":{from_finalize},\"channel\":{{\"alternatives\":[{{\"transcript\":{},\"confidence\":0.9,\"words\":[]}}]}}}}",
            serde_json::Value::String(text.into())
        )
    }

    fn live(v: &mut Voice, src: Src) -> u32 {
        match src {
            Src::Hover => {
                // (each helper session starts with the pointer arriving)
                v.over = false;
                v.block_hover = false;
                assert_eq!(v.hover(true, false, 0.0), vec![Act::Arm(DWELL_MS)]);
                v.tick(DWELL_MS);
            }
            Src::Key => {
                assert_eq!(v.hold(1, 0.0), vec![Act::Arm(HOLD_MS)]);
                v.tick(HOLD_MS);
            }
            Src::Button => {
                v.click(true, 0.0);
            }
        }
        v.session().expect("live")
    }

    #[test]
    fn parses_results_and_ignores_the_rest() {
        let r = parse(&res("  hello   world ", true, true, false)).unwrap();
        assert_eq!(r, Res { text: "hello world".into(), is_final: true, speech_final: true, from_finalize: false });
        assert_eq!(parse("{\"type\":\"Metadata\",\"request_id\":\"x\"}"), None);
        assert_eq!(parse("{\"type\":\"SpeechStarted\"}"), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse("{\"type\":\"Results\"}").unwrap().text, "");
    }

    #[test]
    fn transcript_assembles_finals_and_interim() {
        let mut t = Transcript::default();
        t.apply(&parse(&res("Как сделать", false, false, false)).unwrap());
        assert_eq!(t.text(), "Как сделать");
        assert!(!t.ready());
        t.apply(&parse(&res("Как сделать игру", true, false, false)).unwrap());
        t.apply(&parse(&res("в Roblox", false, false, false)).unwrap());
        assert_eq!(t.text(), "Как сделать игру в Roblox");
        t.apply(&parse(&res("в Roblox?", true, true, false)).unwrap());
        assert!(t.ready());
        assert_eq!(t.query(), "Как сделать игру в Roblox");
        assert_eq!(query_of(" Roblox scripting tutorial. "), "Roblox scripting tutorial");
        assert_eq!(query_of("..."), "");
        assert_eq!(query_of(&"a".repeat(600)).len(), 500);
    }

    #[test]
    fn short_hover_does_nothing_and_dwell_starts() {
        let mut v = Voice::default();
        assert_eq!(v.hover(true, false, 0.0), vec![Act::Arm(DWELL_MS)]);
        assert_eq!(v.hover(false, false, 120.0), vec![]);
        assert_eq!(v.tick(200.0), vec![]);
        assert_eq!(v.phase, Phase::Idle);
        // A drag passing over the mic is not intent.
        assert_eq!(v.hover(true, true, 300.0), vec![]);
        v.hover(false, false, 310.0);
        v.hover(true, false, 400.0);
        assert_eq!(v.tick(500.0), vec![Act::Arm(100.0)]);
        assert_eq!(v.tick(600.0), vec![Act::Start(1, Src::Hover), Act::Arm(MAX_MS)]);
    }

    #[test]
    fn hover_out_searches_at_once_when_settled() {
        let mut v = Voice::default();
        let h = live(&mut v, Src::Hover);
        assert_eq!(v.message(h, &res("roblox", false, false, false)), vec![Act::Text]);
        assert_eq!(v.message(h, &res("Roblox scripting.", true, true, false)), vec![Act::Text, Act::Warm("Roblox scripting".into())]);
        assert_eq!(v.hover(false, false, 2000.0), vec![Act::Search(h, "Roblox scripting".into())]);
        assert_eq!(v.phase, Phase::Idle);
    }

    #[test]
    fn hover_out_with_pending_words_flushes_then_searches() {
        let mut v = Voice::default();
        let h = live(&mut v, Src::Hover);
        v.message(h, &res("hello wor", false, false, false));
        assert_eq!(v.hover(false, false, 1000.0), vec![Act::Flush(h), Act::Arm(FLUSH_MS)]);
        // An interim during the wait updates the field; the Finalize answer searches.
        assert_eq!(v.message(h, &res("hello world", false, false, false)), vec![Act::Text]);
        assert_eq!(v.message(h, &res("Hello world.", true, false, true)), vec![Act::Text, Act::Search(h, "Hello world".into())]);
    }

    #[test]
    fn flush_timeout_uses_what_was_heard_and_nothing_heard_does_nothing() {
        let mut v = Voice::default();
        let h = live(&mut v, Src::Hover);
        v.message(h, &res("partial words", false, false, false));
        v.hover(false, false, 1000.0);
        assert_eq!(v.tick(1000.0 + FLUSH_MS - 200.0), vec![]);
        assert_eq!(v.tick(1000.0 + FLUSH_MS), vec![Act::Search(h, "partial words".into())]);
        // Silence: the pill shrinks back, nothing is searched.
        let h = live(&mut v, Src::Hover);
        v.hover(false, false, 500.0);
        assert_eq!(v.closed(h), vec![Act::Stop(h)]);
    }

    #[test]
    fn push_to_talk() {
        let mut v = Voice::default();
        // A quick tap (Alt+← for Back) never opens the microphone.
        v.hold(1, 0.0);
        assert_eq!(v.hold(2, 80.0), vec![]);
        assert_eq!(v.tick(HOLD_MS), vec![]);
        v.hold(1, 1000.0);
        assert_eq!(v.hold(0, 1100.0), vec![]);
        assert_eq!(v.phase, Phase::Idle);
        let h = live(&mut v, Src::Key);
        assert_eq!(v.src(), Some(Src::Key));
        // Hovering in and out of the mic does not end a key dictation.
        assert_eq!(v.hover(true, false, 300.0), vec![]);
        assert_eq!(v.hover(false, false, 400.0), vec![]);
        v.message(h, &res("лучшие игры", false, false, false));
        assert_eq!(v.hold(0, 900.0), vec![Act::Flush(h), Act::Arm(FLUSH_MS)]);
        assert_eq!(v.message(h, &res("Лучшие игры.", true, true, true)), vec![Act::Text, Act::Warm("Лучшие игры".into()), Act::Search(h, "Лучшие игры".into())]);
        // A chord while talking cancels.
        let h = live(&mut v, Src::Key);
        v.message(h, &res("oops", false, false, false));
        assert_eq!(v.hold(2, 500.0), vec![Act::Stop(h)]);
    }

    #[test]
    fn escape_navigation_and_stale_events() {
        let mut v = Voice::default();
        assert_eq!(v.escape(), vec![]);
        let h = live(&mut v, Src::Hover);
        v.message(h, &res("cancel me", false, false, false));
        assert_eq!(v.escape(), vec![Act::Stop(h)]);
        // The pointer is still on the mic: no new dwell until it leaves.
        assert!(v.block_hover);
        assert_eq!(v.message(h, &res("late", true, true, false)), vec![]);
        v.hover(false, false, 10.0);
        assert!(!v.block_hover);
        let h2 = live(&mut v, Src::Hover);
        assert_ne!(h, h2);
        assert_eq!(v.message(h, &res("old session", true, true, false)), vec![]);
        assert_eq!(v.leave(), vec![Act::Stop(h2)]);
        assert_eq!(v.leave(), vec![]);
    }

    #[test]
    fn missing_key_and_failures() {
        let mut v = Voice { has_key: Some(false), ..Default::default() };
        v.hover(true, false, 0.0);
        assert_eq!(v.tick(DWELL_MS), vec![Act::NeedKey]);
        assert!(v.block_hover && v.phase == Phase::Idle);
        // Unknown key state: try; the relay answers nokey.
        let mut v = Voice::default();
        let h = live(&mut v, Src::Key);
        assert_eq!(v.failed(h, "nokey"), vec![Act::Stop(h), Act::NeedKey]);
        assert_eq!(v.has_key, Some(false));
        v.has_key = Some(true);
        let h = live(&mut v, Src::Hover);
        assert_eq!(v.failed(h, "key"), vec![Act::Fail(h, Problem::BadKey)]);
        let h = live(&mut v, Src::Hover);
        assert_eq!(v.mic(h, "on"), vec![]);
        assert_eq!(v.mic(h, "error:NotAllowedError"), vec![Act::Fail(h, Problem::Blocked)]);
        let h = live(&mut v, Src::Hover);
        assert_eq!(v.closed(h), vec![Act::Fail(h, Problem::Dropped)]);
    }

    #[test]
    fn clicks_and_keyboard_button() {
        let mut v = Voice::default();
        // A click before the dwell ends starts at once.
        v.hover(true, false, 0.0);
        let a = v.click(false, 50.0);
        assert!(matches!(a[0], Act::Start(_, Src::Hover)));
        let h = v.session().unwrap();
        // A click meaning "start" right after the dwell started it keeps listening.
        assert_eq!(v.click(false, 300.0), vec![]);
        v.message(h, &res("Minecraft.", true, true, false));
        assert_eq!(v.click(false, 900.0), vec![Act::Search(h, "Minecraft".into())]);
        assert!(v.block_hover);
        // Keyboard: Enter starts, Enter again ends.
        let mut v = Voice::default();
        let h = live(&mut v, Src::Button);
        assert_eq!(v.src(), Some(Src::Button));
        assert_eq!(v.click(true, 100.0), vec![Act::Flush(h), Act::Arm(FLUSH_MS)]);
    }

    #[test]
    fn context_menu_cancels_what_resting_started() {
        let mut v = Voice::default();
        v.hover(true, false, 0.0);
        assert_eq!(v.context(), vec![]);
        assert_eq!(v.tick(DWELL_MS), vec![]);
        assert!(v.block_hover);
        let h = live(&mut v, Src::Hover);
        assert_eq!(v.context(), vec![Act::Stop(h)]);
        assert!(v.block_hover && v.phase == Phase::Idle);
    }

    #[test]
    fn dictation_is_capped() {
        let mut v = Voice::default();
        let h = live(&mut v, Src::Key);
        v.message(h, &res("long talk", true, true, false));
        assert_eq!(v.tick(MAX_MS + HOLD_MS), vec![Act::Search(h, "long talk".into())]);
    }

    #[test]
    fn the_url_matches_what_is_sent() {
        assert!(URL.starts_with("wss://api.deepgram.com/v1/listen?"));
        for p in ["model=nova-3", "language=multi", "smart_format=true", "interim_results=true", "endpointing=300", "encoding=linear16", "sample_rate=16000", "channels=1"] {
            assert!(URL.contains(p), "{p}");
        }
        assert!(PROBE.starts_with("https://api.deepgram.com/"));
    }
}
