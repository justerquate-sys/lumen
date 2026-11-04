//! Bounded appearance and SponsorSkip preferences. Patches preserve the rest of the config.

use crate::{config::Config, player::sponsor};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Palette {
    #[default]
    Lumen,
    Youtube,
}

impl Palette {
    pub fn key(self) -> &'static str {
        match self { Self::Lumen => "lumen", Self::Youtube => "youtube" }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key { "lumen" => Some(Self::Lumen), "youtube" => Some(Self::Youtube), _ => None }
    }

    pub fn apply(self) {
        crate::bridge::root_attr("data-ytc-palette", Some(self.key().into()));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    pub palette: Option<Palette>,
    pub sponsor_on: Option<bool>,
    pub skip_on: u16,
    pub skip_off: u16,
}

impl Patch {
    pub fn empty(self) -> bool { self == Self::default() }

    pub fn merge(&mut self, next: Self) {
        if next.palette.is_some() { self.palette = next.palette; }
        if next.sponsor_on.is_some() { self.sponsor_on = next.sponsor_on; }
        self.skip_on = (self.skip_on & !next.skip_off) | next.skip_on;
        self.skip_off = (self.skip_off & !next.skip_on) | next.skip_off;
    }

    /// Re-read before writing; change only explicitly edited fields and category bits.
    pub fn apply(self, stored: &str) -> String {
        let mut v: Value = serde_json::from_str(stored).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
        if let Some(palette) = self.palette {
            if palette == Palette::Lumen { v.as_object_mut().unwrap().remove("theme"); }
            else { v["theme"] = palette.key().into(); }
        }
        if self.sponsor_on.is_some() || self.skip_on != 0 || self.skip_off != 0 {
            let old = Config::from_json(stored).prefs;
            if !v.get("p").is_some_and(Value::is_object) { v["p"] = json!({}); }
            let p = v["p"].as_object_mut().unwrap();
            if let Some(on) = self.sponsor_on {
                if on { p.remove("sbo"); } else { p.insert("sbo".into(), false.into()); }
            }
            if self.skip_on != 0 || self.skip_off != 0 {
                let mask = (old.sb_skip | self.skip_on) & !self.skip_off;
                if mask == sponsor::default_mask() { p.remove("sb"); }
                else { p.insert("sb".into(), sponsor::mask_to_keys(mask).into()); }
            }
        }
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_preserve_unrelated_and_concurrent_preferences() {
        let old = r#"{"v":"2.20261030.01.00","vd":"visitor","au":2,"future":7,"p":{"vol":0.3,"cc":false,"cs":{"v":2,"x":0.2},"future":"kept"}}"#;
        let patch = Patch { palette: Some(Palette::Youtube), sponsor_on: Some(false), skip_on: sponsor::Cat::Intro.bit(), ..Default::default() };
        let v: Value = serde_json::from_str(&patch.apply(old)).unwrap();
        assert_eq!(v["theme"], "youtube");
        assert_eq!(v["future"], 7);
        assert_eq!(v["p"]["vol"], 0.3);
        assert_eq!(v["p"]["cc"], false);
        assert_eq!(v["p"]["cs"]["x"], 0.2);
        assert_eq!(v["p"]["future"], "kept");
        assert_eq!(v["p"]["sbo"], false);
        let cfg = Config::from_json(&v.to_string());
        assert_eq!(cfg.palette, Palette::Youtube);
        assert_ne!(cfg.prefs.sb_skip & sponsor::Cat::Intro.bit(), 0);
        let concurrent = r#"{"p":{"vol":0.8,"sb":"sponsor,selfpromo,interaction,outro"}}"#;
        let changed = patch.apply(concurrent);
        let cfg = Config::from_json(&changed);
        assert_eq!(cfg.prefs.volume, 0.8);
        assert_ne!(cfg.prefs.sb_skip & sponsor::Cat::Outro.bit(), 0);
        assert_ne!(cfg.prefs.sb_skip & sponsor::Cat::Intro.bit(), 0);
    }

    #[test]
    fn latest_edits_win_and_defaults_stay_compact() {
        let mut p = Patch { palette: Some(Palette::Youtube), sponsor_on: Some(false), skip_on: sponsor::Cat::Intro.bit(), ..Default::default() };
        p.merge(Patch { palette: Some(Palette::Lumen), sponsor_on: Some(true), skip_off: sponsor::Cat::Intro.bit(), ..Default::default() });
        let v: Value = serde_json::from_str(&p.apply(r#"{"theme":"youtube","p":{"sbo":false,"sb":"sponsor,selfpromo,interaction,intro"}}"#)).unwrap();
        assert!(v.get("theme").is_none());
        assert!(v["p"].get("sbo").is_none());
        assert!(v["p"].get("sb").is_none());
        assert!(Palette::from_key("other").is_none());
        assert_eq!(Config::from_json(r#"{"theme":"other"}"#).palette, Palette::Lumen);
    }
}
