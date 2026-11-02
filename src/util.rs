//! Small string helpers: HTML escaping, percent coding, query parsing and
//! validation of ids/URLs that end up in markup. No regex, no Unicode tables.

/// Process-wide id for fetches, timers and media handles (never 0).
pub fn next_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Appends `s` escaped for both HTML text and double/single-quoted attributes.
pub fn esc_into(out: &mut String, s: &str) {
    let mut last = 0;
    for (i, b) in s.bytes().enumerate() {
        let rep = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + 1;
    }
    out.push_str(&s[last..]);
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    esc_into(&mut out, s);
    out
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decodes `%XX` sequences and `+` (form encoding). Invalid UTF-8 is replaced.
pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn pct_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len() * 3 / 2);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}

/// First value of `name` in a query string (without leading `?`), decoded.
pub fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| pct_decode(v))
    })
}

pub fn is_id_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

pub fn is_video_id(s: &str) -> bool {
    s.len() == 11 && s.bytes().all(is_id_char)
}

/// Playlist ids, browse ids, channel ids: conservative `[A-Za-z0-9_-]{2,160}`.
pub fn is_safe_id(s: &str) -> bool {
    (2..=160).contains(&s.len()) && s.bytes().all(is_id_char)
}

fn clean(s: &str) -> bool {
    !s.bytes().any(|b| b <= b' ' || b == b'\\' || b == 0x7f)
}

/// A same-site path like `/@handle` (never `//host`, never a scheme).
pub fn safe_rel_href(s: &str) -> Option<String> {
    (s.starts_with('/') && !s.starts_with("//") && s.len() < 2048 && clean(s)).then(|| s.to_string())
}

/// An absolute http(s) URL for external links.
pub fn safe_ext_href(s: &str) -> Option<String> {
    let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    ((lower.starts_with("https://") || lower.starts_with("http://")) && s.len() < 4096 && clean(s))
        .then(|| s.to_string())
}

/// Image URLs we are willing to put in markup (YouTube image CDNs only).
pub fn safe_img(s: &str) -> Option<String> {
    let s = if s.starts_with("//") { format!("https:{s}") } else { s.to_string() };
    let ok = ["https://i.ytimg.com/", "https://i1.ytimg.com/", "https://i2.ytimg.com/", "https://i3.ytimg.com/", "https://i4.ytimg.com/", "https://i9.ytimg.com/", "https://yt3.ggpht.com/", "https://yt3.googleusercontent.com/", "https://yt4.ggpht.com/"]
        .iter()
        .any(|p| s.starts_with(p));
    (ok && clean(&s)).then_some(s)
}

/// Parses YouTube `t=` values: `90`, `90s`, `1m30s`, `1h2m3s`.
pub fn parse_t(s: &str) -> u32 {
    let mut total: u32 = 0;
    let mut num: u32 = 0;
    let mut any = false;
    for b in s.bytes() {
        match b {
            b'0'..=b'9' => {
                num = num.saturating_mul(10).saturating_add((b - b'0') as u32);
                any = true;
            }
            b'h' => { total = total.saturating_add(num.saturating_mul(3600)); num = 0; }
            b'm' => { total = total.saturating_add(num.saturating_mul(60)); num = 0; }
            b's' => { total = total.saturating_add(num); num = 0; }
            _ => return 0,
        }
    }
    if any { total.saturating_add(num) } else { 0 }
}

/// Byte offset in `s` of UTF-16 code unit offset `u16_off` (clamped).
pub fn utf16_to_byte(s: &str, u16_off: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= u16_off {
            return i;
        }
        units += c.len_utf16();
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaping() {
        assert_eq!(esc(r#"<img src=x onerror="a('b')">&amp;"#), "&lt;img src=x onerror=&quot;a(&#39;b&#39;)&quot;&gt;&amp;amp;");
        assert_eq!(esc("plain ünïcode"), "plain ünïcode");
    }
    #[test]
    fn percent() {
        assert_eq!(pct_decode("a+b%20c%2Fd%E2%9C%93%zz%4"), "a b c/d✓%zz%4");
        assert_eq!(pct_encode("roblox studio/ü&x"), "roblox%20studio%2F%C3%BC%26x");
        assert_eq!(query_param("v=abc&t=1m2s&x", "t").as_deref(), Some("1m2s"));
    }
    #[test]
    fn t_values() {
        assert_eq!(parse_t("90"), 90);
        assert_eq!(parse_t("1m30s"), 90);
        assert_eq!(parse_t("1h2m3s"), 3723);
        assert_eq!(parse_t("x"), 0);
    }
    #[test]
    fn hrefs() {
        assert!(safe_rel_href("//evil.com").is_none());
        assert!(safe_rel_href("/@x").is_some());
        assert!(safe_ext_href("javascript:alert(1)").is_none());
        assert!(safe_ext_href("HTTPS://ok.example/").is_some());
        assert!(safe_img("https://evil.com/a.jpg").is_none());
        assert_eq!(safe_img("//yt3.ggpht.com/a").as_deref(), Some("https://yt3.ggpht.com/a"));
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
    }
}

/// Test helper: a gzipped JSON fixture from `tests/fixtures`.
#[cfg(test)]
pub fn test_fixture(name: &str) -> crate::json::Value<'static> {
    crate::json::parse(&fixture_bytes(name)).expect("json").into_owned()
}

/// Test helper: the raw bytes of a gzipped fixture from `tests/fixtures`.
#[cfg(test)]
pub fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}.gz", env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("gzip").args(["-dc", &path]).output().expect("gzip");
    assert!(out.status.success(), "fixture {name}");
    out.stdout
}
