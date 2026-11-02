//! Storyboards: the frame sprite sheets YouTube serves for seek previews.
//!
//! The WEB `player` response carries
//! `storyboards.playerStoryboardSpecRenderer.spec`, a `|`-separated string:
//! the sheet URL template, then one level per field,
//! `width#height#count#cols#rows#interval_ms#name#sigh`, level `i` being `L{i}`
//! in the template. A sheet is a `cols × rows` grid of `width × height` frames,
//! frame `n` taken at `n × interval` (interval 0 = `count` frames spread over the
//! whole video, one sheet). `$L` → the level, `$N` → the name with `$M` → the
//! sheet number (`M0`, `M1`…), plus `&sigh=` (checked 2 Nov 2026 on
//! `kCc8FmEb1nY` and `aqz-KE-bpKQ`: `storyboard3_L3/M77.jpg`, WebP). The last
//! sheet is cropped to its frames: `min(n, cols)` wide, `ceil(n / cols)` tall.
//!
//! Frames are shown as a CSS background into the sheet (`style`); the browser
//! caches the sheets, nothing is kept by us. Live storyboards
//! (`playerLiveStoryboardSpecRenderer`, sheets of the latest segments) are not used.

use crate::json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub w: u32,
    pub h: u32,
    pub count: u32,
    pub cols: u32,
    pub rows: u32,
    /// 0 = `count` frames spread evenly over the video.
    pub interval_ms: u32,
    pub name: String,
    pub sigh: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Storyboard {
    pub base: String,
    pub levels: Vec<Level>,
    /// Video length (s), for interval-0 levels.
    pub dur: f64,
}

/// One frame: its sheet and its cell there.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub url: String,
    /// Frame number in the level.
    pub n: u32,
    pub col: u32,
    pub row: u32,
    /// The sheet's grid (the last sheet is cropped).
    pub cols: u32,
    pub rows: u32,
    pub w: u32,
    pub h: u32,
}

/// Only YouTube's image host, and nothing that could leave a CSS `url("…")`.
fn safe_url(u: &str) -> bool {
    u.starts_with("https://i.ytimg.com/") && !u.bytes().any(|b| b <= b' ' || matches!(b, b'"' | b'\\' | b'\'' | b'(' | b')' | b'<' | b'>') || b >= 0x7f)
}

/// Parses a `playerStoryboardSpecRenderer.spec`.
pub fn parse(spec: &str, dur: f64) -> Option<Storyboard> {
    let mut parts = spec.split('|');
    let base = parts.next()?.to_string();
    if !safe_url(&base) {
        return None;
    }
    let mut levels = Vec::new();
    for p in parts {
        let f: Vec<&str> = p.split('#').collect();
        if f.len() < 7 {
            return None;
        }
        let n = |i: usize| f[i].parse::<u32>().ok();
        let lv = Level {
            w: n(0)?,
            h: n(1)?,
            count: n(2)?,
            cols: n(3)?,
            rows: n(4)?,
            interval_ms: n(5)?,
            name: f[6].to_string(),
            sigh: f.get(7).map_or(String::new(), |s| s.to_string()),
        };
        if lv.w == 0 || lv.h == 0 || lv.count == 0 || lv.cols == 0 || lv.rows == 0 {
            return None;
        }
        levels.push(lv);
    }
    if levels.is_empty() {
        return None;
    }
    let sb = Storyboard { base, levels, dur };
    // Every sheet URL it can build must be safe too.
    (0..sb.levels.len()).all(|l| safe_url(&sb.url(l, 0))).then_some(sb)
}

/// The VOD storyboard of a WEB `player` response (`None` for live streams or without one).
pub fn from_player(player: &Value) -> Option<Storyboard> {
    let spec = player.pointer("/storyboards/playerStoryboardSpecRenderer/spec").and_then(Value::as_str)?;
    let dur = player.pointer("/videoDetails/lengthSeconds").and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    parse(spec, dur)
}

impl Storyboard {
    /// The sheet URL of `level`, sheet `page`.
    pub fn url(&self, level: usize, page: u32) -> String {
        let lv = &self.levels[level];
        let name = lv.name.replace("$M", &page.to_string());
        let mut u = self.base.replace("$L", &level.to_string()).replace("$N", &name);
        if !lv.sigh.is_empty() {
            u.push(if u.contains('?') { '&' } else { '?' });
            u.push_str("sigh=");
            u.push_str(&lv.sigh);
        }
        u
    }

    /// The level to show a frame `css_w` CSS px wide at device pixel ratio `dpr`:
    /// the smallest whose frames are at least that many device pixels wide, else
    /// the largest (sharp at 2× when YouTube has it).
    pub fn level_for(&self, css_w: f64, dpr: f64) -> usize {
        let need = css_w * dpr.max(1.0);
        let by_w = |a: &usize, b: &usize| self.levels[*a].w.cmp(&self.levels[*b].w);
        let all = 0..self.levels.len();
        all.clone().filter(|&i| self.levels[i].w as f64 >= need - 0.5).min_by(by_w).or_else(|| all.max_by(by_w)).unwrap_or(0)
    }

    /// Seconds between two frames of `level`.
    fn step(&self, level: usize, dur: f64) -> f64 {
        let lv = &self.levels[level];
        if lv.interval_ms > 0 {
            lv.interval_ms as f64 / 1000.0
        } else if dur > 0.0 {
            dur / lv.count as f64
        } else {
            0.0
        }
    }

    fn dur(&self, dur: f64) -> f64 {
        if self.dur > 0.0 { self.dur } else { dur }
    }

    /// The frame shown for time `t` (the last frame taken at or before it,
    /// like YouTube's seek preview). `dur` = the player's duration when the
    /// response had no length.
    pub fn frame_at(&self, level: usize, t: f64, dur: f64) -> Option<Cell> {
        let step = self.step(level, self.dur(dur));
        if step <= 0.0 || !t.is_finite() {
            return None;
        }
        // A hair of tolerance so an exact boundary (20 s / 10 s) is frame 2, not 1.999….
        Some(self.cell(level, ((t.max(0.0) + 1e-6) / step).floor() as u32))
    }

    /// The frame for a chapter `[start, end)`: the first one taken inside it
    /// (a frame from the end of the previous chapter would be the wrong
    /// picture), else the one at its start.
    pub fn frame_in(&self, level: usize, start: f64, end: f64, dur: f64) -> Option<Cell> {
        let step = self.step(level, self.dur(dur));
        if step <= 0.0 {
            return None;
        }
        let first = ((start.max(0.0) - 1e-6) / step).ceil().max(0.0);
        let n = if first * step < end - 1e-6 { first } else { ((start.max(0.0) + 1e-6) / step).floor() };
        Some(self.cell(level, n as u32))
    }

    fn cell(&self, level: usize, n: u32) -> Cell {
        let lv = &self.levels[level];
        let n = n.min(lv.count - 1);
        let per = lv.cols * lv.rows;
        let page = n / per;
        let k = n % per;
        // The last sheet only holds what is left.
        let left = (lv.count - page * per).min(per);
        Cell {
            url: self.url(level, page),
            n,
            col: k % lv.cols,
            row: k / lv.cols,
            cols: left.min(lv.cols),
            rows: left.div_ceil(lv.cols),
            w: lv.w,
            h: lv.h,
        }
    }
}

/// Inline style for a frame's picture element (absolutely placed inside a
/// 16:9 box, client.css `.sbf`): letterboxed to the frame's own shape, the
/// sheet sized and positioned in percentages so the cell fills it exactly at
/// any box size.
pub fn style(c: &Cell) -> String {
    let k = (c.w as f64 / c.h as f64) / (16.0 / 9.0);
    let (l, t, w, h) = if (k - 1.0).abs() < 0.02 {
        (0.0, 0.0, 100.0, 100.0)
    } else if k < 1.0 {
        ((1.0 - k) * 50.0, 0.0, k * 100.0, 100.0)
    } else {
        (0.0, (1.0 - 1.0 / k) * 50.0, 100.0, 100.0 / k)
    };
    let pos = |i: u32, n: u32| if n > 1 { i as f64 * 100.0 / (n - 1) as f64 } else { 0.0 };
    format!(
        "left:{l:.3}%;top:{t:.3}%;width:{w:.3}%;height:{h:.3}%;background:url(\"{}\") {:.4}% {:.4}%/{}% {}% no-repeat",
        c.url,
        pos(c.col, c.cols),
        pos(c.row, c.rows),
        c.cols * 100,
        c.rows * 100
    )
}

/// Inline style for the same element showing a plain picture (YouTube's chapter
/// thumbnail when there is no storyboard).
pub fn thumb_style(url: &str) -> Option<String> {
    let ok = (url.starts_with("https://i.ytimg.com/") || url.starts_with("https://i9.ytimg.com/")) && !url.bytes().any(|b| b <= b' ' || matches!(b, b'"' | b'\\' | b'\'' | b'(' | b')' | b'<' | b'>') || b >= 0x7f);
    ok.then(|| format!("left:0;top:0;width:100%;height:100%;background:url(\"{url}\") 50% 50%/cover no-repeat"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Big Buck Bunny (aqz-KE-bpKQ), 634 s, 2 Nov 2026: 128 frames every 5 s; L3's last sheet holds 2 frames.
    const BBB: &str = "https://i.ytimg.com/sb/aqz-KE-bpKQ/storyboard3_L$L/$N.jpg?sqp=-oaymwENSDfyq4qpAwVwAcABBqLzl_8DBgjEj7SoBg==|48#27#100#10#10#0#default#rs$AOn4CLC4g9l8goJa9smHYdV-TlPbLIS9SA|80#45#128#10#10#5000#M$M#rs$AOn4CLAze14Ic8NjshmJJmRAU31ljKUK7A|160#90#128#5#5#5000#M$M#rs$AOn4CLC9nRwSugHSXO3o99N1OLx6MOkrlA|320#180#128#3#3#5000#M$M#rs$AOn4CLAsookXQ4X4w2tP5AdvrFx5JZA5gw";

    fn gpt() -> Storyboard {
        from_player(&crate::util::test_fixture("player_web_storyboard.json")).expect("storyboard")
    }

    #[test]
    fn parses_the_real_web_spec() {
        let sb = gpt();
        assert_eq!(sb.dur, 6980.0);
        assert_eq!(sb.levels.len(), 4);
        assert_eq!(sb.levels[0], Level { w: 48, h: 27, count: 100, cols: 10, rows: 10, interval_ms: 0, name: "default".into(), sigh: "rs$AOn4CLDSS2GGRMImsH19WN2e_EE0X6Ww2A".into() });
        assert_eq!((sb.levels[3].w, sb.levels[3].h, sb.levels[3].count, sb.levels[3].cols, sb.levels[3].rows, sb.levels[3].interval_ms), (320, 180, 699, 3, 3, 10000));
        assert_eq!(
            sb.url(3, 77),
            "https://i.ytimg.com/sb/kCc8FmEb1nY/storyboard3_L3/M77.jpg?sqp=-oaymwENSDfyq4qpAwVwAcABBqLzl_8DBgj-s42oBg==&sigh=rs$AOn4CLBQzJikOYA_GtOEYLuA2AGeV9Yw4w"
        );
        assert_eq!(sb.url(0, 0), "https://i.ytimg.com/sb/kCc8FmEb1nY/storyboard3_L0/default.jpg?sqp=-oaymwENSDfyq4qpAwVwAcABBqLzl_8DBgj-s42oBg==&sigh=rs$AOn4CLDSS2GGRMImsH19WN2e_EE0X6Ww2A");
        // Live and missing storyboards: none.
        let live = crate::json::parse_str(r#"{"storyboards":{"playerLiveStoryboardSpecRenderer":{"spec":"https://i.ytimg.com/sb/x/storyboard_live_90_2x2_b2/M$M.jpg?rs=A#159#90#2#2"}}}"#).unwrap();
        assert_eq!(from_player(&live), None);
        assert_eq!(from_player(&crate::util::test_fixture("player.json")), None);
    }

    #[test]
    fn rejects_unsafe_or_broken_specs() {
        assert_eq!(parse("https://evil.example/sb/$L/$N.jpg|80#45#10#5#5#1000#M$M#s", 50.0), None, "other hosts");
        assert_eq!(parse("https://i.ytimg.com/sb/x\")/$N.jpg|80#45#10#5#5#1000#M$M#s", 50.0), None, "CSS breakout");
        assert_eq!(parse("https://i.ytimg.com/sb/x/$N.jpg|80#45#10#5#5#1000#M$M#s)x", 50.0), None, "breakout in sigh");
        assert_eq!(parse("https://i.ytimg.com/sb/x/$N.jpg|80#45#0#5#5#1000#M$M#s", 50.0), None, "no frames");
        assert_eq!(parse("https://i.ytimg.com/sb/x/$N.jpg|80#45", 50.0), None, "short level");
        assert_eq!(parse("https://i.ytimg.com/sb/x/$N.jpg", 50.0), None, "no levels");
        let sb = parse("https://i.ytimg.com/sb/x/L$L/$N.jpg|80#45#10#5#5#1000#M$M", 10.0).unwrap();
        assert_eq!(sb.url(0, 1), "https://i.ytimg.com/sb/x/L0/M1.jpg", "no sigh");
    }

    #[test]
    fn level_choice_is_sharp_at_the_device_pixel_ratio() {
        let sb = gpt();
        assert_eq!(sb.level_for(96.0, 1.0), 2, "96 css px at 1× → 160 px frames");
        assert_eq!(sb.level_for(96.0, 2.0), 3, "at 2× → 320 px frames");
        assert_eq!(sb.level_for(160.0, 1.0), 2);
        assert_eq!(sb.level_for(160.0, 2.0), 3);
        assert_eq!(sb.level_for(400.0, 2.0), 3, "nothing big enough → the largest");
        assert_eq!(sb.level_for(40.0, 1.0), 0);
    }

    #[test]
    fn frame_math() {
        let sb = gpt();
        // L3: 10 s per frame, 9 per sheet. 7:52 = 472 s → frame 47 → sheet 5, cell 2 (col 2, row 0).
        let c = sb.frame_at(3, 472.0, 0.0).unwrap();
        assert_eq!((c.n, c.col, c.row, c.cols, c.rows), (47, 2, 0, 3, 3));
        assert!(c.url.contains("/storyboard3_L3/M5.jpg?"), "{}", c.url);
        // Exact boundary: 20 s is frame 2.
        assert_eq!(sb.frame_at(3, 20.0, 0.0).unwrap().n, 2);
        assert_eq!(sb.frame_at(3, 19.99, 0.0).unwrap().n, 1);
        // Past the end: the last frame (698), on the cropped last sheet (6 frames: 3 × 2, 960 × 360 served).
        let c = sb.frame_at(3, 99999.0, 0.0).unwrap();
        assert_eq!((c.n, c.col, c.row, c.cols, c.rows), (698, 2, 1, 3, 2));
        assert!(c.url.contains("/M77.jpg?"));
        // L2: 5 × 5 per sheet; frame 699 - 1 on sheet 27 holding 24 frames (still 5 rows).
        let c = sb.frame_at(2, 6980.0, 0.0).unwrap();
        assert_eq!((c.n, c.cols, c.rows), (698, 5, 5));
        // L0 (interval 0): 100 frames over 6980 s → 69.8 s each, one 10 × 10 sheet.
        let c = sb.frame_at(0, 3490.0, 0.0).unwrap();
        assert_eq!((c.n, c.col, c.row), (50, 0, 5));
        assert!(c.url.contains("/storyboard3_L0/default.jpg?"));
        // BBB: L3's last sheet (M14) holds 2 frames → 2 × 1 (served 640 × 180); L2's (M5) 3 frames → 3 × 1 (480 × 90).
        let b = parse(BBB, 634.0).unwrap();
        let c = b.frame_at(3, 633.0, 0.0).unwrap();
        assert_eq!((c.n, c.col, c.row, c.cols, c.rows), (126, 0, 0, 2, 1));
        assert!(c.url.contains("/M14.jpg?"));
        let c = b.frame_at(2, 640.0, 0.0).unwrap();
        assert_eq!((c.n, c.col, c.cols, c.rows), (127, 2, 3, 1));
        assert!(c.url.contains("/M5.jpg?"));
        // L1's last sheet (M1): 28 frames → 10 × 3 (800 × 135 served).
        let c = b.frame_at(1, 600.0, 0.0).unwrap();
        assert_eq!((c.n, c.cols, c.rows, c.col, c.row), (120, 10, 3, 0, 2));
        // No length in the response: the player's duration is used for interval-0 levels.
        let mut nolen = b.clone();
        nolen.dur = 0.0;
        assert_eq!(nolen.frame_at(0, 317.0, 634.0).unwrap().n, 50);
        assert_eq!(nolen.frame_at(0, 317.0, 0.0), None);
    }

    #[test]
    fn chapter_frames_come_from_inside_the_chapter() {
        let sb = gpt();
        // Chapter at 472 s (frames every 10 s): the frame taken at 480 s, not the one at 470 s (previous chapter).
        assert_eq!(sb.frame_in(3, 472.0, 1100.0, 0.0).unwrap().n, 48);
        // Starts exactly on a frame: that frame.
        assert_eq!(sb.frame_in(3, 20.0, 30.0, 0.0).unwrap().n, 2);
        assert_eq!(sb.frame_in(3, 0.0, 30.0, 0.0).unwrap().n, 0);
        // A chapter shorter than the gap to the next frame: the frame at its start.
        assert_eq!(sb.frame_in(3, 472.0, 478.0, 0.0).unwrap().n, 47);
        // Different chapters, different cells (the bug this replaces: one cover image for all).
        let a = sb.frame_in(3, 0.0, 472.0, 0.0).unwrap();
        let b = sb.frame_in(3, 472.0, 1101.0, 0.0).unwrap();
        assert_ne!((a.url.clone(), a.col, a.row), (b.url.clone(), b.col, b.row));
    }

    #[test]
    fn styles_place_the_cell() {
        let sb = gpt();
        let c = sb.frame_at(3, 472.0, 0.0).unwrap();
        assert_eq!(
            style(&c),
            format!("left:0.000%;top:0.000%;width:100.000%;height:100.000%;background:url(\"{}\") 100.0000% 0.0000%/300% 300% no-repeat", c.url)
        );
        // Middle cell of a 3 × 2 cropped sheet.
        let c = Cell { url: "https://i.ytimg.com/sb/x/M77.jpg".into(), n: 0, col: 1, row: 1, cols: 3, rows: 2, w: 320, h: 180 };
        assert!(style(&c).ends_with("50.0000% 100.0000%/300% 200% no-repeat"), "{}", style(&c));
        // One column: position 0.
        let c = Cell { cols: 1, rows: 1, col: 0, row: 0, ..c };
        assert!(style(&c).ends_with("0.0000% 0.0000%/100% 100% no-repeat"));
        // 4:3 frames are pillarboxed, 9:16 frames too; 2.39:1 frames letterboxed.
        let c43 = Cell { w: 80, h: 60, ..c.clone() };
        assert!(style(&c43).starts_with("left:12.500%;top:0.000%;width:75.000%;height:100.000%"), "{}", style(&c43));
        let wide = Cell { w: 239, h: 100, ..c.clone() };
        assert!(style(&wide).starts_with("left:0.000%;top:12.808%;width:100.000%;height:74.384%"), "{}", style(&wide));
        assert_eq!(thumb_style("https://i.ytimg.com/vi/x/hqdefault.jpg").unwrap(), "left:0;top:0;width:100%;height:100%;background:url(\"https://i.ytimg.com/vi/x/hqdefault.jpg\") 50% 50%/cover no-repeat");
        assert_eq!(thumb_style("https://i.ytimg.com/vi/x\");x"), None);
        assert_eq!(thumb_style("javascript:alert(1)"), None);
    }
}
