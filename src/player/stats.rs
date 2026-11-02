//! Watch-history pings, built like YouTube's web player does (base.js `Q4a`,
//! player 11a45ffb): `videostatsPlaybackUrl` once at start and
//! `videostatsWatchtimeUrl` on a schedule, on pause/end and at the end of the
//! session (`final=1`). The base URLs come from the WEB `player` response's
//! `playbackTracking`; `s.youtube.com` is replaced by `www.youtube.com` as the
//! web player does (`vss_host`), so the pings are same-origin with cookies.

use crate::util::pct_encode;
use crate::json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Tracking {
    pub playback: String,
    pub watchtime: String,
    /// Wall-clock seconds after start for the first watchtime pings (10, 20, 30).
    pub flush_at: Vec<f64>,
    /// Then every this many seconds (40 on WEB).
    pub flush_every: f64,
}

/// Only YouTube stats endpoints are accepted; the host becomes www.youtube.com.
pub fn rehost(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://s.youtube.com/").or_else(|| url.strip_prefix("https://www.youtube.com/"))?;
    (rest.starts_with("api/stats/") && !url.bytes().any(|b| b <= b' ' || b == b'"' || b == b'\\')).then(|| format!("https://www.youtube.com/{rest}"))
}

pub fn tracking(player: &Value) -> Option<Tracking> {
    let pt = player.get("playbackTracking")?;
    let url = |k: &str| pt.pointer(&format!("/{k}/baseUrl")).and_then(Value::as_str).and_then(rehost);
    let flush_at: Vec<f64> = pt
        .get("videostatsScheduledFlushWalltimeSeconds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).filter(|&x| x > 0.0).collect())
        .unwrap_or_else(|| vec![10.0, 20.0, 30.0]);
    let every = pt.get("videostatsDefaultFlushIntervalSeconds").and_then(Value::as_f64).filter(|&x| x >= 5.0).unwrap_or(40.0);
    Some(Tracking { playback: url("videostatsPlaybackUrl")?, watchtime: url("videostatsWatchtimeUrl")?, flush_at, flush_every: every })
}

const CPN_ALPHABET: &[u8; 64] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

/// 16-character client playback nonce from a seed (time, video id).
pub fn cpn(seed: &str) -> String {
    let d = sha1_smol::Sha1::from(seed).digest().bytes();
    d.iter().take(16).map(|b| CPN_ALPHABET[(b & 63) as usize] as char).collect()
}

/// `Number(x.toFixed(3)).toString()` as base.js formats times.
pub fn n3(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    let r = (x * 1000.0).round() / 1000.0;
    let s = format!("{r:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Values shared by every ping of one playback session.
#[derive(Clone, Debug, Default)]
pub struct Session {
    /// The InnerTube client whose `player` response gave the tracking URLs ("" = WEB) and its
    /// platform ("" = DESKTOP); the TV app's signed-in session is TVHTML5 / TV.
    pub client: String,
    pub platform: String,
    pub cpn: String,
    pub client_version: String,
    pub hl: String,
    pub referrer: String,
    pub len: f64,
    pub fmt: u32,
    pub afmt: u32,
}

/// Player state at the moment of a ping.
#[derive(Clone, Debug, Default)]
pub struct Now {
    /// Media time.
    pub cmt: f64,
    /// Wall-clock seconds since the session started.
    pub rt: f64,
    /// Milliseconds since the last user input.
    pub lact: f64,
    pub fs: bool,
    pub volume: u32,
    pub muted: bool,
    pub playing: bool,
    pub rate: f64,
}

fn push(u: &mut String, k: &str, v: &str) {
    u.push('&');
    u.push_str(k);
    u.push('=');
    u.push_str(&pct_encode(v));
}

fn common(u: &mut String, s: &Session, n: &Now) {
    push(u, "ver", "2");
    push(u, "cpn", &s.cpn);
    push(u, "cmt", &n3(n.cmt));
    if s.fmt > 0 {
        push(u, "fmt", &s.fmt.to_string());
    }
    if s.afmt > 0 {
        push(u, "afmt", &s.afmt.to_string());
    }
    push(u, "fs", if n.fs { "1" } else { "0" });
    push(u, "rt", &n3(n.rt));
    push(u, "lact", &format!("{}", n.lact.max(0.0).round() as u64));
    push(u, "c", if s.client.is_empty() { "WEB" } else { &s.client });
    push(u, "cver", &s.client_version);
    push(u, "cplayer", "UNIPLAYER");
    push(u, "cplatform", if s.platform.is_empty() { "DESKTOP" } else { &s.platform });
    if !s.hl.is_empty() {
        push(u, "hl", &s.hl);
    }
    if !s.referrer.is_empty() {
        push(u, "referrer", &s.referrer);
    }
    if !u.contains("&len=") && !u.contains("?len=") && s.len > 0.0 {
        push(u, "len", &n3(s.len));
    }
}

pub fn playback_url(base: &str, s: &Session, n: &Now) -> String {
    let mut u = base.to_string();
    common(&mut u, s, n);
    push(&mut u, "volume", &n.volume.to_string());
    push(&mut u, "muted", if n.muted { "1" } else { "0" });
    u
}

/// One played stretch of media time (between seeks/pauses).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Played {
    pub st: f64,
    pub et: f64,
}

pub fn watchtime_url(base: &str, s: &Session, n: &Now, segs: &[Played], fin: bool) -> String {
    let mut u = base.to_string();
    common(&mut u, s, n);
    push(&mut u, "state", if n.playing { "playing" } else { "paused" });
    let list = |f: &dyn Fn(&Played) -> String| segs.iter().map(f).collect::<Vec<_>>().join(",");
    push(&mut u, "volume", &list(&|_| n.volume.to_string()));
    push(&mut u, "st", &list(&|p| n3(p.st)));
    push(&mut u, "et", &list(&|p| n3(p.et)));
    if (n.rate - 1.0).abs() > 1e-6 {
        push(&mut u, "rate", &list(&|_| n3(n.rate)));
    }
    push(&mut u, "muted", &list(&|_| if n.muted { "1".into() } else { "0".into() }));
    if fin {
        push(&mut u, "final", "1");
    }
    u
}

/// Collects played media-time stretches between pings.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    done: Vec<Played>,
    cur: Option<Played>,
}

impl Recorder {
    /// Called with the media time while playing (a few times per second).
    /// A jump (seek) closes the current stretch.
    pub fn tick(&mut self, t: f64, rate: f64) {
        match &mut self.cur {
            Some(p) if t >= p.et - 0.05 && t - p.et < 2.0 * rate.max(1.0) + 0.5 => p.et = t,
            _ => {
                self.close();
                self.cur = Some(Played { st: t, et: t });
            }
        }
    }
    /// Pause/seek: close the current stretch.
    pub fn close(&mut self) {
        if let Some(p) = self.cur.take() {
            if p.et > p.st {
                self.done.push(p);
            }
        }
    }
    /// Stretches for a ping (at least one, `[t,t]` when nothing was played);
    /// the open stretch continues from its end.
    pub fn take(&mut self, t: f64) -> Vec<Played> {
        let mut out = std::mem::take(&mut self.done);
        if let Some(p) = &mut self.cur {
            if p.et > p.st {
                out.push(*p);
            }
            *p = Played { st: p.et, et: p.et };
        }
        if out.is_empty() {
            out.push(Played { st: t, et: t });
        }
        out
    }
}

/// Watchtime schedule: `flush_at` walltimes, then every `flush_every`.
#[derive(Clone, Debug)]
pub struct Schedule {
    at: Vec<f64>,
    every: f64,
    next: f64,
    i: usize,
}

impl Schedule {
    pub fn new(t: &Tracking) -> Schedule {
        let mut at = t.flush_at.clone();
        at.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let next = at.first().copied().unwrap_or(t.flush_every);
        Schedule { at, every: t.flush_every, next, i: 0 }
    }
    /// True once per due point; `rt` = seconds since session start.
    pub fn due(&mut self, rt: f64) -> bool {
        if rt < self.next {
            return false;
        }
        self.i += 1;
        let last = self.at.last().copied().unwrap_or(0.0);
        self.next = match self.at.get(self.i) {
            Some(&x) => x,
            None => (last + self.every * (self.i + 1 - self.at.len()) as f64).max(rt + 1.0),
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tracking(v: &serde_json::Value) -> Option<Tracking> {
        super::tracking(&crate::json::Value::from_serde(v))
    }

    fn q(url: &str, k: &str) -> Option<String> {
        crate::util::query_param(url.split_once('?').unwrap().1, k)
    }

    #[test]
    fn tracking_urls() {
        let p = json!({"playbackTracking": {
            "videostatsPlaybackUrl": {"baseUrl": "https://s.youtube.com/api/stats/playback?cl=1&docid=aqz-KE-bpKQ&ei=E&ns=yt&el=detailpage&len=635&of=O&vm=V"},
            "videostatsWatchtimeUrl": {"baseUrl": "https://s.youtube.com/api/stats/watchtime?cl=1&docid=aqz-KE-bpKQ&ei=E&ns=yt&el=detailpage&len=635&of=O&vm=V"},
            "videostatsScheduledFlushWalltimeSeconds": [10, 20, 30], "videostatsDefaultFlushIntervalSeconds": 40}});
        let t = tracking(&p).unwrap();
        assert!(t.playback.starts_with("https://www.youtube.com/api/stats/playback?cl=1&docid="));
        assert_eq!(t.flush_at, vec![10.0, 20.0, 30.0]);
        assert_eq!(t.flush_every, 40.0);
        assert_eq!(rehost("https://evil.com/api/stats/x"), None);
        assert_eq!(rehost("https://s.youtube.com/ptracking?x"), None);
        assert!(tracking(&json!({})).is_none());
    }

    #[test]
    fn ping_parameters() {
        let s = Session { cpn: cpn("seed"), client_version: "2.20261030.01.00".into(), hl: "en".into(), referrer: "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into(), len: 635.0, fmt: 303, afmt: 251, ..Default::default() };
        assert_eq!(s.cpn.len(), 16);
        assert!(s.cpn.bytes().all(|b| CPN_ALPHABET.contains(&b)));
        assert_ne!(cpn("seed"), cpn("seed2"));
        let n = Now { cmt: 12.3456, rt: 10.0004, lact: 1234.4, fs: false, volume: 80, muted: false, playing: true, rate: 1.0 };
        let base = "https://www.youtube.com/api/stats/playback?cl=1&docid=aqz-KE-bpKQ&len=635";
        let u = playback_url(base, &s, &n);
        for (k, v) in [("ver", "2"), ("cmt", "12.346"), ("fmt", "303"), ("afmt", "251"), ("fs", "0"), ("rt", "10"), ("lact", "1234"), ("c", "WEB"), ("cver", "2.20261030.01.00"), ("cplayer", "UNIPLAYER"), ("volume", "80"), ("muted", "0"), ("referrer", "https://www.youtube.com/watch?v=aqz-KE-bpKQ"), ("len", "635")] {
            assert_eq!(q(&u, k).as_deref(), Some(v), "{k} in {u}");
        }
        assert_eq!(u.matches("&len=").count(), 1, "len kept from the base URL only");
        let segs = [Played { st: 0.0, et: 10.5 }, Played { st: 30.0, et: 31.25 }];
        let n2 = Now { playing: false, rate: 1.5, ..n };
        let w = watchtime_url("https://www.youtube.com/api/stats/watchtime?docid=x", &s, &n2, &segs, true);
        for (k, v) in [("st", "0,30"), ("et", "10.5,31.25"), ("state", "paused"), ("volume", "80,80"), ("muted", "0,0"), ("rate", "1.5,1.5"), ("final", "1"), ("len", "635")] {
            assert_eq!(q(&w, k).as_deref(), Some(v), "{k} in {w}");
        }
        // The TV app's signed-in session reports itself as the TV client.
        let tv = Session { client: "TVHTML5".into(), platform: "TV".into(), client_version: "7.20261027.15.00".into(), ..s.clone() };
        let u = playback_url(base, &tv, &n);
        for (k, v) in [("c", "TVHTML5"), ("cplatform", "TV"), ("cver", "7.20261027.15.00")] {
            assert_eq!(q(&u, k).as_deref(), Some(v), "{k} in {u}");
        }
        assert_eq!(q(&playback_url(base, &s, &n), "cplatform").as_deref(), Some("DESKTOP"));
        assert_eq!(n3(1.0), "1");
        assert_eq!(n3(0.1 + 0.2), "0.3");
        assert_eq!(n3(f64::NAN), "0");
    }

    #[test]
    fn recorder_and_schedule() {
        let mut r = Recorder::default();
        for i in 0..=20 {
            r.tick(i as f64 * 0.25, 1.0);
        }
        r.tick(100.0, 1.0); // seek
        r.tick(100.25, 1.0);
        assert_eq!(r.take(100.25), vec![Played { st: 0.0, et: 5.0 }, Played { st: 100.0, et: 100.25 }]);
        r.tick(100.5, 1.0);
        assert_eq!(r.take(100.5), vec![Played { st: 100.25, et: 100.5 }]);
        r.close();
        assert_eq!(r.take(7.0), vec![Played { st: 7.0, et: 7.0 }]);
        let t = Tracking { playback: String::new(), watchtime: String::new(), flush_at: vec![10.0, 20.0, 30.0], flush_every: 40.0 };
        let mut s = Schedule::new(&t);
        let due: Vec<u32> = (0..200).filter(|&x| s.due(x as f64)).collect();
        assert_eq!(due, vec![10, 20, 30, 70, 110, 150, 190]);
    }
}
