//! Voice search glue: turns `features::voice` decisions into bridge calls (the microphone, the socket relay
//! through the background worker, the field's live transcript, the key panel). The Deepgram key never passes
//! through here: the panel's input is stored by `bridge::secret_save` and only the worker reads it.

use super::App;
use crate::bridge;
use crate::features::voice::{self, Act, Src, Voice};
use crate::route::Route;
use crate::util::esc_into;
use std::collections::{HashMap, HashSet};

/// The mic button (its box is the listening pill; `--lv` carries the level).
pub const MIC: &str = "ytc-mic";
const FORM: &str = "ytc-search";
/// The live transcript over the input (finals normal, interim muted).
const LIVE: &str = "ytc-qv";
pub const PANEL: &str = "ytc-vk";
const KEY_IN: &str = "ytc-vk-in";

const MSG_NOKEY: &str = "Paste your Deepgram API key to search by voice.";
const MSG_ON: &str = "Voice search is ready (Deepgram nova-3, Russian and English).";
const MSG_SAVED: &str = "Key saved. Rest the pointer on the mic, or hold ⌥ Option, and speak.";
const MSG_REMOVED: &str = "Key removed. Voice search is off until you paste a new one.";
const MSG_EMPTY: &str = "Paste a key into the field first.";

#[derive(Default)]
pub(super) struct VoiceUi {
    pub v: Voice,
    timers: HashSet<u32>,
    /// Microphone-tail timers after an end → their session.
    tails: HashMap<u32, u32>,
    /// The field's text before dictation (restored when nothing is searched).
    before: String,
    /// What started the live session (decides keyboard vs pointer motion for the search).
    src: Option<Src>,
    pub panel: bool,
    /// Focus is in the panel's key input (Enter saves, Escape closes).
    pub key_focus: bool,
    /// A save was asked (the next `on_secret` answers it).
    saving: bool,
    /// performance.now() when the user ended the live session (0: not ended).
    ended_at: f64,
    /// Last measurements: end → search ms, end → last result ms; dictations this document.
    last: Option<(f64, f64)>,
    last_result: f64,
    count: u32,
}

impl App {
    pub(super) fn voice_boot() {
        bridge::hold_key(voice::HOLD_KEY);
        bridge::ctx_menu();
        bridge::secret_has(voice::KEY);
    }

    fn voice_do(&mut self, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::Arm(ms) => {
                    let id = crate::util::next_id();
                    self.voice.timers.insert(id);
                    bridge::timer(id, ms.max(1.0).ceil() as u32);
                }
                Act::Start(h, src) => {
                    self.voice_panel(false, None);
                    self.voice.before = bridge::input_value("ytc-q");
                    self.voice.src = Some(src);
                    self.voice.ended_at = 0.0;
                    self.voice.count += 1;
                    self.fx_sug_hide();
                    bridge::sock_open(h, voice::URL, voice::PROTOCOLS, voice::KEY, voice::PROBE);
                    bridge::mic_start(h, h, MIC, "--lv");
                    bridge::attr(FORM, "data-v", if src == Src::Hover { "on" } else { "key" });
                    bridge::attr(MIC, "aria-pressed", "true");
                    bridge::mark("ytcVoiceStart");
                    self.voice_paint();
                }
                Act::Flush(h) => {
                    self.voice_ended();
                    // The pill shrinks back at once; the field keeps the words while the last ones arrive (nothing
                    // heard yet: the field looks as before at once).
                    if self.voice.v.t.is_empty() {
                        bridge::attr(FORM, "data-v", "");
                        bridge::value("ytc-q", &self.voice.before);
                    } else {
                        bridge::attr(FORM, "data-v", "end");
                    }
                    bridge::attr(MIC, "aria-pressed", "false");
                    let id = crate::util::next_id();
                    self.voice.tails.insert(id, h);
                    bridge::timer(id, voice::TAIL_MS);
                }
                Act::Stop(h) => {
                    self.voice_close(h);
                    let before = std::mem::take(&mut self.voice.before);
                    bridge::value("ytc-q", &before);
                }
                Act::Search(h, q) => {
                    if self.voice.ended_at == 0.0 {
                        self.voice_ended();
                    }
                    let keyed = self.voice.src != Some(Src::Hover);
                    self.voice_close(h);
                    let now = bridge::now();
                    let res = if self.voice.last_result >= self.voice.ended_at { self.voice.last_result - self.voice.ended_at } else { 0.0 };
                    self.voice.last = Some((now - self.voice.ended_at, res));
                    bridge::mark("ytcVoiceSearch");
                    bridge::value("ytc-q", &q);
                    self.voice.before.clear();
                    // Push-to-talk and the keyboard button search like Enter (no motion); the pointer like a click.
                    if keyed {
                        self.last_key = now;
                    } else {
                        self.last_ptr = now;
                    }
                    self.on_submit("", &q);
                }
                Act::NeedKey => self.voice_panel(true, Some(MSG_NOKEY)),
                Act::Text => self.voice_paint(),
                Act::Warm(q) => {
                    let url = Route::Search { q, sp: None }.url();
                    self.prefetch_page(&url);
                }
                Act::Fail(h, p) => {
                    self.voice_close(h);
                    let before = std::mem::take(&mut self.voice.before);
                    bridge::value("ytc-q", &before);
                    self.voice_panel(true, Some(p.line()));
                }
            }
        }
    }

    fn voice_ended(&mut self) {
        self.voice.ended_at = bridge::now();
        bridge::mark("ytcVoiceEnd");
    }

    /// Microphone and socket off, the field back to its own text.
    fn voice_close(&mut self, h: u32) {
        bridge::mic_stop(h);
        bridge::sock_close(h);
        self.voice.tails.retain(|_, s| *s != h);
        self.voice.src = None;
        bridge::attr(FORM, "data-v", "");
        bridge::attr(MIC, "aria-pressed", "false");
        bridge::html(LIVE, "");
    }

    /// The live transcript: finals in the text colour, interim words muted; the input carries the same text
    /// (that is what a search submits).
    fn voice_paint(&mut self) {
        let t = &self.voice.v.t;
        let mut html = String::from("<span>");
        if t.is_empty() {
            html.push_str("<i class=\"ph\">Listening…</i>");
        } else {
            let f = t.finals_text();
            html.push_str("<b>");
            esc_into(&mut html, &f);
            html.push_str("</b>");
            if !t.interim.is_empty() {
                if !f.is_empty() {
                    html.push(' ');
                }
                html.push_str("<i>");
                esc_into(&mut html, &t.interim);
                html.push_str("</i>");
            }
        }
        html.push_str("</span>");
        bridge::html(LIVE, &html);
        bridge::value("ytc-q", &t.text());
    }

    /// The key panel: `open` with a one-line message (None: the current state), or closed.
    fn voice_panel(&mut self, open: bool, msg: Option<&str>) {
        if !open {
            if self.voice.panel {
                self.voice.panel = false;
                self.voice.key_focus = false;
                bridge::attr(PANEL, "class", if self.keyed() { "vk now" } else { "vk" });
            }
            return;
        }
        let has = self.voice.v.has_key == Some(true);
        let msg = msg.unwrap_or(if has { MSG_ON } else { MSG_NOKEY });
        bridge::html(PANEL, &panel_html(msg, has));
        if !self.voice.panel {
            self.voice.panel = true;
            bridge::attr(PANEL, "class", if self.keyed() { "vk on now" } else { "vk on" });
        }
    }

    pub(super) fn voice_timer(&mut self, id: u32) -> bool {
        if let Some(h) = self.voice.tails.remove(&id) {
            bridge::mic_stop(h);
            bridge::sock_text(h, voice::FINALIZE);
            bridge::sock_text(h, voice::CLOSE);
            return true;
        }
        if !self.voice.timers.remove(&id) {
            return false;
        }
        let acts = self.voice.v.tick(bridge::now());
        self.voice_do(acts);
        true
    }

    /// Hover zones: the mic is `mic`.
    pub(super) fn voice_hov(&mut self, zone: &str, held: bool) {
        let on = zone == "mic";
        if on != self.voice.v.over {
            if on {
                // Learn a key saved in another tab before the dwell ends.
                bridge::secret_has(voice::KEY);
            }
            let acts = self.voice.v.hover(on, held, bridge::now());
            self.voice_do(acts);
        }
    }

    /// `vk-*` clicks: the mic itself, the panel's Save / Remove.
    pub(super) fn voice_click(&mut self, act: &str, kbd: bool) -> bool {
        match act {
            "vk-mic" => {
                let acts = self.voice.v.click(kbd, bridge::now());
                self.voice_do(acts);
            }
            "vk-save" => {
                self.voice.saving = true;
                bridge::secret_save(KEY_IN, voice::KEY);
            }
            "vk-del" => {
                self.voice.saving = true;
                bridge::secret_del(voice::KEY);
            }
            _ => return false,
        }
        true
    }

    /// Keys for voice: Escape cancels a dictation (or closes the panel); Enter in the key field saves.
    pub(super) fn voice_key(&mut self, key: &str) -> bool {
        if key == "Escape" {
            let acts = self.voice.v.escape();
            if !acts.is_empty() {
                self.voice_do(acts);
                return true;
            }
            if self.voice.panel {
                self.voice_panel(false, None);
                bridge::focus(MIC);
                return true;
            }
            return false;
        }
        if key == "Enter" && self.voice.key_focus && self.voice.panel {
            return self.voice_click("vk-save", true);
        }
        false
    }

    pub(super) fn voice_press(&mut self, pop: &str) {
        if self.voice.panel && pop != "vk" {
            self.voice_panel(false, None);
        }
    }

    /// Navigation, tab hidden, page left: stop without searching; the panel closes.
    pub(super) fn voice_leave(&mut self) {
        let acts = self.voice.v.leave();
        self.voice_do(acts);
        self.voice_panel(false, None);
    }

    pub(super) fn voice_debug(&self) -> String {
        let v = &self.voice;
        let key = match v.v.has_key {
            Some(true) => "key set",
            Some(false) => "no key",
            None => "key ?",
        };
        let last = v.last.map(|(s, r)| format!(" · last: end → search {s:.0} ms (last result {r:.0} ms)")).unwrap_or_default();
        format!("voice · {key} · {:?} · {} dictations{last}\n", v.v.phase, v.count)
    }

    pub fn on_sock(&mut self, h: u32, kind: &str, data: &str) {
        let acts = match kind {
            "msg" => {
                if self.voice.v.session() == Some(h) {
                    self.voice.last_result = bridge::now();
                }
                self.voice.v.message(h, data)
            }
            "open" => {
                if self.voice.v.session() == Some(h) {
                    bridge::mark("ytcVoiceOpen");
                }
                vec![]
            }
            "close" => self.voice.v.closed(h),
            "fail" => self.voice.v.failed(h, data),
            _ => vec![],
        };
        self.voice_do(acts);
    }

    pub fn on_mic(&mut self, h: u32, state: &str) {
        if state == "on" && self.voice.v.session() == Some(h) {
            bridge::mark("ytcVoiceMic");
        }
        let acts = self.voice.v.mic(h, state);
        self.voice_do(acts);
    }

    pub fn on_hold(&mut self, key: &str, code: u32) {
        if key != voice::HOLD_KEY {
            return;
        }
        let acts = self.voice.v.hold(code, bridge::now());
        self.voice_do(acts);
    }

    pub fn on_secret(&mut self, key: &str, set: bool, changed: bool) {
        if key != voice::KEY {
            return;
        }
        self.voice.v.has_key = Some(set);
        if std::mem::take(&mut self.voice.saving) && self.voice.panel {
            let msg = match (set, changed) {
                (true, true) => MSG_SAVED,
                (false, true) => MSG_REMOVED,
                _ => MSG_EMPTY,
            };
            self.voice_panel(true, Some(msg));
        }
    }

    /// Right click / the menu key on the mic opens the key panel (again: closes it).
    pub fn on_ctx(&mut self, name: &str, _arg: &str) -> bool {
        if name != "vk" {
            return false;
        }
        // Right-clicking means resting on the mic: a dictation that started meanwhile is not wanted.
        let acts = self.voice.v.context();
        self.voice_do(acts);
        if self.voice.panel {
            self.voice_panel(false, None);
        } else {
            self.voice_panel(true, None);
            bridge::focus(KEY_IN);
        }
        true
    }
}

fn panel_html(msg: &str, has: bool) -> String {
    let mut o = String::with_capacity(900);
    o.push_str("<p class=\"vkm\" id=\"ytc-vk-m\" role=\"status\">");
    esc_into(&mut o, msg);
    o.push_str("</p><div class=\"vkr\"><input id=\"");
    o.push_str(KEY_IN);
    o.push_str("\" type=\"password\" autocomplete=\"off\" spellcheck=\"false\" data-focus=\"vk\" aria-label=\"Deepgram API key\" placeholder=\"");
    o.push_str(if has { "Paste a new key to replace it" } else { "Deepgram API key" });
    o.push_str("\"><button type=\"button\" class=\"vkb\" data-act=\"vk-save\">Save</button></div><p class=\"vkf\">Rest the pointer on the mic or hold ⌥ Option (Alt), speak, then move away or let go to search. Audio goes to Deepgram only while you dictate; the key stays in this browser. <a href=\"https://console.deepgram.com/\" target=\"_blank\" rel=\"noreferrer\">Get a key</a>");
    if has {
        o.push_str(" · <button type=\"button\" class=\"vkd\" data-act=\"vk-del\">Remove key</button>");
    }
    o.push_str("</p>");
    o
}

#[cfg(test)]
mod tests {
    #[test]
    fn panel_markup_never_holds_a_value() {
        let h = super::panel_html("<b>x</b>", true);
        assert!(h.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(h.contains("type=\"password\"") && !h.contains("value="));
        assert!(h.contains("Remove key"));
        assert!(!super::panel_html("m", false).contains("Remove key"));
    }
}
