//! The watch-page description as the old YourTube extension presented it
//! (a port of the old extension's `resource`/`walk`/`resource_rows`): only lines that carry a resource link,
//! each with its own text minus hashtags; a bare link takes the line above it
//! as its label. Pure: no DOM, unit-tested below.

use crate::parse::{Link, Run};

/// JavaScript's `\s` (the old code's regexes ran in the page).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// `trim(value.replace(/\s+/g, " "))` (the old `case_normal`).
fn case_normal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in js_trim(s).chars() {
        if js_space(c) {
            if !gap {
                out.push(' ');
            }
            gap = true;
        } else {
            out.push(c);
            gap = false;
        }
    }
    out
}

/// `[\p{L}\p{N}_]` (letters approximated by `is_alphabetic`).
fn tag_char(c: char) -> bool {
    c == '_' || c.is_alphabetic() || c.is_numeric()
}

/// `value.replace(/(^|[\s(])#[\p{L}\p{N}_]+/gu, "$1")`.
pub fn strip_hashtags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        let starts = c == '#' && prev.is_none_or(|p| js_space(p) || p == '(') && it.peek().is_some_and(|&n| tag_char(n));
        if starts {
            let mut last = c;
            while let Some(&n) = it.peek() {
                if !tag_char(n) {
                    break;
                }
                last = n;
                it.next();
            }
            prev = Some(last);
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Every end of a `(?:\d+:)?\d{1,2}:\d{2}` match anchored at the start.
fn timestamp_ends(s: &str) -> Vec<usize> {
    let b = s.as_bytes();
    let digits = |from: usize| b[from.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    let tail = |at: usize, ends: &mut Vec<usize>| {
        for n in 1..=digits(at).min(2) {
            if b.get(at + n) == Some(&b':') && digits(at + n + 1) >= 2 {
                ends.push(at + n + 3);
            }
        }
    };
    let mut ends = Vec::new();
    for k in 1..=digits(0) {
        if b.get(k) == Some(&b':') {
            tail(k + 1, &mut ends);
        }
    }
    tail(0, &mut ends);
    ends
}

/// `/^(?:\d+:)?\d{1,2}:\d{2}$/` (a timestamp label such as `1:02:03`).
pub fn is_timestamp(s: &str) -> bool {
    timestamp_ends(s).contains(&s.len())
}

/// `/^(?:\d+:)?\d{1,2}:\d{2}/` (text that starts with a timestamp).
fn starts_with_timestamp(s: &str) -> bool {
    !timestamp_ends(s).is_empty()
}

fn param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then_some(v)
    })
}

/// The old `resource()`: a link run as a resource token (trimmed label, or the
/// destination when the label is empty), or `None` for hashtags, timestamps,
/// `/hashtag/` pages and same-video time links.
pub fn resource(run: &Run, video_id: &str) -> Option<Run> {
    let link = run.link.as_ref()?;
    let label = js_trim(&run.text);
    let mut l = label.chars();
    if l.next() == Some('#') && l.next().is_some_and(|c| !js_space(c)) || is_timestamp(label) {
        return None;
    }
    let href = match link {
        Link::Ext(u) => {
            let lower = u.get(..8).unwrap_or(u).to_ascii_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")) {
                return None;
            }
            u.clone()
        }
        Link::Int(h) => {
            let path_end = h.find(['?', '#']).unwrap_or(h.len());
            let path = &h[..path_end];
            if path.starts_with("/hashtag/") {
                return None;
            }
            let query = h[path_end..].strip_prefix('?').map(|q| q.split('#').next().unwrap_or("")).unwrap_or("");
            let id = param(query, "v").unwrap_or("");
            let same = path == "/watch" && (id.is_empty() || id == video_id);
            if same && ["t", "start", "time_continue"].iter().any(|k| param(query, k).is_some()) {
                return None;
            }
            h.clone()
        }
        // A same-video link with a start time is a timestamp; YouTube writes a
        // plain same-video link (no time) as /watch?v=<id>, which stays.
        Link::Seek(t) if *t > 0 => return None,
        Link::Seek(_) => format!("/watch?v={video_id}"),
    };
    Some(Run { text: if label.is_empty() { href } else { label.to_string() }, link: Some(link.clone()) })
}

fn flat(tokens: &[Run]) -> String {
    case_normal(&tokens.iter().map(|r| r.text.as_str()).collect::<String>())
}

/// The old `walk()` + `resource_rows()` over the client's description runs:
/// lines split at newlines; only lines with a resource are kept; a bare link
/// line borrows the previous line as its label when that line is short
/// (≤ 100 UTF-16 units), link-free, unused and not a timestamp.
pub fn resource_rows(desc: &[Run], video_id: &str) -> Vec<Vec<Run>> {
    let mut lines: Vec<Vec<Run>> = vec![vec![]];
    for r in desc {
        if r.link.is_some() {
            if let Some(tok) = resource(r, video_id) {
                lines.last_mut().unwrap().push(tok);
            }
            continue;
        }
        let value = strip_hashtags(&r.text);
        for (i, part) in value.split('\n').enumerate() {
            if i > 0 {
                lines.push(vec![]);
            }
            let part = part.strip_suffix('\r').unwrap_or(part);
            if !part.is_empty() {
                lines.last_mut().unwrap().push(Run { text: part.to_string(), link: None });
            }
        }
    }
    let mut rows = Vec::new();
    let mut used = std::collections::HashSet::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.iter().any(|r| r.link.is_some()) {
            continue;
        }
        let bare = !line.iter().any(|r| r.link.is_none() && !js_trim(&r.text).is_empty());
        let labelled = index > 0 && bare && {
            let previous = &lines[index - 1];
            let context = flat(previous);
            !context.is_empty()
                && context.encode_utf16().count() <= 100
                && !used.contains(&(index - 1))
                && !previous.iter().any(|r| r.link.is_some())
                && !starts_with_timestamp(&context)
        };
        if labelled {
            let mut row = lines[index - 1].clone();
            row.push(Run { text: " ".into(), link: None });
            row.extend(line.iter().cloned());
            rows.push(row);
            used.insert(index - 1);
        } else {
            rows.push(line.clone());
        }
        used.insert(index);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "abcdefghijk";
    fn t(s: &str) -> Run {
        Run { text: s.into(), link: None }
    }
    fn ext(s: &str, u: &str) -> Run {
        Run { text: s.into(), link: Some(Link::Ext(u.into())) }
    }
    fn int(s: &str, h: &str) -> Run {
        Run { text: s.into(), link: Some(Link::Int(h.into())) }
    }
    fn seek(s: &str, t: u32) -> Run {
        Run { text: s.into(), link: Some(Link::Seek(t)) }
    }
    /// Rows as the panel shows them (text collapsed like `white-space:normal`) + their links.
    fn show(rows: &[Vec<Run>]) -> Vec<(String, Vec<Link>)> {
        rows.iter().map(|r| (case_normal(&r.iter().map(|x| x.text.as_str()).collect::<String>()), r.iter().filter_map(|x| x.link.clone()).collect())).collect()
    }

    /// tests/description.test.cjs "keeps resource context and destinations, strips prose, hashtags and timestamps".
    #[test]
    fn keeps_resource_context_strips_prose_hashtags_and_timestamps() {
        let desc = vec![
            t("Unrelated introduction.\n\nJoin my Discord: "),
            ext("Discord", "https://example.test/discord#join"),
            t(" "),
            int("#gaming", "/hashtag/gaming"),
            t("\n\nProject files:\n"),
            ext("Download", "https://example.test/files"),
            t("\n\n"),
            int("#gaming", "/hashtag/gaming"),
            t(" "),
            seek("0:30", 30),
            t("\n\n"),
            int("Related tutorial", "/watch?v=another0001"),
            t("\n\nUnsafe\nThanks for watching. #thanks"),
        ];
        let rows = show(&resource_rows(&desc, V));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("Join my Discord: Discord".into(), vec![Link::Ext("https://example.test/discord#join".into())]));
        assert_eq!(rows[1], ("Project files: Download".into(), vec![Link::Ext("https://example.test/files".into())]));
        assert_eq!(rows[2], ("Related tutorial".into(), vec![Link::Int("/watch?v=another0001".into())]));
    }

    /// A plain-text `#tag` (not a link) in a resource line is stripped too.
    #[test]
    fn text_hashtags_are_stripped() {
        let rows = show(&resource_rows(&[t("Join #gaming my server: "), ext("Discord", "https://d.test"), t(" #cool(#x) a#b #")], V));
        assert_eq!(rows[0].0, "Join my server: Discord () a#b #");
        assert_eq!(strip_hashtags("#a #b x#c (#d) #é_1 #"), "  x#c ()  #");
        assert_eq!(strip_hashtags("\n#tag\n"), "\n\n");
    }

    /// "preserves same-line context across inline spans, avoids preceding paragraphs across blank lines, hides empty panels".
    #[test]
    fn same_line_context_and_no_label_across_blank_lines() {
        let desc = vec![t("Unrelated previous paragraph.\n\n"), ext("Resource", "https://example.test"), t(" — notes after it")];
        assert_eq!(show(&resource_rows(&desc, V)), vec![("Resource — notes after it".into(), vec![Link::Ext("https://example.test".into())])]);
        // Bare link right below a paragraph separated by a blank line: no label.
        let rows = show(&resource_rows(&[t("Unrelated paragraph.\n\n"), ext("Link", "https://e.test")], V));
        assert_eq!(rows[0].0, "Link");
        assert!(resource_rows(&[t("Only plain text #hashtag")], V).is_empty());
        assert!(resource_rows(&[], V).is_empty());
    }

    /// "Downloads: <a>Complete link</a>" and "Two: <a>Two</a>" keep their own line text.
    #[test]
    fn line_text_with_link() {
        assert_eq!(show(&resource_rows(&[t("Downloads: "), ext("Complete link", "https://example.test/full")], V))[0].0, "Downloads: Complete link");
        assert_eq!(show(&resource_rows(&[t("Project files: "), ext("Download", "https://example.test/direct#download")], V))[0].1, vec![Link::Ext("https://example.test/direct#download".into())]);
    }

    #[test]
    fn bare_link_labels() {
        // The previous line labels a bare link.
        let r = show(&resource_rows(&[t("GitHub repo\n"), ext("https://github.com/x", "https://github.com/x")], V));
        assert_eq!(r[0].0, "GitHub repo https://github.com/x");
        // Whitespace around the bare link still counts as bare.
        let r = show(&resource_rows(&[t("Label\n  "), ext("L", "https://l.test"), t("  ")], V));
        assert_eq!(r[0].0, "Label L");
        // A label is used once: the second bare link has a link line above it.
        let r = show(&resource_rows(&[t("Links\n"), ext("A", "https://a.test"), t("\n"), ext("B", "https://b.test")], V));
        assert_eq!(r.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Links A", "B"]);
        // A line with a link never labels the next one.
        let r = show(&resource_rows(&[t("x "), ext("A", "https://a.test"), t("\n"), ext("B", "https://b.test")], V));
        assert_eq!(r[1].0, "B");
        // Too long (> 100 UTF-16 units; emoji count 2).
        let long = "😀".repeat(51);
        let r = show(&resource_rows(&[t(&format!("{long}\n")), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, "A");
        let ok = "😀".repeat(50);
        let r = show(&resource_rows(&[t(&format!("{ok}\n")), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, format!("{ok} A"));
        // A timestamp line never labels.
        let r = show(&resource_rows(&[t("1:02 intro\n"), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, "A");
        // A hashtag-only previous line is empty after stripping: no label.
        let r = show(&resource_rows(&[t("#tag #other\n"), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, "A");
        // The previous line's label keeps its collapsed spacing; \r\n is a newline.
        let r = show(&resource_rows(&[t("My   site:\r\n"), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, "My site: A");
        // First line: nothing to borrow.
        assert_eq!(show(&resource_rows(&[ext("A", "https://a.test")], V))[0].0, "A");
        // A not-bare line keeps only its own text.
        let r = show(&resource_rows(&[t("Label\nsee "), ext("A", "https://a.test")], V));
        assert_eq!(r[0].0, "see A");
        // The real chapters fixture shape: "my twitter:   / karpathy  ".
        let r = show(&resource_rows(&[t("my twitter:   "), ext("/ karpathy", "https://twitter.com/karpathy"), t("  ")], V));
        assert_eq!(r[0].0, "my twitter: / karpathy");
    }

    #[test]
    fn resource_rules() {
        let res = |r: Run| resource(&r, V).map(|x| x.text);
        // Hashtag and timestamp labels.
        assert_eq!(res(ext("#tag", "https://e.test")), None);
        assert_eq!(res(ext("# tag", "https://e.test")).as_deref(), Some("# tag"), "^#\\S only");
        for ts in ["0:30", "12:34", "1:02:03", "100:00:00", " 3:45 "] {
            assert_eq!(res(ext(ts, "https://e.test")), None, "{ts}");
        }
        for not_ts in ["0:3", "123:45", "1:2:3", "a1:23", "1:23x", "1:234"] {
            assert!(res(ext(not_ts, "https://e.test")).is_some(), "{not_ts}");
        }
        // Schemes.
        assert!(res(ext("m", "mailto:a@b.test")).is_some());
        assert!(res(ext("f", "ftp://e.test")).is_none());
        // Internal: hashtag pages, same-video time links.
        assert_eq!(res(int("tag", "/hashtag/tag")), None);
        assert_eq!(res(int("x", &format!("/watch?v={V}&t=30s"))), None);
        assert_eq!(res(int("x", "/watch?t=30")), None);
        assert_eq!(res(int("x", &format!("/watch?v={V}&start=5"))), None);
        assert_eq!(res(int("x", &format!("/watch?v={V}&time_continue=5"))), None);
        assert!(res(int("x", &format!("/watch?v={V}"))).is_some(), "same video, no time");
        assert!(res(int("x", "/watch?v=other000001&t=30s")).is_some(), "another video's time link is a resource");
        assert!(res(int("Channel", "/@chan")).is_some());
        assert!(res(int("pl", "/playlist?list=PL1")).is_some());
        // Seek: a timed same-video link goes; a plain same-video link stays.
        assert_eq!(res(seek("intro", 12)), None);
        assert!(res(seek("this video", 0)).is_some());
        // Empty label shows the destination; labels are trimmed.
        assert_eq!(res(ext("  ", "https://e.test/a")).as_deref(), Some("https://e.test/a"));
        assert_eq!(res(ext("  site  ", "https://e.test/a")).as_deref(), Some("site"));
    }

    #[test]
    fn timestamp_matcher() {
        assert!(starts_with_timestamp("00:00:00 intro"));
        assert!(starts_with_timestamp("1:02 intro"));
        assert!(!starts_with_timestamp("123:45"), "no 1–2 digit minutes at the start");
        assert!(!starts_with_timestamp("intro 1:02"));
        assert!(!is_timestamp("123:45"));
        assert!(is_timestamp("123:45:67"));
    }

    /// The real description in tests/fixtures/chapters_next.json: only its link lines survive, with real destinations.
    #[test]
    fn real_fixture_keeps_only_link_lines() {
        let v = crate::util::test_fixture("chapters_next.json");
        let w = crate::parse::next(&v);
        let rows = show(&resource_rows(&w.desc, &w.id));
        let texts: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert!(texts.contains(&"Google colab for the video: https://colab.research.google.com/dri..."), "{texts:#?}");
        assert!(texts.contains(&"nanoGPT repo: https://github.com/karpathy/nanoGPT"));
        assert!(texts.contains(&"my twitter: / karpathy"));
        assert!(!texts.iter().any(|t| t.contains("We build a Generatively") || t.contains("Suggested exercises") || t.contains("00:00")), "{texts:#?}");
        assert!(rows.iter().all(|r| !r.1.iter().any(|l| matches!(l, Link::Seek(t) if *t > 0))), "no timestamp links");
        assert!(rows.iter().flat_map(|r| &r.1).all(|l| !matches!(l, Link::Ext(u) if u.contains("youtube.com/redirect"))), "real destinations");
        assert!(rows.iter().flat_map(|r| &r.1).any(|l| l == &Link::Ext("https://github.com/karpathy/nanoGPT".into())));
    }
}
