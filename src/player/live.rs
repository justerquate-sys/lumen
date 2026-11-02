//! Live streams (pure, unit tested; `own.rs` is the glue).
//!
//! What VISIONOS gives for a live stream (checked 1 Nov 2026 on two public
//! streams): `hlsManifestUrl` (master playlist on manifest.googlevideo.com)
//! whose variants are MPEG-TS video with *separate* packed-ADTS audio
//! (`demuxed/1`), and `adaptiveFormats` without byte ranges (`targetDurationSec`
//! 5, `maxDvrDurationSec`). The same segment numbers (`sq`) are served by the
//! adaptive URLs with `&sq=N` as self-initializing fragmented MP4
//! (`ftyp moov emsg moof mdat`, audio and video on one timeline). So:
//!
//! - the timeline comes from HLS: master → a variant's media playlist
//!   (sequence numbers, durations, sliding window, end), refreshed every
//!   target duration (~14 KB gzip for a one-hour window);
//! - the segments come as fMP4 from the adaptive URLs (`&sq=N`), so MSE takes
//!   them directly: no TS→fMP4 remux, the VOD pipeline (SourceBuffers,
//!   ladder, ABR) is reused.

use std::collections::VecDeque;

/// Latency we aim for behind the end of the newest segment (seconds of
/// media): two target durations. With a refresh every target duration the
/// buffer never drops below ~one segment; 1 Nov 2026 on a 5 s stream this
/// was ~16 s behind the broadcast (YouTube's own "normal latency" range).
pub fn target_latency(target: f64) -> f64 {
    (target * 2.0).clamp(4.0, 20.0)
}

/// Following the live edge, we jump forward when this far behind the target.
pub fn max_drift(target: f64) -> f64 {
    (target * 3.0).max(12.0)
}

/// Whether a position `behind` seconds before the end of the newest segment
/// still counts as "live" (normal play swings by about one segment plus the
/// refresh delay around the target latency).
pub fn at_edge(w: &Window, behind: f64) -> bool {
    behind <= target_latency(w.target) + w.target * 1.5
}

/// Buffer ahead of the playhead when playing behind the edge (DVR).
pub const AHEAD: f64 = 30.0;

/// One `#EXT-X-STREAM-INF` variant of a master playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub bandwidth: u64,
    pub width: u32,
    pub height: u32,
    pub codecs: String,
    pub uri: String,
}

/// Attribute list (`A=1,B="x,y"`) value by name.
pub fn attr(list: &str, name: &str) -> Option<String> {
    let mut rest = list;
    while !rest.is_empty() {
        let (k, after) = rest.split_once('=')?;
        let (v, next) = if let Some(q) = after.strip_prefix('"') {
            let end = q.find('"')?;
            (&q[..end], q[end + 1..].trim_start_matches(','))
        } else {
            match after.split_once(',') {
                Some((v, n)) => (v, n),
                None => (after, ""),
            }
        };
        if k.trim() == name {
            return Some(v.to_string());
        }
        rest = next;
    }
    None
}

fn https(uri: &str) -> bool {
    uri.starts_with("https://") && !uri.bytes().any(|b| b <= b' ' || b == b'"')
}

/// Master playlist → its variants (https URIs only).
pub fn parse_master(text: &str) -> Vec<Variant> {
    let mut out = Vec::new();
    if !text.trim_start().starts_with("#EXTM3U") {
        return out;
    }
    let mut pending: Option<&str> = None;
    for line in text.lines().map(str::trim) {
        if let Some(a) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(a);
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(a) = pending.take() {
                if !https(line) {
                    continue;
                }
                let (w, h) = attr(a, "RESOLUTION").and_then(|r| r.split_once('x').map(|(w, h)| (w.parse().unwrap_or(0), h.parse().unwrap_or(0)))).unwrap_or((0, 0));
                out.push(Variant { bandwidth: attr(a, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0), width: w, height: h, codecs: attr(a, "CODECS").unwrap_or_default(), uri: line.to_string() });
            }
        }
    }
    out
}

/// The variant whose playlist we follow: the one nearest `p` (the rung we
/// play; all variants list the same sequence numbers, so this only keeps the
/// durations closest to what plays), lowest bandwidth on ties.
pub fn pick_variant(vs: &[Variant], p: u32) -> Option<usize> {
    let rung = |v: &Variant| if v.width > 0 && v.height > 0 { v.width.min(v.height) } else { v.height };
    (0..vs.len()).min_by_key(|&i| ((rung(&vs[i]) as i64 - p as i64).unsigned_abs(), vs[i].bandwidth))
}

/// A media playlist: target duration, segments (sequence number, duration)
/// and whether it ended (`#EXT-X-ENDLIST`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playlist {
    pub target: f64,
    pub segs: Vec<(u64, f64)>,
    pub ended: bool,
}

/// `…/sq/1649580/…` (YouTube's path form) or `sq=1649580`.
pub fn sq_of(uri: &str) -> Option<u64> {
    if let Some(i) = uri.find("/sq/") {
        return uri[i + 4..].split(['/', '?', '&']).next()?.parse().ok();
    }
    let q = uri.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.strip_prefix("sq=")).and_then(|v| v.parse().ok())
}

pub fn parse_media(text: &str) -> Option<Playlist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return None;
    }
    let mut p = Playlist::default();
    let mut seq: Option<u64> = None;
    let mut dur: Option<f64> = None;
    let mut n = 0u64;
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            p.target = v.trim().parse().ok()?;
        } else if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("#EXTINF:") {
            dur = v.split(',').next().and_then(|d| d.trim().parse().ok()).filter(|d: &f64| d.is_finite() && *d > 0.0);
        } else if line == "#EXT-X-ENDLIST" {
            p.ended = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            let Some(d) = dur.take() else { continue };
            let sq = sq_of(line).or(seq.map(|s| s + n))?;
            n += 1;
            if p.segs.last().is_some_and(|l| l.0 >= sq) {
                return None; // not increasing: not something we understand
            }
            p.segs.push((sq, d));
        }
    }
    (p.target > 0.0 && !p.segs.is_empty()).then_some(p)
}

/// The sliding window of segments we know (from the latest playlist) and
/// the mapping sequence number ↔ media time, anchored at a segment whose
/// real start time (`tfdt`) we have seen.
#[derive(Clone, Debug, Default)]
pub struct Window {
    pub target: f64,
    pub segs: VecDeque<(u64, f64)>,
    pub ended: bool,
    /// (sequence number, media start time) of a fetched segment.
    pub anchor: Option<(u64, f64)>,
}

/// What a playlist refresh brought.
#[derive(Debug, PartialEq)]
pub enum Update {
    /// New segments at the end.
    Grew(usize),
    /// Nothing new (refresh again sooner: half the target duration).
    Same,
}

impl Window {
    pub fn update(&mut self, p: &Playlist) -> Update {
        let before = self.last();
        self.target = p.target;
        self.ended = p.ended;
        self.segs = p.segs.iter().copied().collect();
        match (before, self.last()) {
            (Some(b), Some(l)) if l > b => Update::Grew((l - b) as usize),
            (None, Some(_)) => Update::Grew(self.segs.len()),
            _ => Update::Same,
        }
    }
    pub fn first(&self) -> Option<u64> {
        self.segs.front().map(|s| s.0)
    }
    pub fn last(&self) -> Option<u64> {
        self.segs.back().map(|s| s.0)
    }
    fn pos(&self, sq: u64) -> Option<usize> {
        let f = self.first()?;
        let i = sq.checked_sub(f)? as usize;
        (self.segs.get(i).map(|s| s.0) == Some(sq)).then_some(i).or_else(|| self.segs.iter().position(|s| s.0 == sq))
    }
    /// Seconds from the window's start to the start of `sq` (by playlist durations).
    pub fn offset(&self, sq: u64) -> Option<f64> {
        let i = self.pos(sq)?;
        Some(self.segs.iter().take(i).map(|s| s.1).sum())
    }
    pub fn span(&self) -> f64 {
        self.segs.iter().map(|s| s.1).sum()
    }
    /// Where to start: the newest segment such that at least `latency`
    /// seconds of media lie from its start to the end of the window (an
    /// ended playlist starts at `from` seconds into it).
    pub fn start_sq(&self, latency: f64) -> Option<u64> {
        let mut acc = 0.0;
        for s in self.segs.iter().rev() {
            acc += s.1;
            if acc >= latency {
                return Some(s.0);
            }
        }
        self.first()
    }
    pub fn note_anchor(&mut self, sq: u64, t: f64) {
        self.anchor = Some((sq, t));
    }
    /// Media time of the window start (needs an anchor inside the window).
    pub fn t_first(&self) -> Option<f64> {
        let (sq, t) = self.anchor?;
        Some(t - self.offset(sq)?)
    }
    /// Media time where `sq` starts.
    pub fn t_of(&self, sq: u64) -> Option<f64> {
        Some(self.t_first()? + self.offset(sq)?)
    }
    /// Media time at the end of the newest segment (the live edge).
    pub fn t_edge(&self) -> Option<f64> {
        Some(self.t_first()? + self.span())
    }
    /// Segment containing media time `t` (clamped to the window).
    pub fn sq_at(&self, t: f64) -> Option<u64> {
        let mut acc = self.t_first()?;
        for s in &self.segs {
            if t < acc + s.1 {
                return Some(s.0);
            }
            acc += s.1;
        }
        self.last()
    }
    /// The segment after the media buffered up to `end` (a hair before the
    /// boundary, so estimates never skip one: an overlap is harmless, a gap is not).
    pub fn after(&self, end: f64) -> Option<u64> {
        self.sq_at(end - 0.1).map(|s| s + 1)
    }
}

/// Fetch decision for one track.
#[derive(Debug, PartialEq)]
pub enum Next {
    Fetch(u64),
    Wait,
    Done,
}

/// `next` is the next sequence number wanted; `t` the playhead. Before an
/// anchor exists only the first segment may be fetched (`first` = true).
pub fn next(w: &Window, next: u64, t: f64, first: bool) -> Next {
    let (Some(f), Some(l)) = (w.first(), w.last()) else { return Next::Wait };
    let sq = next.max(f);
    if sq > l {
        return if w.ended { Next::Done } else { Next::Wait };
    }
    if w.anchor.is_none() {
        return if first { Next::Fetch(sq) } else { Next::Wait };
    }
    match w.t_of(sq) {
        Some(t0) if t0 > t + AHEAD => Next::Wait,
        _ => Next::Fetch(sq),
    }
}

/// When to fetch the playlist again (ms): the target duration after new
/// segments, half of it when nothing changed (HLS client rules).
pub fn refresh_ms(target: f64, u: &Update) -> u32 {
    let t = target.clamp(1.0, 20.0) * 1000.0;
    (if *u == Update::Same { t / 2.0 } else { t }) as u32
}

/// Live-edge follow-up while playing: a seek target when the playhead fell
/// too far behind the edge (stall, background tab) or out of the window.
pub fn catch_up(w: &Window, t: f64) -> Option<f64> {
    let (first, edge) = (w.t_first()?, w.t_edge()?);
    let want = edge - target_latency(w.target);
    if t < first || edge - t > target_latency(w.target) + max_drift(w.target) {
        return Some(want.max(first));
    }
    None
}

// ------------------------------------------------------------ fMP4 segments

fn boxes(d: &[u8]) -> Vec<([u8; 4], usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= d.len() {
        let sz = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as u64;
        let typ = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
        let (sz, head) = match sz {
            1 if i + 16 <= d.len() => (u64::from_be_bytes(d[i + 8..i + 16].try_into().unwrap_or_default()), 16),
            0 => ((d.len() - i) as u64, 8),
            s => (s, 8),
        };
        if sz < head as u64 || i as u64 + sz > d.len() as u64 {
            break;
        }
        out.push((typ, i, i + head, i + sz as usize));
        i += sz as usize;
    }
    out
}

fn child<'a>(d: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    let (first, rest) = path.split_first()?;
    let (_, _, b, e) = boxes(d).into_iter().find(|x| &x.0 == *first)?;
    if rest.is_empty() { Some(&d[b..e]) } else { child(&d[b..e], rest) }
}

fn be(d: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = d.get(at..at + n)?;
    Some(s.iter().fold(0u64, |a, b| a << 8 | *b as u64))
}

/// A self-initializing live segment split for MSE.
#[derive(Debug, PartialEq)]
pub struct Parts {
    /// `ftyp` + `moov` (appended when the representation changes).
    pub init: Vec<u8>,
    /// `moof` + `mdat` boxes (`emsg`/`sidx`/`styp` dropped).
    pub media: Vec<u8>,
    /// Media start time (`tfdt` / `mdhd` timescale), seconds.
    pub t0: Option<f64>,
    /// `Ingestion-Walltime-Us` from YouTube's `emsg` (epoch ms), for latency.
    pub wall_ms: Option<f64>,
}

pub fn split(d: &[u8]) -> Result<Parts, String> {
    let list = boxes(d);
    let (mut init, mut media) = (Vec::new(), Vec::new());
    let (mut moov, mut wall_ms, mut t0) = (None, None, None);
    for (typ, s, b, e) in &list {
        match typ {
            b"ftyp" => init.extend_from_slice(&d[*s..*e]),
            b"moov" => {
                init.extend_from_slice(&d[*s..*e]);
                moov = Some(&d[*b..*e]);
            }
            b"moof" => {
                if t0.is_none() {
                    t0 = child(&d[*b..*e], &[b"traf", b"tfdt"]).and_then(|t| if t.first() == Some(&1) { be(t, 4, 8) } else { be(t, 4, 4) });
                }
                media.extend_from_slice(&d[*s..*e]);
            }
            b"mdat" => media.extend_from_slice(&d[*s..*e]),
            b"emsg" => {
                let text = String::from_utf8_lossy(&d[*b..*e]);
                wall_ms = text.split("Ingestion-Walltime-Us: ").nth(1).and_then(|r| r.split(['\r', '\n']).next()).and_then(|v| v.trim().parse::<f64>().ok()).map(|us| us / 1000.0);
            }
            _ => {}
        }
    }
    if media.is_empty() {
        return Err(format!("no moof/mdat in {} bytes", d.len()));
    }
    let scale = moov.and_then(|m| child(m, &[b"trak", b"mdia", b"mdhd"])).and_then(|h| if h.first() == Some(&1) { be(h, 20, 4) } else { be(h, 12, 4) }).filter(|s| *s > 0);
    let t0 = match (t0, scale) {
        (Some(t), Some(s)) => Some(t as f64 / s as f64),
        _ => None,
    };
    Ok(Parts { init, media, t0, wall_ms })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n\
#EXT-X-MEDIA:URI=\"https://manifest.googlevideo.com/a/233\",TYPE=AUDIO,GROUP-ID=\"233\",NAME=\"Default\",DEFAULT=YES\n\
#EXT-X-STREAM-INF:BANDWIDTH=507418,CODECS=\"avc1.4D4015,mp4a.40.5\",RESOLUTION=426x240,FRAME-RATE=30,AUDIO=\"233\"\n\
https://manifest.googlevideo.com/v/240\n\
#EXT-X-STREAM-INF:BANDWIDTH=962461,CODECS=\"avc1.4D401E,mp4a.40.2\",RESOLUTION=640x360,FRAME-RATE=30,AUDIO=\"234\"\n\
https://manifest.googlevideo.com/v/360\n\
#EXT-X-STREAM-INF:BANDWIDTH=4561186,CODECS=\"avc1.640028,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=30,AUDIO=\"234\"\n\
https://manifest.googlevideo.com/v/1080\n\
#EXT-X-STREAM-INF:BANDWIDTH=1,RESOLUTION=1x1\nhttp://insecure.example/x\n";

    #[test]
    fn master_variants() {
        let v = parse_master(MASTER);
        assert_eq!(v.len(), 3, "http variant dropped");
        assert_eq!(v[0], Variant { bandwidth: 507418, width: 426, height: 240, codecs: "avc1.4D4015,mp4a.40.5".into(), uri: "https://manifest.googlevideo.com/v/240".into() });
        assert_eq!(pick_variant(&v, 360), Some(1));
        assert_eq!(pick_variant(&v, 720), Some(1), "nearest rung (|360-720| < |1080-720|)");
        assert_eq!(pick_variant(&v, 1080), Some(2));
        assert_eq!(pick_variant(&[], 1080), None);
        assert!(parse_master("<html>").is_empty());
        assert_eq!(attr("A=1,B=\"x,y\",C=z", "B").as_deref(), Some("x,y"));
        assert_eq!(attr("A=1,B=\"x,y\",C=z", "C").as_deref(), Some("z"));
        assert_eq!(attr("A=1", "Q"), None);
    }

    fn media(first: u64, n: u64, ended: bool) -> String {
        let mut s = format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:{first}\n#EXT-X-DISCONTINUITY-SEQUENCE:520\n");
        for sq in first..first + n {
            s.push_str("#EXT-X-DATERANGE:CLASS=\"CUEPOINT-AD\",DURATION=1\n");
            s.push_str(&format!("#EXTINF:{},\nhttps://rr4---sn-x.googlevideo.com/videoplayback/id/x.2/itag/230/playlist/index.m3u8/sq/{sq}/dur/5.005/file/seg.ts\n", if sq % 7 == 3 { "1.735066" } else { "5.005" }));
        }
        if ended {
            s.push_str("#EXT-X-ENDLIST\n");
        }
        s
    }

    #[test]
    fn media_playlists() {
        let p = parse_media(&media(1649580, 4, false)).unwrap();
        assert_eq!(p.target, 5.0);
        assert!(!p.ended);
        assert_eq!(p.segs, vec![(1649580, 5.005), (1649581, 1.735066), (1649582, 5.005), (1649583, 5.005)]);
        assert!(parse_media(&media(10, 2, true)).unwrap().ended);
        // Without /sq/ in the URI the media sequence numbers them.
        let q = parse_media("#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:7\n#EXTINF:2.0,\nseg-a.ts\n#EXTINF:2.0,\nseg-b.ts\n").unwrap();
        assert_eq!(q.segs, vec![(7, 2.0), (8, 2.0)]);
        assert_eq!(sq_of("https://x/videoplayback?itag=134&sq=42&x=1"), Some(42));
        assert_eq!(sq_of("https://x/a/sq/1649580/dur/5"), Some(1649580));
        assert!(parse_media("#EXTM3U\n#EXT-X-TARGETDURATION:5\n").is_none(), "no segments");
        assert!(parse_media("#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:5,\nhttps://x/sq/9\n#EXTINF:5,\nhttps://x/sq/8\n").is_none(), "decreasing");
        assert!(parse_media("nope").is_none());
    }

    #[test]
    fn window_mapping_and_start() {
        let mut w = Window::default();
        assert_eq!(w.update(&parse_media(&media(100, 6, false)).unwrap()), Update::Grew(6));
        // 100..105; 101 is a 1.735 s (ad cue) segment, the rest 5.005 s.
        assert_eq!((w.first(), w.last()), (Some(100), Some(105)));
        assert_eq!(w.start_sq(target_latency(5.0)), Some(104), "10 s: 105 + 104 (10.01)");
        assert_eq!(w.start_sq(12.0), Some(103), "12 s: 105 + 104 (10.01) + 103 (15.015)");
        assert_eq!(w.start_sq(1000.0), Some(100));
        assert_eq!(w.t_of(101), None, "no anchor yet");
        w.note_anchor(102, 8_194_864.0);
        assert!((w.t_first().unwrap() - (8_194_864.0 - 5.005 - 1.735066)).abs() < 1e-6);
        assert!((w.t_of(104).unwrap() - (8_194_864.0 + 10.01)).abs() < 1e-6);
        assert_eq!(w.sq_at(8_194_864.0 + 0.5), Some(102));
        assert_eq!(w.sq_at(8_194_864.0 + 5.2), Some(103));
        assert_eq!(w.sq_at(1.0), Some(100), "before the window → first");
        assert_eq!(w.sq_at(9e9), Some(105), "past the edge → last");
        // Buffered exactly to the end of 102 → continue at 103 (never skip one).
        assert_eq!(w.after(w.t_of(103).unwrap()), Some(103));
        // The window slides; the anchor stays valid while inside it.
        assert_eq!(w.update(&parse_media(&media(101, 6, false)).unwrap()), Update::Grew(1));
        assert!((w.t_of(104).unwrap() - (8_194_864.0 + 10.01)).abs() < 1e-6);
        assert_eq!(w.update(&parse_media(&media(101, 6, false)).unwrap()), Update::Same);
        assert_eq!(refresh_ms(5.0, &Update::Same), 2500);
        assert_eq!(refresh_ms(5.0, &Update::Grew(1)), 5000);
        assert_eq!(refresh_ms(0.0, &Update::Grew(1)), 1000);
    }

    #[test]
    fn fetch_decisions() {
        let mut w = Window::default();
        assert_eq!(next(&w, 5, 0.0, true), Next::Wait, "no playlist yet");
        w.update(&parse_media(&media(100, 10, false)).unwrap());
        assert_eq!(next(&w, 107, 0.0, true), Next::Fetch(107), "first segment before an anchor");
        assert_eq!(next(&w, 108, 0.0, false), Next::Wait, "the rest waits for the anchor");
        assert_eq!(next(&w, 90, 0.0, true), Next::Fetch(100), "fell out of the window → its start");
        w.note_anchor(107, 1000.0);
        assert_eq!(next(&w, 108, 1000.0, false), Next::Fetch(108));
        assert_eq!(next(&w, 110, 1000.0, false), Next::Wait, "not published yet");
        // DVR: far behind the edge only AHEAD seconds are buffered.
        assert_eq!(next(&w, 108, 1000.0 - 40.0, false), Next::Wait);
        w.update(&parse_media(&media(100, 10, true)).unwrap());
        assert_eq!(next(&w, 110, 1000.0, false), Next::Done, "ended and everything fetched");
    }

    #[test]
    fn catch_up_to_the_edge() {
        let mut w = Window::default();
        w.update(&parse_media(&media(100, 60, false)).unwrap());
        assert_eq!(catch_up(&w, 0.0), None, "no anchor");
        w.note_anchor(100, 0.0);
        let edge = w.t_edge().unwrap();
        assert_eq!(catch_up(&w, edge - 13.0), None, "near the target latency");
        assert_eq!(catch_up(&w, edge - 24.0), None, "within the allowed drift (10 + 15 s)");
        let want = catch_up(&w, edge - 60.0).unwrap();
        assert!((want - (edge - 10.0)).abs() < 1e-9);
        assert_eq!(catch_up(&w, -5.0), Some(edge - 10.0), "fell out of the window");
        assert!(at_edge(&w, 17.0) && !at_edge(&w, 18.0), "5 s target: 10 + 7.5");
        assert_eq!(target_latency(1.0), 4.0);
        assert_eq!(target_latency(5.0), 10.0);
    }

    fn bx(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(t);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn fmp4_segments() {
        // mdhd v0: version/flags, creation, modification, timescale 90000, duration
        let mut mdhd = vec![0u8; 4 + 4 + 4];
        mdhd.extend_from_slice(&90_000u32.to_be_bytes());
        mdhd.extend_from_slice(&[0; 8]);
        let moov = bx(b"moov", &bx(b"trak", &bx(b"mdia", &bx(b"mdhd", &mdhd))));
        let ftyp = bx(b"ftyp", b"dash\0\0\0\0iso6");
        let emsg = bx(b"emsg", b"\0\0\0\0http://youtube.com/streaming/metadata/segment/102015\0\0Sequence-Number: 1649580\r\nIngestion-Walltime-Us: 1793549626322563\r\n\r\n");
        let mut tfdt = vec![1u8, 0, 0, 0];
        tfdt.extend_from_slice(&737_537_784_984u64.to_be_bytes());
        let moof = bx(b"moof", &[bx(b"mfhd", &[0; 8]), bx(b"traf", &[bx(b"tfhd", &[0; 8]), bx(b"tfdt", &tfdt)].concat())].concat());
        let mdat = bx(b"mdat", &[7; 100]);
        let seg = [ftyp.clone(), moov.clone(), emsg, moof.clone(), mdat.clone()].concat();
        let p = split(&seg).unwrap();
        assert_eq!(p.init, [ftyp, moov].concat());
        assert_eq!(p.media, [moof.clone(), mdat.clone()].concat(), "emsg dropped");
        assert!((p.t0.unwrap() - 8_194_864.2776).abs() < 1e-3);
        assert_eq!(p.wall_ms, Some(1_793_549_626_322.563));
        // Media-only segment: no init, no time scale → no t0.
        let q = split(&[moof, mdat].concat()).unwrap();
        assert!(q.init.is_empty() && q.t0.is_none());
        assert!(split(b"garbage").is_err());
        assert!(split(&bx(b"ftyp", b"x")).is_err());
    }
}
