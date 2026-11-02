//! The TV app's YouTube sign-in: the OAuth 2.0 device flow of YouTube's own TV client, exactly as
//! the YouTube app on a TV does it. The TV shows a code; the owner enters it at
//! youtube.com/activate (Google answers `verification_url` = google.com/device, the same page) on
//! a phone and picks the account. The refresh token never leaves the shell (the caller seals it with
//! the Android Keystore); the page only gets access tokens (1 h) through `auth_token`.
//!
//! Verified live 2 Nov 2026: `POST oauth2.googleapis.com/device/code` with this client answers a
//! code; polling answers `authorization_pending` until it is entered.

use crate::net::{Answer, Http};
use serde_json::{Value, json};

/// YouTube's TV client (the public client of the youtube.com/tv app; the same values every YouTube
/// TV device uses). Kept as data: when Google rotates them, they come from the TV app's script.
pub const CLIENT_ID: &str = "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";
pub const SCOPE: &str = "http://gdata.youtube.com https://www.googleapis.com/auth/youtube-paid-content";
pub const GOOGLE: &str = "https://oauth2.googleapis.com";
/// The device grant type Google's TV clients use.
const DEVICE_GRANT: &str = "http://oauth.net/grant_type/device/1.0";
/// Refresh this long before an access token expires.
const MARGIN_MS: f64 = 5.0 * 60.0 * 1000.0;

/// Where the OAuth calls go (Google; a local mock in tests and debug builds).
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints { base: GOOGLE.into() }
    }
}

impl Endpoints {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.trim_end_matches('/'))
    }
}

struct Device {
    code: String,
    expires_at: f64,
}

/// The signed-in account (or none). `persist` is called with the refresh token whenever it changes
/// (`None` = signed out); the caller stores it sealed.
pub struct Account {
    pub ep: Endpoints,
    device_id: String,
    refresh: Option<String>,
    access: Option<(String, f64)>,
    device: Option<Device>,
    persist: Box<dyn Fn(Option<&str>) + Send>,
}

fn err_of(a: &Answer) -> String {
    a.json()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| format!("HTTP {}", a.status))
}

/// `{"token": ..., "exp": epoch ms}` for the page.
pub fn token_json(t: Option<&(String, f64)>) -> String {
    match t {
        Some((t, exp)) => json!({"token": t, "exp": exp}).to_string(),
        None => json!({"token": null}).to_string(),
    }
}

impl Account {
    pub fn new(ep: Endpoints, device_id: String, refresh: Option<String>, persist: Box<dyn Fn(Option<&str>) + Send>) -> Account {
        Account {
            ep,
            device_id,
            refresh,
            access: None,
            device: None,
            persist,
        }
    }

    pub fn signed_in(&self) -> bool {
        self.refresh.is_some()
    }

    /// A valid access token (refreshed when it runs out within 5 minutes), `Ok(None)` when signed
    /// out (or the grant was revoked: the account is then signed out here too), `Err` on a network
    /// failure (the page keeps what it has).
    pub fn token(&mut self, http: &Http, now: f64) -> Result<Option<(String, f64)>, String> {
        if let Some((t, exp)) = &self.access {
            if *exp - MARGIN_MS > now {
                return Ok(Some((t.clone(), *exp)));
            }
        }
        let Some(refresh) = self.refresh.clone() else { return Ok(None) };
        let a = http.post_json(
            &self.ep.url("/token"),
            &json!({"client_id": CLIENT_ID, "client_secret": CLIENT_SECRET, "refresh_token": refresh, "grant_type": "refresh_token"}),
        )?;
        let v = a.json().unwrap_or(Value::Null);
        if let (200, Some(t)) = (a.status, v.get("access_token").and_then(Value::as_str)) {
            let exp = now + v.get("expires_in").and_then(Value::as_f64).unwrap_or(3600.0) * 1000.0;
            self.access = Some((t.to_string(), exp));
            return Ok(self.access.clone());
        }
        match err_of(&a).as_str() {
            // Revoked at myaccount.google.com, or the password changed: signed out.
            "invalid_grant" | "unauthorized_client" => {
                self.forget();
                Ok(None)
            }
            e => Err(e.to_string()),
        }
    }

    /// Starts a device sign-in: the page's `auth_start` answer (user code, URL, interval, expiry).
    pub fn start(&mut self, http: &Http, now: f64) -> Result<String, String> {
        let a = http.post_json(
            &self.ep.url("/device/code"),
            &json!({"client_id": CLIENT_ID, "scope": SCOPE, "device_id": self.device_id, "device_model": "ytlr::"}),
        )?;
        let v = a.json().unwrap_or(Value::Null);
        let (Some(code), Some(user), Some(url)) = (
            v.get("device_code").and_then(Value::as_str),
            v.get("user_code").and_then(Value::as_str),
            v.get("verification_url").and_then(Value::as_str),
        ) else {
            return Err(err_of(&a));
        };
        let expires = v.get("expires_in").and_then(Value::as_f64).unwrap_or(1800.0);
        let interval = v.get("interval").and_then(Value::as_f64).unwrap_or(5.0);
        self.device = Some(Device {
            code: code.into(),
            expires_at: now + expires * 1000.0,
        });
        Ok(json!({"user_code": user, "verification_url": url, "expires_in": expires, "interval": interval}).to_string())
    }

    /// One poll of a running device sign-in: the page's `auth_poll` answer
    /// (`{"state": "pending"|"done"|"denied"|"expired"|"error", ...}`).
    pub fn poll(&mut self, http: &Http, now: f64) -> String {
        let Some(dev) = &self.device else {
            return json!({"state": "expired"}).to_string();
        };
        if now > dev.expires_at {
            self.device = None;
            return json!({"state": "expired"}).to_string();
        }
        let a = match http.post_json(
            &self.ep.url("/token"),
            &json!({"client_id": CLIENT_ID, "client_secret": CLIENT_SECRET, "code": dev.code, "grant_type": DEVICE_GRANT}),
        ) {
            Ok(a) => a,
            Err(e) => return json!({"state": "error", "error": e}).to_string(),
        };
        let v = a.json().unwrap_or(Value::Null);
        if let (200, Some(t), Some(r)) = (
            a.status,
            v.get("access_token").and_then(Value::as_str),
            v.get("refresh_token").and_then(Value::as_str),
        ) {
            let exp = now + v.get("expires_in").and_then(Value::as_f64).unwrap_or(3600.0) * 1000.0;
            self.device = None;
            self.refresh = Some(r.to_string());
            self.access = Some((t.to_string(), exp));
            (self.persist)(Some(r));
            return json!({"state": "done", "token": t, "exp": exp}).to_string();
        }
        match err_of(&a).as_str() {
            "authorization_pending" => json!({"state": "pending"}),
            "slow_down" => json!({"state": "pending", "slow": true}),
            "access_denied" => {
                self.device = None;
                json!({"state": "denied"})
            }
            "expired_token" => {
                self.device = None;
                json!({"state": "expired"})
            }
            e => json!({"state": "error", "error": e}),
        }
        .to_string()
    }

    /// Signs out: the grant is revoked at Google (best effort) and forgotten here.
    pub fn sign_out(&mut self, http: &Http) {
        if let Some(r) = self.refresh.clone() {
            let _ = http.post_json(&format!("{}?token={r}", self.ep.url("/revoke")), &json!({}));
        }
        self.forget();
    }

    fn forget(&mut self) {
        self.refresh = None;
        self.access = None;
        self.device = None;
        (self.persist)(None);
    }
}

/// A random UUID v4 for `device_id` (kept by the shell so the account lists one TV).
pub fn new_device_id() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}
