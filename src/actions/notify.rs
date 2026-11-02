//! Top-bar account surfaces: the notification bell (unread count from the
//! `topbar` that page responses already carry, the list from
//! `notification/get_notification_menu`) and a minimal account menu that
//! hands account pages to YouTube's own site.

use super::{arr, at, find, find_all, st, text_at};
use crate::parse::cmd_href;
use crate::util::{esc_into, is_safe_id, safe_img};
use crate::json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Topbar {
    pub count: Option<u32>,
    pub avatar: Option<String>,
    /// The topbar shows a Sign in button.
    pub signed_out: bool,
}

/// Bell count and avatar from a page response's `topbar` (no extra request).
pub fn topbar(v: &Value) -> Option<Topbar> {
    let btns = arr(v, &["topbar", "desktopTopbarRenderer", "topbarButtons"]);
    if btns.is_empty() {
        return None;
    }
    let mut t = Topbar::default();
    for b in btns {
        if let Some(n) = b.get("notificationTopbarButtonRenderer") {
            t.count = n.get("notificationCount").and_then(Value::as_u64).map(|c| c.min(999) as u32);
        } else if let Some(m) = b.get("topbarMenuButtonRenderer") {
            t.avatar = t.avatar.take().or_else(|| arr(m, &["avatar", "thumbnails"]).iter().find_map(|x| safe_img(x.get("url")?.as_str()?)));
        } else if st(b, &["buttonRenderer", "icon", "iconType"]) == Some("AVATAR_LOGGED_OUT") {
            t.signed_out = true;
        }
    }
    Some(t)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notif {
    pub text: String,
    pub time: String,
    pub href: Option<String>,
    pub avatar: Option<String>,
    pub thumb: Option<String>,
    pub unread: bool,
    /// Marks it read (`notification/record_interactions`).
    pub record: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub items: Vec<Notif>,
    pub ctoken: Option<String>,
    /// Empty-state text ("Your notifications live here").
    pub empty: Option<String>,
}

pub fn menu(v: &Value) -> Menu {
    let mut m = Menu::default();
    for r in find_all(v, "notificationRenderer") {
        let Some(text) = text_at(r, &["shortMessage"]) else { continue };
        let img = |k: &str| arr(r, &[k, "thumbnails"]).iter().find_map(|x| safe_img(x.get("url")?.as_str()?));
        m.items.push(Notif {
            text,
            time: text_at(r, &["sentTimeText"]).unwrap_or_default(),
            href: r.get("navigationEndpoint").and_then(cmd_href),
            avatar: img("thumbnail"),
            thumb: img("videoThumbnail"),
            unread: r.get("read").and_then(Value::as_bool) == Some(false),
            record: at(r, &["recordClickEndpoint", "recordNotificationInteractionsEndpoint", "serializedInteractionsRequest"]).and_then(Value::as_str).map(String::from),
        });
    }
    m.ctoken = find(v, "getNotificationMenuEndpoint").and_then(|e| st(e, &["ctoken"])).filter(|t| t.len() < 4096).map(String::from);
    if m.items.is_empty() {
        m.empty = find(v, "backgroundPromoRenderer").and_then(|b| text_at(b, &["title"])).or_else(|| find(v, "messageRenderer").and_then(|b| text_at(b, &["text"])));
    }
    m
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub name: String,
    pub handle: String,
    pub photo: Option<String>,
    pub channel: Option<String>,
}

pub fn account(v: &Value) -> Option<Account> {
    let h = find(v, "activeAccountHeaderRenderer")?;
    let channel = find_all(v, "compactLinkRenderer").into_iter().find_map(|c| {
        let b = at(c, &["navigationEndpoint", "browseEndpoint", "browseId"]).and_then(Value::as_str)?;
        (b.starts_with("UC") && is_safe_id(b)).then(|| c.get("navigationEndpoint").and_then(cmd_href)).flatten()
    });
    Some(Account {
        name: text_at(h, &["accountName"]).unwrap_or_default(),
        handle: text_at(h, &["channelHandle"]).unwrap_or_default(),
        photo: arr(h, &["accountPhoto", "thumbnails"]).iter().find_map(|x| safe_img(x.get("url")?.as_str()?)),
        channel,
    })
}

// ---------------------------------------------------------------- markup

const I_BELL: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M6 16v-5a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0\"/></svg>";
const I_USER: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><circle cx=\"12\" cy=\"9\" r=\"3.5\"/><path d=\"M5.5 19a6.5 6.5 0 0 1 13 0\"/></svg>";

/// The bell's accessible name (the badge itself is hidden from assistive tech).
pub fn bell_label(count: u32) -> String {
    if count > 0 { format!("Notifications, {count} new") } else { "Notifications".into() }
}

pub fn count_label(n: u32) -> String {
    if n > 9 { "9+".into() } else { n.to_string() }
}

/// Popover ids (the panels always exist; opening and closing flip `pop_class`).
pub const NT_P: &str = "ytc-nt-p";
pub const AC_P: &str = "ytc-ac-p";
pub const NT_B: &str = "ytc-nt-b";
/// The bell's badge: always present (`z` hides it), so a count change morphs in place.
pub const NT_N: &str = "ytc-nt-n";
pub const AC_B: &str = "ytc-ac-b";
pub const NT_CLASS: &str = "ya-pop yac-p";
pub const AC_CLASS: &str = "ya-pop yac-p yac-m";

/// Class list of a persistent popover: `open` shows it (client.css grows it
/// from its trigger and shrinks it back), `now` skips the transition
/// (keyboard opens/closes are instant).
pub fn pop_class(base: &str, open: bool, now: bool) -> String {
    format!("{base}{}{}", if open { " open" } else { "" }, if now { " now" } else { "" })
}

/// Bell + avatar buttons with their panels (always present; `nt`/`ac` = the
/// open panel's content).
pub fn slot_html(count: u32, avatar: Option<&str>, nt: Option<&str>, ac: Option<&str>) -> String {
    let mut s = format!(
        "<div class=\"ya-pw\" data-pop=\"nt\"><button type=\"button\" id=\"{NT_B}\" class=\"yac-b\" data-act=\"a-nt\" aria-haspopup=\"dialog\" aria-expanded=\"{}\" aria-label=\"{}\">{I_BELL}",
        nt.is_some(),
        bell_label(count)
    );
    s.push_str(&format!(
        "<b id=\"{NT_N}\" class=\"{}\" aria-hidden=\"true\">{}</b>",
        if count > 0 { "yac-n" } else { "yac-n z" },
        if count > 0 { count_label(count) } else { String::new() }
    ));
    s.push_str(&format!("</button><div id=\"{NT_P}\" class=\"{}\" role=\"dialog\" aria-label=\"Notifications\">", pop_class(NT_CLASS, nt.is_some(), false)));
    s.push_str(nt.unwrap_or(""));
    s.push_str(&format!("</div></div><div class=\"ya-pw\" data-pop=\"ac\"><button type=\"button\" id=\"{AC_B}\" class=\"yac-av\" data-act=\"a-ac\" aria-haspopup=\"menu\" aria-expanded=\"{}\" aria-label=\"Account\">", ac.is_some()));
    match avatar {
        Some(a) => {
            s.push_str("<img src=\"");
            esc_into(&mut s, a);
            s.push_str("\" alt=\"\" decoding=\"async\">");
        }
        None => s.push_str(I_USER),
    }
    s.push_str(&format!("</button><div id=\"{AC_P}\" class=\"{}\" role=\"menu\" aria-label=\"Account\">", pop_class(AC_CLASS, ac.is_some(), false)));
    s.push_str(ac.unwrap_or(""));
    s.push_str("</div></div>");
    s
}

pub fn list_html(items: &[Notif], more: bool, empty: Option<&str>, loading: bool, err: Option<&str>) -> String {
    let mut s = String::from("<div class=\"ya-ph\">Notifications</div><div class=\"yac-l\">");
    for (i, n) in items.iter().enumerate() {
        let href = n.href.as_deref().unwrap_or("/");
        s.push_str(&format!("<a class=\"yac-i{}\" href=\"", if n.unread { " new" } else { "" }));
        esc_into(&mut s, href);
        s.push_str(&format!("\" data-act=\"a-ntc\" data-arg=\"{i}\">"));
        for (src, cls) in [(&n.avatar, "av"), (&n.thumb, "th")] {
            if let Some(u) = src {
                s.push_str(&format!("<img class=\"{cls}\" src=\""));
                esc_into(&mut s, u);
                s.push_str("\" alt=\"\" loading=\"lazy\" decoding=\"async\">");
            }
        }
        s.push_str("<span><span class=\"yac-t\">");
        esc_into(&mut s, &n.text);
        s.push_str("</span><small>");
        esc_into(&mut s, &n.time);
        s.push_str("</small></span></a>");
    }
    if loading {
        s.push_str("<div class=\"ya-load\"></div>");
    } else if let Some(e) = err {
        s.push_str("<p class=\"ya-err\">");
        esc_into(&mut s, e);
        s.push_str(" <button type=\"button\" class=\"lnk2\" data-act=\"a-ntmore\">Try again</button></p>");
    } else if more {
        s.push_str("<button type=\"button\" class=\"lnk2 yac-more\" data-act=\"a-ntmore\">More</button>");
    } else if items.is_empty() {
        s.push_str("<p class=\"ya-err\">");
        esc_into(&mut s, empty.unwrap_or("No notifications."));
        s.push_str("</p>");
    }
    s.push_str("</div>");
    s
}

/// Account menu: links open YouTube's / Google's own pages (full navigation).
pub fn account_html(a: Option<&Account>) -> String {
    let mut s = String::new();
    if let Some(a) = a {
        s.push_str("<div class=\"yac-h\">");
        if let Some(p) = &a.photo {
            s.push_str("<img class=\"av\" src=\"");
            esc_into(&mut s, p);
            s.push_str("\" alt=\"\">");
        }
        s.push_str("<span><b>");
        esc_into(&mut s, &a.name);
        s.push_str("</b><small>");
        esc_into(&mut s, &a.handle);
        s.push_str("</small></span></div>");
    }
    // Trimmed to the four account entries (2 Nov 2026): no "Your channel", no "This page on YouTube".
    for (label, href) in [
        ("Google Account", "https://myaccount.google.com/"),
        ("Switch account", "/channel_switcher?yt-native=1"),
        ("Settings", "/account"),
        ("Sign out", "/logout"),
    ] {
        s.push_str("<a class=\"yac-a\" role=\"menuitem\" href=\"");
        esc_into(&mut s, href);
        s.push_str("\" data-act=\"a-go\" data-arg=\"");
        esc_into(&mut s, href);
        s.push_str("\">");
        s.push_str(label);
        s.push_str("</a>");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::tests::fixture;
    #[test]
    fn topbars() {
        let t = topbar(&fixture("next.json")).unwrap();
        assert!(t.signed_out && t.count.is_none() && t.avatar.is_none());
        let t = topbar(&crate::json::jv!({"topbar": {"desktopTopbarRenderer": {"topbarButtons": [
            {"notificationTopbarButtonRenderer": {"notificationCount": 4}},
            {"topbarMenuButtonRenderer": {"avatar": {"thumbnails": [{"url": "https://yt3.ggpht.com/a=s88"}]}}}]}}})).unwrap();
        assert_eq!((t.count, t.avatar.as_deref(), t.signed_out), (Some(4), Some("https://yt3.ggpht.com/a=s88"), false));
        assert!(topbar(&crate::json::jv!({})).is_none());
    }
    #[test]
    fn notification_menu() {
        let j = crate::json::jv!({"actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"sections": [{"multiPageMenuNotificationSectionRenderer": {"items": [
            {"notificationRenderer": {"thumbnail": {"thumbnails": [{"url": "https://yt3.ggpht.com/c"}]}, "videoThumbnail": {"thumbnails": [{"url": "https://i.ytimg.com/vi/abcdefghijk/mq.jpg"}]},
                "shortMessage": {"simpleText": "Chan uploaded: <b>New</b>"}, "sentTimeText": {"simpleText": "2 hours ago"}, "read": false,
                "navigationEndpoint": {"commandMetadata": {"webCommandMetadata": {"url": "/watch?v=abcdefghijk"}}, "watchEndpoint": {"videoId": "abcdefghijk"}},
                "recordClickEndpoint": {"recordNotificationInteractionsEndpoint": {"serializedInteractionsRequest": "SER"}}}},
            {"notificationRenderer": {"shortMessage": {"simpleText": "Old"}, "read": true, "navigationEndpoint": {"urlEndpoint": {"url": "javascript:x"}}}},
            {"continuationItemRenderer": {"continuationEndpoint": {"getNotificationMenuEndpoint": {"ctoken": "CT"}}}}]}}]}}}}]});
        let m = menu(&j);
        assert_eq!(m.items.len(), 2);
        assert_eq!((m.items[0].href.as_deref(), m.items[0].unread, m.items[0].record.as_deref()), (Some("/watch?v=abcdefghijk"), true, Some("SER")));
        assert_eq!((m.items[1].href.as_deref(), m.items[1].unread), (None, false));
        assert_eq!(m.ctoken.as_deref(), Some("CT"));
        let h = list_html(&m.items, true, None, false, None);
        assert!(h.contains("&lt;b&gt;New") && h.contains("data-act=\"a-ntc\" data-arg=\"1\"") && h.contains("a-ntmore"));
        let e = menu(&crate::json::jv!({"actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"sections": [{"backgroundPromoRenderer": {"title": {"runs": [{"text": "Your notifications live here"}]}}}]}}}}]}));
        assert_eq!(e.empty.as_deref(), Some("Your notifications live here"));
    }
    #[test]
    fn account_menu() {
        let j = crate::json::jv!({"actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"header": {"activeAccountHeaderRenderer": {
            "accountName": {"simpleText": "Viewer"}, "channelHandle": {"simpleText": "@viewer"}, "accountPhoto": {"thumbnails": [{"url": "https://yt3.ggpht.com/p"}]}}},
            "sections": [{"multiPageMenuSectionRenderer": {"items": [{"compactLinkRenderer": {"title": {"simpleText": "Your channel"}, "navigationEndpoint": {"browseEndpoint": {"browseId": "UCown", "canonicalBaseUrl": "/@viewer"}}}}]}}]}}}}]});
        let a = account(&j).unwrap();
        assert_eq!(a, Account { name: "Viewer".into(), handle: "@viewer".into(), photo: Some("https://yt3.ggpht.com/p".into()), channel: Some("/@viewer".into()) });
        let h = account_html(Some(&a));
        assert!(h.contains("href=\"/logout\" data-act=\"a-go\""));
        assert!(!h.contains("Your channel") && !h.contains("This page on YouTube"), "trimmed menu");
        let labels: Vec<&str> = h.match_indices("role=\"menuitem\"").map(|(i, _)| { let t = &h[i..]; let a = t.find('>').unwrap() + 1; &t[a..a + t[a..].find('<').unwrap()] }).collect();
        assert_eq!(labels, ["Google Account", "Switch account", "Settings", "Sign out"]);
        assert!(account(&crate::json::jv!({})).is_none());
        assert!(slot_html(12, None, None, None).contains("<b id=\"ytc-nt-n\" class=\"yac-n\" aria-hidden=\"true\">9+</b>"));
        // No unread: the badge element stays (hidden), so the next count grows it in place.
        assert!(slot_html(0, None, None, None).contains("<b id=\"ytc-nt-n\" class=\"yac-n z\" aria-hidden=\"true\"></b>"));
        // Both panels always exist (closed: empty, no `open`), so opening is a class flip.
        let closed = slot_html(0, None, None, None);
        assert!(closed.contains("id=\"ytc-nt-p\" class=\"ya-pop yac-p\"") && closed.contains("id=\"ytc-ac-p\" class=\"ya-pop yac-p yac-m\""));
        let open = slot_html(0, None, Some("<i>list</i>"), None);
        assert!(open.contains("class=\"ya-pop yac-p open\" role=\"dialog\" aria-label=\"Notifications\"><i>list</i>") && open.contains("aria-expanded=\"true\""));
        assert_eq!(pop_class("ya-pop", true, true), "ya-pop open now");
    }
}
