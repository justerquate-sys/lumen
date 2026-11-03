//! Speed telemetry from the TV to the owner's debug center (`tools/debug-center` on their computer), **off
//! unless the owner turns it on** (About & updates → Developer, or `files/dev.json` in debug builds).
//!
//! While it is on, the app runs the bundle's development variant (`content-dev.js`, the client's speed
//! telemetry; see `release::DEV_FILE`), the page shim hands every report the client makes (`ytcPerf` batches,
//! `ytcIncident` reports) to the shell, and the shell delivers them to the debug center, one POST per report to
//! `<url>/incident`, oldest first, as the extension's background worker does. Every report is stamped with the
//! build and `"platform":"tv"` (the debug center tells TV and desktop apart by it).
//!
//! - The address can only be on the owner's network (private IPv4 ranges, `*.local`, the emulator's host);
//!   reports never go to the internet.
//! - Reports are kept in memory only (last 30 batches, last 100 incidents) and dropped when the app ends.
//! - [`discover`]: one UDP probe (broadcast, plus the emulator's host) that a debug center started with `--lan`
//!   answers; its sender address is the collector.

use crate::net::Http;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

/// The debug center's port (TCP for reports, UDP for discovery).
pub const PORT: u16 = 47321;
/// The discovery probe and the start of its answer (`LUMEN-DC <tcp port>`).
pub const PROBE: &[u8] = b"LUMEN-DC?";
pub const ANSWER: &str = "LUMEN-DC ";
/// Largest report kept (the extension's worker cuts at the same size).
const MAX_REPORT: usize = 400_000;
const MAX_PERF: usize = 30;
const MAX_INCIDENTS: usize = 100;

/// The owner's choice, kept in `files/telemetry.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub on: bool,
    /// `http://<host>:<port>` (normalised by [`collector_url`]); empty = not set.
    #[serde(default)]
    pub url: String,
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        let s: Settings = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // (A file edited by hand: an address that is not on the network is dropped, and telemetry with it.)
        match collector_url(&s.url) {
            Ok(url) => Settings { on: s.on, url },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::write_atomic(path, serde_json::to_string(self).unwrap_or_default().as_bytes())
    }
}

/// Is `ip` on the owner's network: private ranges, loopback, link-local, carrier-grade NAT (Tailscale).
pub fn local_ip(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || (o[0] == 100 && (64..128).contains(&o[1]))
}

/// The collector's base URL from what the owner typed: `192.168.1.20`, `192.168.1.20:47321`,
/// `http://mac.local:47321/` → `http://<host>:<port>`. Only plain HTTP to an address on the owner's network.
pub fn collector_url(input: &str) -> Result<String, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("no address".into());
    }
    let s = s.strip_prefix("http://").unwrap_or(s);
    if s.contains("://") {
        return Err("the debug center speaks plain http".into());
    }
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix("/incident").unwrap_or(s);
    if s.contains(['/', '?', '#', '@', '[', ' ']) {
        return Err("not an address".into());
    }
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p > 0).ok_or("bad port")?),
        None => (s, PORT),
    };
    let host = host.to_ascii_lowercase();
    let ok = match host.parse::<Ipv4Addr>() {
        Ok(ip) => local_ip(ip),
        Err(_) => {
            (host.ends_with(".local") || host == "localhost")
                && host.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        }
    };
    if !ok {
        return Err("only an address on your own network (e.g. 192.168.1.20)".into());
    }
    Ok(format!("http://{host}:{port}"))
}

/// A report as the debug center stores it: `{"build": <build>, "platform": "tv", …the report}` (a report that
/// names its build already keeps it). None for something that is not a JSON object or is too large.
pub fn stamp(json: &str, build: &str) -> Option<String> {
    let t = json.trim_start();
    if !t.starts_with('{') || t.len() > MAX_REPORT {
        return None;
    }
    let rest = t[1..].trim_start();
    let mut out = String::with_capacity(t.len() + build.len() + 40);
    out.push('{');
    if !rest.starts_with("\"build\"") {
        out.push_str("\"build\":");
        out.push_str(&serde_json::Value::from(build).to_string());
        out.push(',');
    }
    out.push_str("\"platform\":\"tv\"");
    if !rest.starts_with('}') {
        out.push(',');
    }
    out.push_str(rest);
    Some(out)
}

/// An incident without what could name a search: the page's query and fragment are cut from its `path` (the client's
/// own script-error reports carry `location.pathname + location.search`). Anything that is not a JSON object stays as
/// it is (and `stamp` refuses it).
pub fn scrub(json: &str) -> String {
    let Ok(serde_json::Value::Object(mut o)) = serde_json::from_str::<serde_json::Value>(json) else { return json.to_string() };
    if let Some(serde_json::Value::String(p)) = o.get_mut("path") {
        if let Some(i) = p.find(['?', '#']) {
            p.truncate(i);
        }
    }
    serde_json::Value::Object(o).to_string()
}

/// Reports waiting for the debug center (memory only).
#[derive(Default)]
pub struct Queue {
    items: VecDeque<(bool, String)>,
}

impl Queue {
    /// Adds a report (`perf`: a telemetry batch); the oldest of its kind goes beyond the caps.
    pub fn push(&mut self, perf: bool, json: String) {
        self.items.push_back((perf, json));
        let cap = if perf { MAX_PERF } else { MAX_INCIDENTS };
        if self.items.iter().filter(|x| x.0 == perf).count() > cap {
            if let Some(i) = self.items.iter().position(|x| x.0 == perf) {
                self.items.remove(i);
            }
        }
    }

    pub fn front(&self) -> Option<&str> {
        self.items.front().map(|x| x.1.as_str())
    }

    /// Removes the oldest report if it is still `json` (delivered).
    pub fn delivered(&mut self, json: &str) {
        if self.front() == Some(json) {
            self.items.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// Delivers one report to the collector at `url`.
pub fn post(http: &Http, url: &str, json: &str) -> Result<(), String> {
    let a = http.post_body(&format!("{url}/incident"), "application/json", json.as_bytes())?;
    if (200..300).contains(&a.status) { Ok(()) } else { Err(format!("debug center: HTTP {}", a.status)) }
}

/// The debug center's answer to a probe from `from`: its URL.
pub fn answer_url(msg: &[u8], from: SocketAddr) -> Option<String> {
    let s = std::str::from_utf8(msg).ok()?.trim();
    let port: u16 = s.strip_prefix(ANSWER)?.trim().parse().ok()?;
    match from {
        SocketAddr::V4(a) if local_ip(*a.ip()) && port > 0 => Some(format!("http://{}:{port}", a.ip())),
        _ => None,
    }
}

/// Looks for a debug center started with `--lan`: one probe to each of `to`, the first good answer within
/// `wait` wins.
pub fn discover_at(to: &[SocketAddr], wait: Duration) -> Result<String, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let _ = sock.set_broadcast(true);
    let mut sent = false;
    for a in to {
        sent |= sock.send_to(PROBE, a).is_ok();
    }
    if !sent {
        return Err("no network".into());
    }
    let end = Instant::now() + wait;
    let mut buf = [0u8; 64];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("no debug center answered (start it with --lan on your computer)".into());
        }
        sock.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(u) = answer_url(&buf[..n], from) {
                    return Ok(u);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// [`discover_at`] the network's broadcast address and the Android emulator's host (10.0.2.2).
pub fn discover(wait: Duration) -> Result<String, String> {
    let to = [SocketAddr::from((Ipv4Addr::BROADCAST, PORT)), SocketAddr::from((Ipv4Addr::new(10, 0, 2, 2), PORT))];
    discover_at(&to, wait)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_on_the_network_only() {
        assert_eq!(collector_url("192.168.1.20").unwrap(), "http://192.168.1.20:47321");
        assert_eq!(collector_url(" http://10.0.2.2:47321/incident ").unwrap(), "http://10.0.2.2:47321");
        assert_eq!(collector_url("172.20.0.5:8080/").unwrap(), "http://172.20.0.5:8080");
        assert_eq!(collector_url("Mac-Mini.local").unwrap(), "http://mac-mini.local:47321");
        assert_eq!(collector_url("100.101.102.103").unwrap(), "http://100.101.102.103:47321");
        for bad in ["", "8.8.8.8", "example.com", "https://192.168.1.2", "192.168.1.2:0", "192.168.1.2:99999", "user@192.168.1.2", "192.168.1.2/x", "evil.local.example.com", "[::1]:47321", "100.128.0.1"] {
            assert!(collector_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reports_carry_the_build_and_the_platform() {
        assert_eq!(stamp(r#"{"kind":"perf","m":1}"#, "0.10.0 (abc) TV").unwrap(), r#"{"build":"0.10.0 (abc) TV","platform":"tv","kind":"perf","m":1}"#);
        assert_eq!(stamp(r#"{"build":"x","a":1}"#, "y").unwrap(), r#"{"platform":"tv","build":"x","a":1}"#);
        assert_eq!(stamp("{}", "b").unwrap(), r#"{"build":"b","platform":"tv"}"#);
        assert!(stamp("[1]", "b").is_none() && stamp("ytc-dev-check", "b").is_none());
        assert!(stamp(&format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_REPORT)), "b").is_none());
        let v: serde_json::Value = serde_json::from_str(&stamp(r#"{"a":"q\"}"}"#, "b\"c").unwrap()).unwrap();
        assert_eq!((v["build"].as_str(), v["platform"].as_str(), v["a"].as_str()), (Some("b\"c"), Some("tv"), Some("q\"}")));
    }

    #[test]
    fn incidents_lose_the_query() {
        let v: serde_json::Value = serde_json::from_str(&scrub(r#"{"kind":"js-error","path":"/results?search_query=secret#x","reason":"e"}"#)).unwrap();
        assert_eq!((v["path"].as_str(), v["reason"].as_str()), (Some("/results"), Some("e")));
        assert_eq!(scrub("not json"), "not json");
        assert!(!scrub(r#"{"path":"/watch?v=abc"}"#).contains("abc"));
    }

    #[test]
    fn the_queue_keeps_the_newest_of_each_kind() {
        let mut q = Queue::default();
        for i in 0..40 {
            q.push(true, format!("p{i}"));
        }
        q.push(false, "i0".into());
        assert_eq!((q.len(), q.front()), (31, Some("p10")));
        q.delivered("p9");
        assert_eq!(q.len(), 31);
        q.delivered("p10");
        assert_eq!(q.front(), Some("p11"));
        for i in 0..120 {
            q.push(false, format!("i{i}"));
        }
        assert_eq!(q.len(), 29 + 100);
    }

    #[test]
    fn settings_round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("lumen-tel-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("telemetry.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings { on: true, url: "http://192.168.1.20:47321".into() };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        std::fs::write(&p, r#"{"on":true,"url":"http://203.0.113.7:47321"}"#).unwrap();
        assert_eq!(Settings::load(&p), Settings::default(), "an outside address turns it off");
        std::fs::write(&p, "junk").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_answer() {
        let from: SocketAddr = "192.168.1.20:47321".parse().unwrap();
        assert_eq!(answer_url(b"LUMEN-DC 47321", from).as_deref(), Some("http://192.168.1.20:47321"));
        assert_eq!(answer_url(b"LUMEN-DC 47321", "8.8.8.8:47321".parse().unwrap()), None);
        assert_eq!(answer_url(b"hello", from), None);
        // A responder on this machine.
        let srv = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = srv.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut b = [0u8; 32];
            if let Ok((n, peer)) = srv.recv_from(&mut b) {
                if &b[..n] == PROBE {
                    let _ = srv.send_to(b"LUMEN-DC 47999", peer);
                }
            }
        });
        assert_eq!(discover_at(&[at], Duration::from_secs(2)).unwrap(), "http://127.0.0.1:47999");
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(discover_at(&[silent.local_addr().unwrap()], Duration::from_millis(200)).is_err());
    }
}
