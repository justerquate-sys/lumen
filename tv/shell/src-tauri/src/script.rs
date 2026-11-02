//! The document-start script for https://www.youtube.com: the page shim (`tv/shim/shim.js`) around
//! the client bundle, as one function so nothing leaks into the page's globals but what the shim
//! defines on purpose.

use lumen_tv_core::release::Bundle;

const SHIM: &str = include_str!("../../../shim/shim.js");

pub fn compose(b: &Bundle, shell_version: &str) -> String {
    let cfg = serde_json::json!({"version": shell_version, "version_name": format!("{} (TV {shell_version}, {})", b.client, b.source)});
    let css = serde_json::json!({"client": b.file("client.css"), "native": b.file("native.css")});
    let js = b.file("content.js");
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
        let s = super::compose(&b, "0.1.0");
        assert!(s.starts_with("(() => {") && s.trim_end().ends_with("})();"));
        assert!(s.contains("const ytcClient = () => {\nwindow.__ran = 1;\n};"));
        assert!(s.contains(r#""client":"a{color:\"red\"}</script>""#));
        assert!(s.contains("YTC_PLATFORM"));
    }
}
