//! The document-start script for https://www.youtube.com: the page shim (`tv/shim/shim.js`) around
//! the client bundle, as one function so nothing leaks into the page's globals but what the shim
//! defines on purpose.

use lumen_tv_core::release::{Bundle, DEV_FILE};

const SHIM: &str = include_str!("../../../shim/shim.js");

/// The build a report names (the debug center groups by it): the client, the shell, the bundle's source and, while
/// telemetry is on, `telemetry`.
pub fn build_name(b: &Bundle, shell_version: &str, tel: bool) -> String {
    let dev = if tel && !b.file(DEV_FILE).is_empty() { ", telemetry" } else { "" };
    format!("{} (TV {shell_version}, {}{dev})", b.client, b.source)
}

/// `tel`: the owner turned telemetry on: the bundle's telemetry variant runs (when it has one) and the shim
/// hands the client's reports to the shell (`CFG.tel`).
pub fn compose(b: &Bundle, shell_version: &str, tel: bool) -> String {
    let cfg = serde_json::json!({"version": shell_version, "version_name": build_name(b, shell_version, tel), "tel": tel});
    let css = serde_json::json!({"client": b.file("client.css"), "native": b.file("native.css")});
    let js = match b.file(DEV_FILE) {
        dev if tel && !dev.is_empty() => dev,
        _ => b.file("content.js"),
    };
    let mut s = String::with_capacity(js.len() + 2 * 1024 * 1024);
    s.push_str("(() => {\n'use strict';\nconst CFG = ");
    s.push_str(&cfg.to_string());
    s.push_str(";\nconst CSS = ");
    s.push_str(&css.to_string());
    s.push_str(";\nconst ytcClient = () => {\n");
    s.push_str(js);
    s.push_str("\n};\n");
    s.push_str(SHIM);
    s.push_str("\n})();\n");
    s
}

/// A desktop Chrome user agent with the WebView's own Chrome version: www.youtube.com serves its
/// desktop site to it (an Android UA is sent to m.youtube.com).
pub fn user_agent(webview_default: &str) -> String {
    let major = webview_default
        .split("Chrome/")
        .nth(1)
        .and_then(|v| v.split('.').next())
        .filter(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("140");
    format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36")
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_agent_keeps_the_chrome_version() {
        let wv = "Mozilla/5.0 (Linux; Android 14; sdk_google_atv64_arm64 Build/UTT3.240625.001.K5; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/138.0.7204.168 Mobile Safari/537.36";
        let ua = super::user_agent(wv);
        assert_eq!(
            ua,
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36"
        );
        assert!(!ua.contains("Android") && !ua.contains("wv") && !ua.contains("Mobile"));
        assert!(super::user_agent("junk").contains("Chrome/140.0.0.0"));
    }

    #[test]
    fn script_wraps_the_bundle() {
        let mut files = std::collections::BTreeMap::new();
        files.insert("content.js".to_string(), "window.__ran = 1;".to_string());
        files.insert("client.css".to_string(), "a{color:\"red\"}</script>".to_string());
        files.insert("native.css".to_string(), "b{}".to_string());
        let b = lumen_tv_core::release::Bundle {
            seq: 1,
            client: "0.8.6 (x)".into(),
            source: "builtin",
            files,
        };
        let s = super::compose(&b, "0.1.0", false);
        assert!(s.starts_with("(() => {") && s.trim_end().ends_with("})();"));
        assert!(s.contains("const ytcClient = () => {\nwindow.__ran = 1;\n};"));
        assert!(s.contains(r#""client":"a{color:\"red\"}</script>""#));
        assert!(s.contains("YTC_PLATFORM") && s.contains(r#""tel":false"#));
        // Telemetry on without a variant: the public client, the shim forwarding (incidents only).
        let s = super::compose(&b, "0.1.0", true);
        assert!(s.contains("window.__ran = 1;") && s.contains(r#""tel":true"#) && !s.contains("telemetry)"));
    }

    #[test]
    fn telemetry_runs_the_variant() {
        let mut files = std::collections::BTreeMap::new();
        files.insert("content.js".to_string(), "public();".to_string());
        files.insert("content-dev.js".to_string(), "dev();".to_string());
        let b = lumen_tv_core::release::Bundle { seq: 1, client: "0.10.0 (x)".into(), source: "release", files };
        let off = super::compose(&b, "0.2.0", false);
        assert!(off.contains("public();") && !off.contains("dev();") && off.contains(r#""version_name":"0.10.0 (x) (TV 0.2.0, release)""#));
        let on = super::compose(&b, "0.2.0", true);
        assert!(on.contains("dev();") && !on.contains("public();") && on.contains(r#""version_name":"0.10.0 (x) (TV 0.2.0, release, telemetry)""#));
    }
}
