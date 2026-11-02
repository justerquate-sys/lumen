//! Native fallback pages (`?yt-native=1`, e.g. when our player cannot play a
//! video). YouTube's app runs there and navigates in-page, which would keep the
//! user on vanilla YouTube for the rest of the tab. Leaving the fallback video
//! instead reloads into the client. `clean` keeps these pages close to the
//! client's look (with `extension/native.css`).

pub mod clean;

use crate::bridge;
use crate::route::{self, Route};
use std::cell::RefCell;

thread_local! {
    static VIDEO: RefCell<Option<String>> = const { RefCell::new(None) };
    static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// This document is a native page (storage and timer callbacks belong here, not to the app).
pub fn active() -> bool {
    ACTIVE.with(|a| a.get())
}

/// The DOM changed (at most once per frame): the cleanup for the video in the address bar.
pub fn on_dom() {
    clean::on_dom(video_of(&bridge::href()));
}

pub(crate) fn video_of(href: &str) -> Option<String> {
    match route::parse(href) {
        Some(Route::Watch { v, .. }) => Some(v),
        _ => None,
    }
}

/// `href` without the `yt-native` parameter.
pub fn client_url(href: &str) -> String {
    let (path, query) = href.split_once('?').unwrap_or((href, ""));
    let kept: Vec<&str> = query.split('&').filter(|p| !p.is_empty() && !p.starts_with("yt-native=")).collect();
    if kept.is_empty() { path.to_string() } else { format!("{path}?{}", kept.join("&")) }
}

/// Whether the client should take over `href` from the native page.
pub fn leaves(current: Option<&str>, href: &str) -> bool {
    if !href.starts_with('/') || href.starts_with("//") || href.contains("yt-native=1") {
        return false;
    }
    match route::parse(href) {
        Some(Route::Watch { v, .. }) => current != Some(v.as_str()),
        // Pages the client does not render stay native.
        Some(Route::Unknown { .. }) | None => false,
        Some(_) => true,
    }
}

pub fn boot(href: &str) {
    ACTIVE.with(|a| a.set(true));
    VIDEO.with(|v| *v.borrow_mut() = video_of(href));
    bridge::native_listen();
    clean::boot();
}

pub fn on_link(href: &str) -> bool {
    let current = VIDEO.with(|v| v.borrow().clone());
    if leaves(current.as_deref(), href) {
        bridge::go(&client_url(href));
        return true;
    }
    false
}

pub fn on_nav(href: &str) {
    let current = VIDEO.with(|v| v.borrow().clone());
    if leaves(current.as_deref(), href) {
        bridge::replace_doc(&client_url(href));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leaving_the_fallback_video_returns_to_the_client() {
        assert!(!leaves(Some("aqz-KE-bpKQ"), "/watch?v=aqz-KE-bpKQ&t=30s"));
        assert!(leaves(Some("aqz-KE-bpKQ"), "/watch?v=jNQXAC9IVRw"));
        assert!(leaves(Some("aqz-KE-bpKQ"), "/"));
        assert!(leaves(Some("aqz-KE-bpKQ"), "/results?search_query=rust"));
        assert!(!leaves(Some("aqz-KE-bpKQ"), "/account"));
        assert!(!leaves(Some("aqz-KE-bpKQ"), "https://example.com/"));
        assert!(!leaves(Some("aqz-KE-bpKQ"), "/watch?v=x1&yt-native=1"));
        assert_eq!(client_url("/watch?v=a&yt-native=1&t=3"), "/watch?v=a&t=3");
        assert_eq!(client_url("/?yt-native=1"), "/");
    }
}
