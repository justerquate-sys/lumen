//! Custom Home topic feeds (
//! `yourTubeRecommendationTopicsV1` = `{version:1, topics:[string]}`, ≤ 20
//! topics of ≤ 160 characters).
//!
//! 1 Nov 2026: topics must be fast and fresh, following the same pattern
//! YouTube uses for its own topics. One anonymous `search` for the topic with
//! YouTube's own filter params for This month + Videos gives the first
//! screen; This year and any date (and their continuations) enrich in the
//! background, fresher tiers always ranked first. Candidates are scored
//! lexically against the topic (Roblox topics need tutorial/technical
//! evidence), deduplicated, at most three per channel per batch. The
//! earlier seed → related chain (≈ 15 s, old videos) is gone.
//!
//! Bounds: 30 requests and 160 cards per session, the first search alone,
//! then two at a time, requests only on selection or scrolling, everything
//! in memory, aborted on navigation.

use crate::parse::{Feed, Item, Video};
use crate::json::Value as In;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};

pub const KEY: &str = "yourTubeRecommendationTopicsV1";
pub const MAX_TOPICS: usize = 20;
pub const TOPIC_CHARS: usize = 160;
pub const MAX_REQUESTS: u32 = 30;
pub const MAX_CARDS: usize = 160;
pub const BATCH: usize = 12;
/// The first delivery fills a screen.
pub const FIRST: usize = 20;
pub const PER_CHANNEL: usize = 3;
pub const CONCURRENT: usize = 2;
/// Requests started per "more" (selection, scroll) at most.
pub const READS_PER_ADVANCE: u32 = 3;
const MAX_SEEN: usize = 4096;
const MAX_TOKENS: usize = 90;
/// The official Roblox channel (RDC talks count as technical context).
const ROBLOX_CHANNEL: &str = "UCjiPEaapiHbJMoAdi_L8fNA";

const ALIASES: &[(&str, &[&str])] = &[
    ("animation", &["animation", "animations", "animate", "animated", "animating", "animator", "rigging", "keyframe", "keyframes"]),
    ("motion", &["motion", "movement", "camera", "cameras", "bobbing", "shake", "spring", "springs", "tween", "tweens", "tweening", "footstep", "footsteps"]),
    ("scripting", &["scripting", "script", "scripts", "luau", "lua", "programming", "coding", "code"]),
    ("development", &["development", "develop", "developer", "developers", "dev", "devs", "create", "creating", "make", "making", "build", "building", "design", "scripting", "coding", "programming", "studio"]),
    ("monetization", &["monetization", "monetisation", "monetize", "monetise", "revenue", "pricing", "economy", "economies", "gamepass", "gamepasses", "game pass", "game passes", "developer product", "developer products"]),
    ("ads", &["ads", "advertising", "advertisement", "advertisements", "ad creative", "ad creatives", "marketing", "sponsoring"]),
    ("growth", &["growth", "grow", "acquisition", "discovery", "algorithm", "analytics", "ccu", "players"]),
    ("retention", &["retention", "retain", "onboarding", "engagement", "session time", "d1", "d7"]),
    ("vfx", &["vfx", "effects", "particle", "particles", "visual effects", "impact frames"]),
    ("building", &["building", "build", "modeling", "modelling", "model", "models", "terrain", "lighting", "environment"]),
    ("performance", &["performance", "optimization", "optimisation", "optimize", "optimise", "profiling", "lag", "fps", "microprofiler"]),
    ("ui", &["ui", "gui", "ux", "interface", "interfaces", "hud", "menus", "menu"]),
    ("feel", &["feel", "juice", "juicy", "juicing", "feedback", "movement", "physics", "camera", "controls", "controller"]),
];

const STOP: &[&str] = &["a", "an", "the", "of", "for", "to", "and", "or", "in", "on", "with", "about", "from", "my", "me", "i", "want", "find", "show", "best", "videos", "video", "tutorial", "tutorials", "learn", "learning", "how", "please"];

/// Lower case, every run of non-alphanumerics → one space.
pub fn words(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else {
            space = true;
        }
    }
    out
}

fn phrase(value: &str, p: &str) -> bool {
    format!(" {value} ").contains(&format!(" {p} "))
}

fn any_phrase(value: &str, list: &[&str]) -> bool {
    list.iter().any(|p| phrase(value, p))
}

/// A topic string as stored: control characters removed, whitespace
/// collapsed, 1–160 characters.
pub fn clean_topic(s: &str) -> Option<String> {
    let t: Vec<&str> = s.split(|c: char| c.is_whitespace() || c.is_control() || c == '\u{feff}').filter(|w| !w.is_empty()).collect();
    let t = t.join(" ");
    (!t.is_empty() && t.encode_utf16().count() <= TOPIC_CHARS).then_some(t)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: String,
    pub terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    pub topic: String,
    pub query: String,
    pub normalized: String,
    pub roblox: bool,
    pub practical: bool,
    pub groups: Vec<Group>,
}

pub fn create(s: &str) -> Result<Topic, &'static str> {
    let topic = clean_topic(s).ok_or("Use a topic of 1–160 characters.")?;
    let normalized = words(&topic);
    let mut seen = HashSet::new();
    let tokens: Vec<&str> = normalized.split(' ').filter(|t| !t.is_empty() && !STOP.contains(t) && seen.insert(*t)).collect();
    if tokens.is_empty() {
        return Err("Enter a specific topic.");
    }
    let roblox = tokens.contains(&"roblox") || tokens.contains(&"luau") || phrase(&normalized, "moon animator") || phrase(&normalized, "eclipse animator");
    let mut groups = Vec::new();
    let mut assigned = HashSet::new();
    for &(label, variants) in ALIASES {
        let activate = tokens.iter().any(|t| {
            *t == label
                || match label {
                    "animation" => ["animations", "animate", "animator", "animating"].contains(t),
                    "scripting" => ["script", "scripts", "luau", "lua", "programming", "coding", "code"].contains(t),
                    "development" => ["develop", "developer", "developers", "dev", "design"].contains(t),
                    "monetization" => ["monetisation", "monetize", "monetise"].contains(t),
                    "ads" => ["advertising", "marketing"].contains(t),
                    "performance" => ["optimization", "optimisation"].contains(t),
                    _ => false,
                }
        });
        if !activate {
            continue;
        }
        groups.push(Group { label: label.into(), terms: variants.iter().map(|v| v.to_string()).collect() });
        for t in &tokens {
            if variants.contains(t) || *t == label {
                assigned.insert(*t);
            }
        }
    }
    for t in &tokens {
        if assigned.contains(t) || (roblox && ["roblox", "studio", "moon", "eclipse", "animator"].contains(t)) || (*t == "game" && !groups.is_empty()) {
            continue;
        }
        groups.push(Group { label: t.to_string(), terms: vec![t.to_string()] });
    }
    let practical = roblox && groups.iter().any(|g| ALIASES.iter().any(|(l, _)| *l == g.label));
    let query = if practical && !any_phrase(&normalized, &["tutorial", "guide", "basics", "tips"]) { format!("{topic} tutorial") } else { topic.clone() };
    Ok(Topic { topic, query, normalized, roblox, practical, groups })
}

/// Score of a candidate for `t` (None = rejected). `rank` is its position in
/// the source list (earlier is slightly better).
pub fn score(v: &Video, t: &Topic, rank: usize) -> Option<f64> {
    let title = words(&v.title);
    let dur = super::filters::duration_secs(&v.duration);
    if title.is_empty() || v.live || dur.is_some_and(|d| d < 120) {
        return None;
    }
    if any_phrase(&title, &["gameplay", "walkthrough", "playthrough", "speedrun", "funny moments", "lets play", "let s play"]) {
        return None;
    }
    let raw_title = v.title.trim();
    let rdc = raw_title.rfind('|').is_some_and(|i| {
        let tail = raw_title[i + 1..].trim();
        tail.len() == 8 && tail.starts_with("RDC 20") && tail[6..].bytes().all(|b| b.is_ascii_digit())
    });
    let developer = t.roblox && t.practical && v.channel_bid.as_deref() == Some(ROBLOX_CHANNEL) && rdc && dur.is_some_and(|d| d >= 120);
    if developer && any_phrase(&title, &["film", "films", "react", "reacts", "reaction", "reactions"]) {
        return None;
    }
    if t.roblox && !developer && !any_phrase(&title, &["roblox", "moon animator", "eclipse animator", "luau"]) {
        return None;
    }
    let matches = t.groups.iter().filter(|g| g.terms.iter().any(|term| phrase(&title, term))).count();
    if !t.groups.is_empty() && (matches as f64) < (t.groups.len() as f64 * 0.6).ceil() {
        return None;
    }
    let instructional = developer
        || any_phrase(&title, &["tutorial", "guide", "explained", "how to", "how it works", "tips", "basics", "breakdown", "full process", "workflow", "workflows", "best practices", "for beginners"]);
    if t.practical && (!instructional || any_phrase(&title, &["challenge", "challenges", "funniest", "funny", "compilation", "movie", "music video", "meme"])) {
        return None;
    }
    Some(
        matches as f64 * 3.0
            + if t.roblox { 2.0 } else { 0.0 }
            + if instructional { 2.0 } else { 0.0 }
            + if phrase(&title, &t.normalized) { 1.0 } else { 0.0 }
            + 1.0 / (1.0 + rank as f64),
    )
}

// ------------------------------------------------------------------ store

pub fn topics_from_json(v: &In) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in v.get("topics").and_then(In::as_array).into_iter().flatten() {
        if let Some(c) = t.as_str().and_then(clean_topic) {
            if !out.iter().any(|o| o.to_lowercase() == c.to_lowercase()) && out.len() < MAX_TOPICS {
                out.push(c);
            }
        }
    }
    out
}

pub fn topics_json(topics: &[String]) -> String {
    json!({"version": 1, "topics": topics}).to_string()
}

/// Adds a topic (validated, deduplicated, ≤ 20). Returns its index.
pub fn add_topic(topics: &mut Vec<String>, s: &str) -> Result<usize, &'static str> {
    let t = create(s)?.topic;
    if let Some(i) = topics.iter().position(|o| o.to_lowercase() == t.to_lowercase()) {
        return Ok(i);
    }
    if topics.len() >= MAX_TOPICS {
        return Err("Use at most 20 topics.");
    }
    topics.push(t);
    Ok(topics.len() - 1)
}

// ---------------------------------------------------------------- session

/// YouTube's own search-filter `params` (as its filter dialog returns them),
/// freshest first: This month + Videos, This year + Videos, Videos.
pub const LADDER: [&str; 3] = ["EgQIBBAB", "EgQIBRAB", "EgIQAQ%3D%3D"];

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// The topic search with `LADDER[i]` (0 = freshest).
    Find(u8),
    /// A search continuation (tier of the search it came from).
    More(u8, String),
}

impl Job {
    fn tier(&self) -> u8 {
        match self {
            Job::Find(t) | Job::More(t, _) => *t,
        }
    }
}

/// One topic's feed session (memory only).
pub struct Session {
    pub topic: Topic,
    seen: HashSet<String>,
    tokens: HashSet<String>,
    pub queue: VecDeque<Job>,
    /// Scored candidates waiting for delivery: (score, video).
    pending: Vec<(f64, Video)>,
    pub inflight: HashMap<u32, Job>,
    pub requests: u32,
    pub delivered: usize,
    /// The freshest search answered (or failed): the first cards can go out.
    pub initialized: bool,
    pub halted: bool,
    pub failures: u32,
    pub last_error: Option<String>,
    /// Requests started since the last delivery (bounded per advance).
    pub reads: u32,
}

impl Session {
    /// `exclude` = videos already on the Home page (never repeated).
    pub fn new(topic: Topic, exclude: &[String]) -> Session {
        let seen: HashSet<String> = exclude.iter().take(MAX_SEEN / 2).cloned().collect();
        Session {
            topic,
            seen,
            tokens: HashSet::new(),
            queue: (0..LADDER.len() as u8).map(Job::Find).collect(),
            pending: Vec::new(),
            inflight: HashMap::new(),
            requests: 0,
            delivered: 0,
            initialized: false,
            halted: false,
            failures: 0,
            last_error: None,
            reads: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        MAX_CARDS.saturating_sub(self.delivered)
    }

    fn capacity(&self) -> bool {
        !self.halted && self.requests < MAX_REQUESTS && self.remaining() > self.pending.len() && self.seen.len() < MAX_SEEN
    }

    /// Jobs to start now (respects every bound). The freshest search runs
    /// alone so its cards are first; the rest enrich in the background.
    pub fn jobs(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        let limit = if self.initialized { CONCURRENT } else { 1 };
        while self.pending.len() < BATCH * 2 && self.inflight.len() + out.len() < limit && self.reads < READS_PER_ADVANCE && self.capacity() {
            let Some(j) = self.queue.pop_front() else { break };
            self.requests += 1;
            self.reads += 1;
            out.push(j);
        }
        out
    }

    /// A search page (`job`'s tier): scored candidates, fresher tiers first.
    pub fn on_results(&mut self, job: &Job, feed: &Feed) {
        self.initialized = true;
        let bonus = (LADDER.len() as u8 - job.tier()) as f64 * 10.0;
        // Topic feeds take ordinary videos only (Shorts never rank or fill slots).
        for (rank, it) in feed.items.iter().filter(|i| !i.is_short()).enumerate() {
            let Item::Video(v) = it else { continue };
            if self.seen.contains(&v.id) {
                continue;
            }
            if self.seen.len() >= MAX_SEEN {
                self.halted = true;
                break;
            }
            self.seen.insert(v.id.clone());
            if self.remaining() <= self.pending.len() {
                continue;
            }
            if let Some(s) = score(v, &self.topic, rank) {
                self.pending.push((s + bonus, v.clone()));
            }
        }
        if let Some(t) = &feed.cont {
            if !self.tokens.contains(t) && self.tokens.len() < MAX_TOKENS {
                self.tokens.insert(t.clone());
                // Fresher pages stay ahead of older tiers' first pages.
                let at = self.queue.iter().position(|q| q.tier() > job.tier()).unwrap_or(self.queue.len());
                self.queue.insert(at, Job::More(job.tier(), t.clone()));
            }
        }
    }

    pub fn on_error(&mut self, _job: &Job, why: &str) {
        self.failures += 1;
        self.last_error = Some(why.chars().take(120).collect());
        self.initialized = true;
        if self.failures >= 3 {
            self.halted = true;
        }
    }

    /// Whether cards can be delivered now (enough waiting, or nothing more coming).
    pub fn ready(&self) -> bool {
        self.initialized && (self.pending.len() >= BATCH.min(self.remaining().max(1)) || self.inflight.is_empty())
    }

    /// The best waiting candidates (a full first screen, then BATCH), at most
    /// three per channel per batch; ties keep YouTube's order.
    pub fn take_batch(&mut self) -> Vec<Video> {
        super::filters::sort_keyed(&mut self.pending, |p| -p.0);
        let max = (if self.delivered == 0 { FIRST } else { BATCH }).min(self.remaining());
        let mut out = Vec::new();
        let mut keep = Vec::new();
        let mut per: HashMap<String, usize> = HashMap::new();
        for (s, v) in self.pending.drain(..) {
            let key = v.channel_bid.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| if v.channel.is_empty() { v.id.clone() } else { v.channel.clone() });
            let n = per.entry(key).or_default();
            if out.len() < max && *n < PER_CHANNEL {
                *n += 1;
                out.push(v);
            } else {
                keep.push((s, v));
            }
        }
        self.pending = keep;
        self.delivered += out.len();
        self.reads = 0;
        out
    }

    /// More cards can still come.
    pub fn more(&self) -> bool {
        self.remaining() > 0 && (!self.pending.is_empty() || !self.inflight.is_empty() || !self.initialized || (self.capacity() && !self.queue.is_empty()))
    }

    pub fn status(&self) -> String {
        format!(
            "topic \"{}\" · {} requests · {} delivered · {} waiting · {} queued · {} failures{}",
            self.topic.topic,
            self.requests,
            self.delivered,
            self.pending.len(),
            self.queue.len(),
            self.failures,
            self.last_error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default()
        )
    }
}

/// Anonymous search body (hl=en so the lexical rules apply), with the job's
/// YouTube filter params or continuation.
pub fn search_body(version: &str, q: &str, job: &Job) -> String {
    let mut b = json!({"context": {"client": {"clientName": "WEB", "clientVersion": version, "hl": "en", "gl": "US"}}});
    match job {
        Job::Find(t) => {
            b["query"] = q.into();
            b["params"] = LADDER[*t as usize % LADDER.len()].into();
        }
        Job::More(_, token) => b["continuation"] = token.clone().into(),
    }
    b.to_string()
}

pub const SEARCH_URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, title: &str, ch: &str, dur: &str) -> Video {
        Video { id: id.into(), title: title.into(), channel: ch.into(), channel_bid: Some(format!("UC{ch}")), duration: dur.into(), ..Default::default() }
    }

    #[test]
    fn topic_creation() {
        let t = create("  Roblox   animation ").unwrap();
        assert_eq!(t.topic, "Roblox animation");
        assert!(t.roblox && t.practical);
        assert_eq!(t.query, "Roblox animation tutorial");
        assert_eq!(t.groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), vec!["animation"]);
        let t = create("sourdough baking").unwrap();
        assert!(!t.roblox && !t.practical);
        assert_eq!(t.query, "sourdough baking");
        assert_eq!(t.groups.len(), 2);
        assert!(create("the best videos").is_err());
        assert!(create("").is_err());
        assert!(create(&"x".repeat(161)).is_err());
    }

    #[test]
    fn scoring_rules() {
        let t = create("Roblox animation").unwrap();
        assert!(score(&v("aaaaaaaaaaa", "Roblox Animation Tutorial for Beginners", "a", "12:00"), &t, 0).is_some());
        assert!(score(&v("aaaaaaaaaaa", "Roblox Animation Tutorial", "a", "1:00"), &t, 0).is_none(), "too short");
        assert!(score(&v("aaaaaaaaaaa", "Animation tutorial in Blender", "a", "12:00"), &t, 0).is_none(), "Roblox topic needs Roblox");
        assert!(score(&v("aaaaaaaaaaa", "Roblox animation funny moments", "a", "12:00"), &t, 0).is_none());
        assert!(score(&v("aaaaaaaaaaa", "Roblox animation movie", "a", "12:00"), &t, 0).is_none(), "practical needs instruction");
        let mut live = v("aaaaaaaaaaa", "Roblox animation tutorial", "a", "");
        live.live = true;
        assert!(score(&live, &t, 0).is_none());
        // RDC talks from the official channel count as technical context.
        let mut rdc = v("bbbbbbbbbbb", "Animating characters at scale | RDC 2024", "x", "30:00");
        rdc.channel_bid = Some(ROBLOX_CHANNEL.into());
        assert!(score(&rdc, &t, 0).is_some());
        rdc.channel_bid = Some("UCsomeoneelse".into());
        assert!(score(&rdc, &t, 0).is_none());
        let earlier = score(&v("a", "Roblox Animation Tutorial", "a", "12:00"), &t, 0).unwrap();
        let later = score(&v("a", "Roblox Animation Tutorial", "a", "12:00"), &t, 5).unwrap();
        assert!(earlier > later);
    }

    #[test]
    fn store_bounds() {
        let mut topics = Vec::new();
        assert_eq!(add_topic(&mut topics, "Roblox animation"), Ok(0));
        assert_eq!(add_topic(&mut topics, "roblox ANIMATION"), Ok(0));
        for i in 0..30 {
            let _ = add_topic(&mut topics, &format!("topic number {i}"));
        }
        assert_eq!(topics.len(), MAX_TOPICS);
        assert!(add_topic(&mut topics, "one more topic").is_err());
        let v: serde_json::Value = serde_json::from_str(&topics_json(&topics)).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(topics_from_json(&In::from_serde(&v)), topics);
        assert_eq!(topics_from_json(&crate::json::jv!({"topics": ["a", "A", 5, "", "b"]})), vec!["a", "b"]);
    }

    fn feed(items: Vec<Video>, cont: Option<&str>) -> Feed {
        Feed { items: items.into_iter().map(Item::Video).collect(), cont: cont.map(String::from), ..Default::default() }
    }

    #[test]
    fn session_flow_and_bounds() {
        let mut s = Session::new(create("Roblox animation").unwrap(), &["shownshown1".into()]);
        assert_eq!(s.jobs(), vec![Job::Find(0)], "the freshest search first, alone");
        s.inflight.insert(1, Job::Find(0));
        assert!(s.jobs().is_empty());
        let body: serde_json::Value = serde_json::from_str(&search_body("2.x", &s.topic.query, &Job::Find(0))).unwrap();
        assert_eq!((body["query"].as_str(), body["params"].as_str()), (Some("Roblox animation tutorial"), Some("EgQIBBAB")));
        s.inflight.clear();
        let mut month = vec![v("shownshown1", "Roblox animation tutorial", "z", "10:00"), v("offtopic001", "cooking pasta", "c", "10:00")];
        for i in 0..8 {
            month.push(v(&format!("month{i:06}"), &format!("Roblox animation tutorial part {i}"), if i < 5 { "same" } else { &"abcdefghij"[i..i + 1] }, "10:00"));
        }
        s.on_results(&Job::Find(0), &feed(month, Some("MCONT")));
        assert!(s.ready(), "first cards right after the first search");
        assert_eq!(s.queue.front(), Some(&Job::More(0, "MCONT".into())), "this month's next page before older tiers");
        let b = s.take_batch();
        assert!(b.iter().all(|x| x.id.starts_with("month")), "shown and off-topic dropped");
        assert_eq!(b.iter().filter(|x| x.channel == "same").count(), PER_CHANNEL);
        // Background enrichment two at a time; older tiers rank after fresher ones.
        let j = s.jobs();
        assert_eq!(j, vec![Job::More(0, "MCONT".into()), Job::Find(1)]);
        s.on_results(&Job::Find(1), &feed(vec![v("yearyear001", "Roblox animation tutorial guide", "y", "10:00")], None));
        s.on_results(&Job::More(0, "MCONT".into()), &feed(vec![v("month000099", "Roblox animation tutorial", "q", "10:00")], None));
        let b2 = s.take_batch();
        let pos = |id: &str| b2.iter().position(|x| x.id == id).unwrap();
        assert!(pos("month000099") < pos("yearyear001"), "fresher first");
        let body: serde_json::Value = serde_json::from_str(&search_body("2.x", "q", &Job::More(0, "T".into()))).unwrap();
        assert_eq!(body["continuation"], "T");
        s.requests = MAX_REQUESTS;
        assert!(s.jobs().is_empty(), "request cap");
    }

    #[test]
    fn card_cap() {
        let shown: Vec<String> = (0..200).map(|i| format!("s{i:010}")).collect();
        let mut s = Session::new(create("sourdough").unwrap(), &shown);
        assert_eq!(s.remaining(), MAX_CARDS, "Home cards are excluded, not counted");
        s.delivered = 158;
        s.queue.clear();
        s.on_results(&Job::Find(0), &feed((0..10).map(|i| v(&format!("c{i:010}"), "sourdough bread", &format!("ch{i}"), "10:00")).collect(), None));
        let b = s.take_batch();
        assert_eq!(b.len(), 2, "160-card bound");
        assert!(!s.more());
        for _ in 0..3 {
            s.on_error(&Job::Find(1), "x");
        }
        assert!(s.halted);
    }
}
