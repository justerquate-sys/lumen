//! Browser-toolbar controls. Decisions, markup and preference edits stay in Rust.

use crate::{appearance::{Palette, Patch}, bridge, config::{self, Config}, player::sponsor, util::next_id};
use std::cell::{Cell, RefCell};

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<Popup> = RefCell::new(Popup::default());
}

#[derive(Default)]
struct Popup {
    raw: String,
    pending: Patch,
    writing: Option<(u32, Patch)>,
    reading: bool,
    error: bool,
    counts: Option<Vec<u64>>,
    undo: bool,
    message: u32,
}

pub fn active() -> bool { ACTIVE.with(Cell::get) }

fn with(f: impl FnOnce(&mut Popup)) { STATE.with(|s| { if let Ok(mut s) = s.try_borrow_mut() { f(&mut s); } }); }

pub fn boot() {
    ACTIVE.with(|a| a.set(true));
    bridge::doc(&html());
    bridge::storage_watch(config::STORAGE_KEY);
    with(|s| s.read());
    refresh_status();
    bridge::timer(1, 1000);
}

impl Popup {
    fn view(&self) -> Config {
        let written = self.writing.map(|(_, p)| p.apply(&self.raw)).unwrap_or_else(|| self.raw.clone());
        Config::from_json(&self.pending.apply(&written))
    }

    fn read(&mut self) {
        if !self.reading && self.writing.is_none() {
            self.reading = true;
            bridge::storage_get(config::STORAGE_KEY);
        }
    }

    fn sync(&self) {
        let cfg = self.view();
        cfg.palette.apply();
        for palette in [Palette::Lumen, Palette::Youtube] {
            bridge::attr(&format!("palette-{}", palette.key()), "aria-pressed", if cfg.palette == palette { "true" } else { "false" });
        }
        bridge::attr("popup-sponsor", "aria-checked", if cfg.prefs.sb_on { "true" } else { "false" });
        for cat in sponsor::SKIPPABLE {
            let id = format!("popup-cat-{}", cat.key());
            bridge::attr(&id, "aria-pressed", if cfg.prefs.sb_skip & cat.bit() != 0 { "true" } else { "false" });
            let count = self.counts.as_ref().and_then(|c| c.get(cat.idx())).copied().unwrap_or(0);
            bridge::text(&format!("{id}-count"), &if count > 0 { count.to_string() } else { String::new() });
        }
        bridge::attr("popup-undo", "data-visible", if self.undo { "true" } else { "false" });
        bridge::text("popup-status", if self.error { "Couldn’t save preferences. Try again." } else { "" });
    }
}

pub fn on_click(act: &str, arg: &str, keyboard: bool) -> bool {
    bridge::root_attr("data-ytc-popup-input", Some(if keyboard { "keyboard" } else { "pointer" }.into()));
    if act == "popup-undo" {
        message("sponsor-undo");
        return true;
    }
    let mut handled = false;
    with(|s| {
        let cfg = s.view();
        let patch = match act {
            "palette" => match Palette::from_key(arg) { Some(p) => Patch { palette: Some(p), ..Default::default() }, None => return },
            "sponsor-toggle" => Patch { sponsor_on: Some(!cfg.prefs.sb_on), ..Default::default() },
            "sponsor-category" => match sponsor::Cat::from_key(arg).filter(|c| *c != sponsor::Cat::Highlight) {
                Some(c) if cfg.prefs.sb_skip & c.bit() == 0 => Patch { skip_on: c.bit(), ..Default::default() },
                Some(c) => Patch { skip_off: c.bit(), ..Default::default() },
                None => return,
            },
            _ => return,
        };
        handled = true;
        s.error = false;
        s.pending.merge(patch);
        s.sync();
        s.read();
    });
    handled
}

pub fn on_storage(key: &str, raw: &str) {
    if key != config::STORAGE_KEY { return; }
    with(|s| {
        s.raw = raw.into();
        s.reading = false;
        if !s.pending.empty() && s.writing.is_none() {
            let patch = std::mem::take(&mut s.pending);
            let id = next_id();
            s.writing = Some((id, patch));
            bridge::storage_write(id, config::STORAGE_KEY, &patch.apply(&s.raw));
        }
        s.sync();
    });
}

pub fn on_storage_change(key: &str, raw: &str) {
    if key == config::STORAGE_KEY { with(|s| { s.raw = raw.into(); s.sync(); }); }
}

pub fn on_store_result(id: u32, ok: bool) {
    with(|s| {
        let Some((pending_id, patch)) = s.writing else { return };
        if id != pending_id { return; }
        s.writing = None;
        if ok { s.raw = patch.apply(&s.raw); }
        s.error = !ok;
        s.sync();
        if !s.pending.empty() { s.read(); }
    });
    refresh_status();
}

fn message(name: &str) {
    let id = next_id();
    with(|s| s.message = id);
    bridge::active_message(id, name);
}

fn refresh_status() { message("sponsor-status"); }

pub fn on_message_result(id: u32, raw: &str) {
    let v = crate::json::parse_str(raw);
    with(|s| {
        if s.message != id { return; }
        s.counts = v.as_ref().and_then(|v| v.get("counts")).and_then(crate::json::Value::as_array)
            .filter(|a| a.len() == sponsor::SKIPPABLE.len()).map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0).min(64)).collect());
        s.undo = v.as_ref().and_then(|v| v.get("undo")).and_then(crate::json::Value::as_bool).unwrap_or(false);
        s.sync();
    });
}

pub fn on_timer(id: u32) {
    if id == 1 {
        if bridge::visible() { refresh_status(); }
        bridge::timer(1, 1000);
    }
}

fn html() -> String {
    let mut s = String::from(r#"<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Lumen</title><link rel="stylesheet" href="client.css"><link rel="stylesheet" href="popup.css"></head><body id="ytc" class="popup"><main id="ytc-popup"><header><svg viewBox="0 0 28 20" aria-hidden="true"><rect width="28" height="20" rx="5"/><path d="M11 5.5v9l8-4.5z"/></svg><span>Lumen</span></header><section aria-labelledby="palette-heading"><h1 id="palette-heading">Color palette</h1><div class="palette-list" role="group" aria-labelledby="palette-heading">"#);
    for (palette, label, detail) in [(Palette::Lumen, "Lumen", "Charcoal &amp; mint"), (Palette::Youtube, "YouTube", "Graphite, white &amp; red")] {
        let key = palette.key();
        s.push_str(&format!(r#"<button type="button" id="palette-{key}" class="palette-choice {key}" data-act="palette" data-arg="{key}" aria-pressed="false"><i class="palette-preview" aria-hidden="true"><i></i><i></i><i></i></i><span><b>{label}</b><small>{detail}</small></span><svg class="palette-check" viewBox="0 0 20 20" aria-hidden="true"><path d="m4 10 4 4 8-8"/></svg></button>"#));
    }
    s.push_str(r#"</div></section><section aria-labelledby="sponsor-heading"><button type="button" id="popup-sponsor" class="popup-sponsor" data-act="sponsor-toggle" role="switch" aria-checked="true" aria-labelledby="sponsor-heading"><span><b id="sponsor-heading">SponsorSkip</b><small>Skip selected segments automatically</small></span><i class="popup-switch" aria-hidden="true"></i></button><details><summary>Skip categories</summary><div class="popup-categories" role="group" aria-label="Skip categories">"#);
    for c in sponsor::SKIPPABLE {
        let key = c.key();
        s.push_str(&format!(r#"<button type="button" id="popup-cat-{key}" data-act="sponsor-category" data-arg="{key}" aria-pressed="false"><i aria-hidden="true"><svg viewBox="0 0 16 16"><path d="m3 8 3 3 7-7"/></svg></i><span>{}</span><small id="popup-cat-{key}-count"></small></button>"#, c.label()));
    }
    s.push_str(r#"</div></details><button type="button" id="popup-undo" data-act="popup-undo" data-visible="false">Undo last skip</button><p class="popup-source">Segment data from SponsorBlock</p></section><p id="popup-status" role="status" aria-live="polite"></p></main></body>"#);
    s
}
