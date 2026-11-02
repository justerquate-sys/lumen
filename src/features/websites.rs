//! Blocked websites from description links (`yourTubeBlockedWebsitesV1` = `{version:1, domains:[…]}`,
//! at most 50). Hostnames are normalized (lower case, no `www.`, no trailing
//! dot); a blocked domain covers its subdomains but never lookalike suffixes
//! (`example.com` does not block `notexample.com`). YouTube/Google redirect
//! wrappers are unwrapped; other shorteners are not resolved.
//!
//! Verdicts come only from a video's own primary description (anonymous,
//! bounded `next` requests with a field mask, two at a time, only while at
//! least one domain is blocked). Up to 500 verdicts live in memory; nothing
//! about them is stored. Unknown descriptions stay visible.

use crate::parse::{Link, Run, Watch};
use crate::util::{esc_into, pct_decode, query_param};
use crate::json::Value as In;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};

pub const KEY: &str = "yourTubeBlockedWebsitesV1";
pub const MAX: usize = 50;
pub const VERDICTS: usize = 500;
pub const CONCURRENT: usize = 2;
/// `next` response mask: the description and the current video id only.
pub const FIELDS: &str = "contents/twoColumnWatchNextResults/results,currentVideoEndpoint";

/// A hostname as typed or taken from a URL → normalized domain, or None.
pub fn normalize(input: &str) -> Option<String> {
    let v = input.trim();
    if v.is_empty() || v.len() > 2048 || v.contains(|c: char| c.is_whitespace() || c == '\\') {
        return None;
    }
    let rest = match v.find("://") {
        Some(i) => {
            let scheme = v[..i].to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return None;
            }
            &v[i + 3..]
        }
        None => v,
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.contains('@') {
        return None;
    }
    let host = match auth.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => {
            if !port.is_empty() {
                return None; // explicit ports are not website rules
            }
            h
        }
        _ => auth,
    };
    let host = host.to_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.len() > 253 || !host.contains('.') {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    for l in &labels {
        if l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-') || !l.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return None;
        }
    }
    let tld = labels.last()?;
    if tld.len() < 2 || !tld.chars().next()?.is_alphabetic() {
        return None;
    }
    Some(host.to_string())
}

const OWN_HOSTS: [&str; 6] = ["youtube.com", "m.youtube.com", "youtu.be", "google.com", "music.youtube.com", "youtube-nocookie.com"];

/// A link target → the external website's domain. Unwraps YouTube's
/// `/redirect?q=` and Google's `/url?q=` (twice at most); YouTube/Google
/// themselves are never a website rule.
pub fn destination(url: &str) -> Option<String> {
    let mut u = url.trim().to_string();
    for _ in 0..2 {
        let host = normalize(&u)?;
        let path = u.split_once("://").map(|x| x.1).unwrap_or(&u).split_once('/').map(|(_, p)| p).unwrap_or("");
        let (p, query) = path.split_once('?').unwrap_or((path, ""));
        let wrapped = ((host == "youtube.com" || host == "m.youtube.com") && p == "redirect") || (host == "google.com" && p == "url");
        if !wrapped {
            break;
        }
        let q = query.split('#').next().unwrap_or("");
        let inner = query_param(q, "q").or_else(|| query_param(q, "url"))?;
        if !(inner.starts_with("http://") || inner.starts_with("https://")) {
            return None;
        }
        u = inner;
    }
    let host = normalize(&u)?;
    (!OWN_HOSTS.contains(&host.as_str()) && !host.ends_with(".youtube.com")).then_some(host)
}

/// Whether `host` is `domain` or one of its subdomains.
pub fn covers(domain: &str, host: &str) -> bool {
    host == domain || (host.len() > domain.len() && host.ends_with(domain) && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// URL-like tokens in plain description text ("https://x.com/a", "x.com/a").
pub fn text_hosts(text: &str, out: &mut HashSet<String>) {
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '(' | '[' | '{')) {
        let t = token.trim_end_matches(['.', ',', ';', '!', '?', ')', ']', ':']);
        if t.is_empty() || t.contains('@') {
            continue;
        }
        let explicit = t.starts_with("http://") || t.starts_with("https://");
        if !explicit {
            // A bare domain needs a dot and a letter-only TLD before any path.
            let host = t.split(['/', '?', '#']).next().unwrap_or("");
            let tld_ok = host.rsplit('.').next().is_some_and(|x| x.len() >= 2 && x.chars().all(char::is_alphabetic));
            if !host.contains('.') || !tld_ok {
                continue;
            }
        }
        if let Some(h) = destination(t) {
            out.insert(h);
        }
    }
}

/// Every website domain a description links to (links and plain text).
pub fn description_hosts(desc: &[Run]) -> HashSet<String> {
    let mut out = HashSet::new();
    for r in desc {
        match &r.link {
            Some(Link::Ext(u)) => {
                if let Some(h) = destination(u) {
                    out.insert(h);
                }
            }
            Some(_) => {}
            None => text_hosts(&r.text, &mut out),
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub domains: Vec<String>,
}

impl Rules {
    pub fn from_json(v: &In) -> Rules {
        let mut domains = Vec::new();
        for d in v.get("domains").and_then(In::as_array).into_iter().flatten() {
            if let Some(n) = d.as_str().and_then(normalize) {
                if !domains.contains(&n) && domains.len() < MAX {
                    domains.push(n);
                }
            }
        }
        Rules { domains }
    }
    pub fn to_json(&self) -> String {
        json!({"version": 1, "domains": self.domains}).to_string()
    }
    pub fn add(&mut self, input: &str) -> Result<String, &'static str> {
        let d = normalize(input).or_else(|| destination(input)).ok_or("Enter a valid website domain.")?;
        if self.domains.contains(&d) {
            return Ok(d);
        }
        if self.domains.len() >= MAX {
            return Err("Use at most 50 websites.");
        }
        self.domains.push(d.clone());
        Ok(d)
    }
    pub fn remove(&mut self, d: &str) -> bool {
        let n = self.domains.len();
        self.domains.retain(|x| x != d);
        n != self.domains.len()
    }
    /// The blocked domains `hosts` hit.
    pub fn matched(&self, hosts: &HashSet<String>) -> Vec<String> {
        self.domains.iter().filter(|d| hosts.iter().any(|h| covers(d, h))).cloned().collect()
    }
    pub fn blocks_host(&self, host: &str) -> bool {
        self.domains.iter().any(|d| covers(d, host))
    }
}

/// Verdict for a video id from its own description.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Allowed,
    Blocked,
}

/// Description-based verdicts and the bounded check queue (memory only).
#[derive(Default)]
pub struct Checker {
    /// Description hosts per video (not the description), newest last.
    hosts: HashMap<String, HashSet<String>>,
    lru: VecDeque<String>,
    pub queue: VecDeque<String>,
    pub inflight: HashMap<u32, String>,
    /// Videos already queued or answered unknown this session.
    tried: HashSet<String>,
}

impl Checker {
    pub fn learn(&mut self, id: &str, hosts: HashSet<String>) {
        if !self.hosts.contains_key(id) {
            self.lru.push_back(id.to_string());
        }
        self.hosts.insert(id.to_string(), hosts);
        while self.lru.len() > VERDICTS {
            if let Some(old) = self.lru.pop_front() {
                self.hosts.remove(&old);
            }
        }
    }
    pub fn verdict(&self, id: &str, rules: &Rules) -> Option<Verdict> {
        let h = self.hosts.get(id)?;
        Some(if rules.matched(h).is_empty() { Verdict::Allowed } else { Verdict::Blocked })
    }
    pub fn known(&self, id: &str) -> bool {
        self.hosts.contains_key(id)
    }
    /// Queues ids without a verdict (each at most once per session).
    pub fn want(&mut self, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            if !self.known(&id) && self.tried.insert(id.clone()) {
                self.queue.push_back(id);
            }
        }
    }
    /// Next id to request while fewer than two checks run.
    pub fn next_job(&mut self) -> Option<String> {
        if self.inflight.len() >= CONCURRENT {
            return None;
        }
        while let Some(id) = self.queue.pop_front() {
            if !self.known(&id) {
                return Some(id);
            }
        }
        None
    }
    pub fn clear_queue(&mut self) {
        for id in self.queue.drain(..) {
            self.tried.remove(&id);
        }
    }
    pub fn len(&self) -> usize {
        self.hosts.len()
    }
}

impl Checker {
    /// Checks for these ids go first (cards held back right now).
    pub fn prioritize(&mut self, ids: &[String]) {
        for id in ids.iter().rev() {
            if self.known(id) {
                continue;
            }
            self.queue.retain(|q| q != id);
            self.tried.insert(id.clone());
            self.queue.push_front(id.clone());
        }
    }
}

/// Held cards are shown anyway when no check answered for this long (a
/// stalled or failing check pipeline: unknown stays visible). While checks
/// keep answering, cards wait their turn (they were appended below the
/// viewport, so waiting is invisible).
pub const HOLD_MS: f64 = 4000.0;

/// What holding back cost (debug overlay, e2e).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoldStats {
    pub held: u32,
    pub shown: u32,
    pub blocked: u32,
    pub timed_out: u32,
    pub wait_sum_ms: f64,
    pub wait_max_ms: f64,
}

/// Cards of a newly appended batch that have no verdict yet are kept hidden
/// until their own description was checked (blocked videos must never
/// flash on feeds). The first screen is never held.
#[derive(Default)]
pub struct Hold {
    held: Vec<(String, f64)>,
    /// Last time a held card was added to an empty hold or settled.
    progress: f64,
    pub stats: HoldStats,
}

impl Hold {
    /// The ids of a batch to hold: those without a verdict.
    pub fn pick(ids: &[String], known: &dyn Fn(&str) -> bool) -> Vec<String> {
        ids.iter().filter(|id| !known(id)).cloned().collect()
    }
    pub fn add(&mut self, ids: &[String], now: f64) {
        if self.held.is_empty() {
            self.progress = now;
        }
        for id in ids {
            if !self.is_held(id) {
                self.held.push((id.clone(), now));
                self.stats.held += 1;
            }
        }
    }
    pub fn ids(&self) -> Vec<String> {
        self.held.iter().map(|h| h.0.clone()).collect()
    }
    pub fn is_held(&self, id: &str) -> bool {
        self.held.iter().any(|h| h.0 == id)
    }
    pub fn len(&self) -> usize {
        self.held.len()
    }
    /// A verdict (or a failed check) arrived: true when `id` was held.
    pub fn settle(&mut self, id: &str, now: f64, blocked: bool) -> bool {
        let Some(i) = self.held.iter().position(|h| h.0 == id) else { return false };
        let (_, since) = self.held.remove(i);
        self.progress = now;
        let w = now - since;
        self.stats.wait_sum_ms += w;
        self.stats.wait_max_ms = self.stats.wait_max_ms.max(w);
        if blocked { self.stats.blocked += 1 } else { self.stats.shown += 1 }
        true
    }
    /// Everything held when no check answered for `HOLD_MS` (shown unchecked).
    pub fn expire(&mut self, now: f64) -> Vec<String> {
        if self.held.is_empty() || now - self.progress < HOLD_MS {
            return Vec::new();
        }
        let old: Vec<(String, f64)> = self.held.drain(..).collect();
        for (_, since) in &old {
            self.stats.timed_out += 1;
            self.stats.wait_max_ms = self.stats.wait_max_ms.max(now - since);
            self.stats.wait_sum_ms += now - since;
        }
        old.into_iter().map(|h| h.0).collect()
    }
    /// Everything held (page left or re-rendered): released without a verdict.
    pub fn clear(&mut self) -> Vec<String> {
        self.held.drain(..).map(|h| h.0).collect()
    }
}

/// `.c` cards of these ids inside `#list`.
pub fn card_selector(list: &str, ids: &[String]) -> String {
    ids.iter().filter(|id| crate::util::is_video_id(id)).map(|id| format!("#{list} .c[data-v=\"{id}\"]")).collect::<Vec<_>>().join(",")
}

/// Anonymous masked `next` body for one video.
pub fn check_body(version: &str, id: &str) -> String {
    json!({"context": {"client": {"clientName": "WEB", "clientVersion": version, "hl": "en"}}, "videoId": id}).to_string()
}

pub fn check_url() -> String {
    format!("https://www.youtube.com/youtubei/v1/next?prettyPrint=false&fields={}", crate::util::pct_encode(FIELDS))
}

/// Hosts from a masked `next` response, only when it is about `id` and has a description.
pub fn hosts_from_next(w: &Watch, id: &str) -> Option<HashSet<String>> {
    (w.id == id && !w.desc.is_empty()).then(|| description_hosts(&w.desc))
}

/// Block control placed right after a description link.
pub fn link_control(out: &mut String, url: &str, rules: &Rules) {
    let Some(host) = destination(url) else { return };
    let blocked = rules.blocks_host(&host);
    out.push_str("<button type=\"button\" class=\"fx-blk");
    out.push_str(if blocked { " on\" data-act=\"fx-unblock\"" } else { "\" data-act=\"fx-block\"" });
    out.push_str(" data-arg=\"");
    esc_into(out, &host);
    out.push_str("\">");
    if blocked {
        out.push_str("Blocked · Undo");
    } else {
        out.push_str("Block ");
        esc_into(out, &host);
    }
    out.push_str("</button>");
}

/// Decodes a percent-encoded domain argument (defensive; args are plain domains).
pub fn arg_domain(arg: &str) -> Option<String> {
    normalize(&pct_decode(arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_domains() {
        assert_eq!(normalize("WWW.Example.COM"), Some("example.com".into()));
        assert_eq!(normalize("https://shop.example.com/path?q=1#x"), Some("shop.example.com".into()));
        assert_eq!(normalize("example.com."), Some("example.com".into()));
        assert_eq!(normalize("bücher.de"), Some("bücher.de".into()));
        assert_eq!(normalize("javascript:alert(1)"), None);
        assert_eq!(normalize("ftp://x.com"), None);
        assert_eq!(normalize("https://user@x.com"), None);
        assert_eq!(normalize("https://x.com:8080/"), None);
        assert_eq!(normalize("localhost"), None);
        assert_eq!(normalize("-bad.com"), None);
        assert_eq!(normalize("a b.com"), None);
        assert_eq!(normalize("1.2.3.4"), None);
    }

    #[test]
    fn destinations_unwrap_redirects() {
        assert_eq!(destination("https://www.youtube.com/redirect?event=video_description&q=https%3A%2F%2Fwww.example.com%2Fref%3Fa%3D1"), Some("example.com".into()));
        assert_eq!(destination("https://www.google.com/url?q=https://go.example.org/x"), Some("go.example.org".into()));
        assert_eq!(destination("https://www.youtube.com/watch?v=x"), None);
        assert_eq!(destination("https://youtu.be/abc"), None);
        assert_eq!(destination("https://www.youtube.com/redirect?q=javascript:alert(1)"), None);
        assert_eq!(destination("https://bit.ly/abc"), Some("bit.ly".into()), "shorteners are not resolved");
    }

    #[test]
    fn subdomains_not_lookalikes() {
        assert!(covers("example.com", "example.com"));
        assert!(covers("example.com", "shop.example.com"));
        assert!(!covers("example.com", "notexample.com"));
        assert!(!covers("example.com", "example.com.evil.net"));
    }

    #[test]
    fn hosts_from_description_text_and_links() {
        let desc = vec![
            Run { text: "Get it at example.com/ref, or https://Shop.Example.org/x). Mail me@mail.com. Version 1.2 is out".into(), link: None },
            Run { text: "link".into(), link: Some(Link::Ext("https://www.youtube.com/redirect?q=https%3A%2F%2Fpatreon.com%2Fx".into())) },
            Run { text: "video".into(), link: Some(Link::Int("/watch?v=abc".into())) },
        ];
        let h = description_hosts(&desc);
        let mut v: Vec<_> = h.into_iter().collect();
        v.sort();
        assert_eq!(v, vec!["example.com", "patreon.com", "shop.example.org"]);
    }

    #[test]
    fn rules_store_and_cap() {
        let mut r = Rules::default();
        assert_eq!(r.add("https://www.Example.com/x"), Ok("example.com".into()));
        assert_eq!(r.add("example.com"), Ok("example.com".into()));
        assert_eq!(r.domains.len(), 1);
        assert!(r.add("not a domain").is_err());
        for i in 0..60 {
            let _ = r.add(&format!("site{i}.com"));
        }
        assert_eq!(r.domains.len(), MAX);
        assert!(r.add("one-more.com").is_err());
        let v: serde_json::Value = serde_json::from_str(&r.to_json()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(Rules::from_json(&In::from_serde(&v)), r);
        assert!(r.remove("example.com"));
        let bad = crate::json::jv!({"domains": ["ok.com", "OK.com", "bad domain", 5]});
        assert_eq!(Rules::from_json(&bad).domains, vec!["ok.com"]);
    }

    #[test]
    fn checker_bounds() {
        let mut c = Checker::default();
        let mut r = Rules::default();
        r.add("example.com").unwrap();
        c.learn("aaaaaaaaaaa", ["shop.example.com".to_string()].into_iter().collect());
        c.learn("bbbbbbbbbbb", HashSet::new());
        assert_eq!(c.verdict("aaaaaaaaaaa", &r), Some(Verdict::Blocked));
        assert_eq!(c.verdict("bbbbbbbbbbb", &r), Some(Verdict::Allowed));
        assert_eq!(c.verdict("ccccccccccc", &r), None);
        // Rule changes re-evaluate without refetching (hosts, not verdicts, are kept).
        r.remove("example.com");
        assert_eq!(c.verdict("aaaaaaaaaaa", &r), Some(Verdict::Allowed));
        c.want(["aaaaaaaaaaa".into(), "ccccccccccc".into(), "ccccccccccc".into(), "ddddddddddd".into(), "eeeeeeeeeee".into()]);
        assert_eq!(c.queue.len(), 3);
        let a = c.next_job().unwrap();
        c.inflight.insert(1, a);
        let b = c.next_job().unwrap();
        c.inflight.insert(2, b);
        assert_eq!(c.next_job(), None, "at most two at a time");
        for i in 0..(VERDICTS + 10) {
            c.learn(&format!("v{i:010}"), HashSet::new());
        }
        assert_eq!(c.len(), VERDICTS);
        let body: serde_json::Value = serde_json::from_str(&check_body("2.1", "aaaaaaaaaaa")).unwrap();
        assert_eq!(body["videoId"], "aaaaaaaaaaa");
        assert!(check_url().contains("fields=contents%2FtwoColumnWatchNextResults%2Fresults%2CcurrentVideoEndpoint"));
    }

    #[test]
    fn hold_back_new_cards() {
        let ids: Vec<String> = ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"].iter().map(|s| s.to_string()).collect();
        let held = Hold::pick(&ids, &|id| id == "bbbbbbbbbbb");
        assert_eq!(held, vec!["aaaaaaaaaaa", "ccccccccccc"], "verdicts already known (hover/visit) are not held");
        let mut h = Hold::default();
        h.add(&held, 1000.0);
        h.add(&held, 1100.0);
        assert_eq!(h.len(), 2);
        assert!(h.settle("aaaaaaaaaaa", 1300.0, false));
        assert!(!h.settle("aaaaaaaaaaa", 1300.0, false), "once");
        assert!(!h.settle("bbbbbbbbbbb", 1300.0, true), "never held");
        assert!(h.expire(1000.0 + HOLD_MS).is_empty(), "a check answered at 1300: still progressing");
        assert_eq!(h.expire(1300.0 + HOLD_MS), vec!["ccccccccccc"], "no answer for HOLD_MS");
        assert_eq!(h.stats, HoldStats { held: 2, shown: 1, blocked: 0, timed_out: 1, wait_sum_ms: 300.0 + 300.0 + HOLD_MS, wait_max_ms: 300.0 + HOLD_MS });
        h.add(&ids[..1], 0.0);
        assert_eq!(h.clear(), vec!["aaaaaaaaaaa"]);
        assert_eq!(card_selector("ytc-list", &ids[..2]), "#ytc-list .c[data-v=\"aaaaaaaaaaa\"],#ytc-list .c[data-v=\"bbbbbbbbbbb\"]");
        assert_eq!(card_selector("ytc-list", &["x\"]".to_string()]), "", "ids are validated");
        // Held cards jump the check queue.
        let mut c = Checker::default();
        c.want(ids.clone());
        c.prioritize(&["ccccccccccc".to_string(), "ddddddddddd".to_string()]);
        assert_eq!(c.queue.iter().cloned().collect::<Vec<_>>(), vec!["ccccccccccc", "ddddddddddd", "aaaaaaaaaaa", "bbbbbbbbbbb"]);
        c.learn("eeeeeeeeeee", HashSet::new());
        c.prioritize(&["eeeeeeeeeee".to_string()]);
        assert_eq!(c.queue.len(), 4, "known verdicts are not requested");
    }

    #[test]
    fn link_control_markup() {
        let mut r = Rules::default();
        let mut out = String::new();
        link_control(&mut out, "https://www.youtube.com/redirect?q=https%3A%2F%2Fexample.com%2F", &r);
        assert!(out.contains("data-act=\"fx-block\" data-arg=\"example.com\">Block example.com"));
        r.add("example.com").unwrap();
        out.clear();
        link_control(&mut out, "https://shop.example.com/", &r);
        assert!(out.contains("fx-unblock") && out.contains("Blocked · Undo") && out.contains("data-arg=\"shop.example.com\""));
        out.clear();
        link_control(&mut out, "https://www.youtube.com/watch?v=x", &r);
        assert!(out.is_empty());
    }
}
