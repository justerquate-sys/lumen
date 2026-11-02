//! `extension/rules.json` (the extension's declarativeNetRequest rules) for the TV app's WebView, which
//! has no declarativeNetRequest: the shell's request hook asks [`Rules::decide`] for every request
//! the WebView makes. The subset of DNR the client's rules use is implemented with DNR's semantics:
//! `urlFilter` (`||` domain anchor, `|` start anchor, `^` separator, `*`), `requestDomains`,
//! `initiatorDomains` (the top document's host stands in for the initiator), `resourceTypes` (the
//! WebView only tells main-frame navigations apart), priorities (highest wins; on a tie
//! allowAllRequests beats block), `allowAllRequests` (matched against the top document) and the one
//! `modifyHeaders` use: `access-control-allow-origin` set on responses (the shell then fetches the
//! request itself and adds the header).

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Decision {
    Pass,
    Block,
    /// Fetch natively and set `Access-Control-Allow-Origin: *` on the answer.
    Cors,
}

#[derive(Deserialize)]
struct RawRule {
    priority: Option<i64>,
    action: RawAction,
    condition: RawCond,
}

#[derive(Deserialize)]
struct RawAction {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "responseHeaders")]
    response_headers: Vec<RawHeader>,
}

#[derive(Deserialize)]
struct RawHeader {
    header: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawCond {
    url_filter: Option<String>,
    request_domains: Vec<String>,
    initiator_domains: Vec<String>,
    resource_types: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    AllowAll,
    Block,
    Cors,
}

struct Rule {
    priority: i64,
    kind: Kind,
    filter: Option<String>,
    request_domains: Vec<String>,
    initiator_domains: Vec<String>,
    types: Vec<String>,
}

pub struct Rules {
    rules: Vec<Rule>,
}

/// The host of an absolute URL (lowercase).
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let hp = &rest[..end];
    let hp = hp.rsplit_once('@').map_or(hp, |x| x.1);
    hp.split(':').next().unwrap_or("")
}

fn domain_match(h: &str, domains: &[String]) -> bool {
    domains
        .iter()
        .any(|d| h == d || (h.len() > d.len() && h.ends_with(d.as_str()) && h.as_bytes()[h.len() - d.len() - 1] == b'.'))
}

fn is_sep(c: u8) -> bool {
    !(c.is_ascii_alphanumeric() || b"_-.%".contains(&c))
}

/// Matches pattern `p` (with `*` and `^`) at the start of `s`; `end` = must consume all of `s`.
fn match_here(p: &[u8], s: &[u8], end: bool) -> bool {
    match p.first() {
        None => !end || s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| match_here(&p[1..], &s[i..], end)),
        // `^` matches a separator or the end of the URL.
        Some(b'^') => match s.first() {
            None => match_here(&p[1..], s, end),
            Some(&c) => is_sep(c) && match_here(&p[1..], &s[1..], end),
        },
        Some(&c) => s.first().is_some_and(|&x| x == c) && match_here(&p[1..], &s[1..], end),
    }
}

/// DNR `urlFilter` against a URL (case-insensitive, as DNR by default).
pub fn url_filter(filter: &str, url: &str) -> bool {
    let f = filter.to_ascii_lowercase();
    let u = url.to_ascii_lowercase();
    let (f, end) = match f.strip_suffix('|') {
        Some(x) => (x.to_string(), true),
        None => (f, false),
    };
    if let Some(rest) = f.strip_prefix("||") {
        // Domain anchor: at the start of the host or of one of its labels.
        let Some(hstart) = u.find("://").map(|i| i + 3) else { return false };
        let h = host(&u);
        let mut starts = vec![hstart];
        for (i, b) in h.bytes().enumerate() {
            if b == b'.' {
                starts.push(hstart + i + 1);
            }
        }
        return starts.into_iter().any(|i| match_here(rest.as_bytes(), &u.as_bytes()[i..], end));
    }
    if let Some(rest) = f.strip_prefix('|') {
        return match_here(rest.as_bytes(), u.as_bytes(), end);
    }
    (0..=u.len()).any(|i| match_here(f.as_bytes(), &u.as_bytes()[i..], end))
}

impl Rules {
    pub fn parse(json: &str) -> Result<Rules, String> {
        let raw: Vec<RawRule> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let rules = raw
            .into_iter()
            .filter_map(|r| {
                let kind = match r.action.kind.as_str() {
                    "allowAllRequests" => Kind::AllowAll,
                    "block" => Kind::Block,
                    "modifyHeaders"
                        if r.action
                            .response_headers
                            .iter()
                            .any(|h| h.header.eq_ignore_ascii_case("access-control-allow-origin")) =>
                    {
                        Kind::Cors
                    }
                    _ => return None,
                };
                Some(Rule {
                    priority: r.priority.unwrap_or(1),
                    kind,
                    filter: r.condition.url_filter,
                    request_domains: r.condition.request_domains,
                    initiator_domains: r.condition.initiator_domains,
                    types: r.condition.resource_types,
                })
            })
            .collect();
        Ok(Rules { rules })
    }

    fn matches(r: &Rule, url: &str, main_frame: bool, top: &str) -> bool {
        // Resource types: no list = everything but main_frame (DNR's default).
        let typed = if r.types.is_empty() {
            !main_frame
        } else if main_frame {
            r.types.iter().any(|t| t == "main_frame")
        } else {
            r.types.iter().any(|t| t != "main_frame" && t != "sub_frame")
        };
        typed
            && (r.request_domains.is_empty() || domain_match(&host(url).to_ascii_lowercase(), &r.request_domains))
            && (r.initiator_domains.is_empty() || (!main_frame && domain_match(&host(top).to_ascii_lowercase(), &r.initiator_domains)))
            && r.filter.as_deref().is_none_or(|f| url_filter(f, url))
    }

    /// What happens to a request for `url` made while the top document is `top`.
    pub fn decide(&self, url: &str, main_frame: bool, top: &str) -> Decision {
        // allowAllRequests: matched against the document (a navigation to it, or the page it made).
        let doc = if main_frame { url } else { top };
        let allow = self
            .rules
            .iter()
            .filter(|r| r.kind == Kind::AllowAll && Self::matches(r, doc, true, doc))
            .map(|r| r.priority)
            .max();
        let block = self
            .rules
            .iter()
            .filter(|r| r.kind == Kind::Block && Self::matches(r, url, main_frame, top))
            .map(|r| r.priority)
            .max();
        let cors = self
            .rules
            .iter()
            .filter(|r| r.kind == Kind::Cors && Self::matches(r, url, main_frame, top))
            .map(|r| r.priority)
            .max();
        if let Some(b) = block {
            if allow.is_none_or(|a| b > a) {
                return Decision::Block;
            }
        }
        match cors {
            Some(c) if allow.is_none_or(|a| c > a) => Decision::Cors,
            _ => Decision::Pass,
        }
    }
}
