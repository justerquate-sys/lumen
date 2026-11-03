//! The shell's state and the `ytc` command the page calls (`YTC_SHELL.call`, tv/shim/shim.js):
//!
//! | cmd            | answer                                                                  |
//! |----------------|-------------------------------------------------------------------------|
//! | `auth_token`   | `{"token", "exp"}` (refreshed when due) or `{"token": null}`            |
//! | `auth_start`   | `{"user_code", "verification_url", "expires_in", "interval"}`           |
//! | `auth_poll`    | `{"state": "pending"/"done"/"denied"/"expired"/"error", ...}`           |
//! | `auth_signout` | `{}`                                                                    |
//! | `store_get/set/remove` | chrome.storage.local                                            |
//! | `shell_info`   | shell version, bundle in use, update state                              |
//! | `shell_check`  | checks for updates now, then answers like `shell_info`                  |
//!
//! Updates: at launch and every 6 hours the latest GitHub release is checked (`lumen_tv_core::release`);
//! a newer verified bundle is cached and handed to the WebView (`host::bundle_ready`), a newer APK is
//! offered (`host::offer_update`).

use crate::host;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;
use lumen_tv_core::auth::{Account, Endpoints};
use lumen_tv_core::net::Http;
use lumen_tv_core::release::{self, ApkOffer, Bundle};
use lumen_tv_core::rules::{Decision, Rules};
use lumen_tv_core::store::Store;

/// The release key (`tv/keys/release.pub`): only bundles and APKs it signed are used.
const RELEASE_KEY: &str = include_str!("../../../keys/release.pub");
/// Where releases come from (`LUMEN_TV_REPO=owner/name` at build time).
const REPO: &str = match option_env!("LUMEN_TV_REPO") {
    Some(r) => r,
    None => "justerquate-sys/lumen",
};
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Ask about the same shell update again after this long.
const OFFER_AGAIN_MS: f64 = 3.0 * 24.0 * 3600.0 * 1000.0;

/// The client bundle built into this APK (`./build.sh --tv` writes it).
fn builtin() -> Bundle {
    let mut files = std::collections::BTreeMap::new();
    files.insert("content.js".into(), include_str!("../bundle/content.js").into());
    files.insert("client.css".into(), include_str!("../bundle/client.css").into());
    files.insert("native.css".into(), include_str!("../bundle/native.css").into());
    files.insert("rules.json".into(), include_str!("../bundle/rules.json").into());
    Bundle {
        seq: include_str!("../bundle/seq").trim().parse().unwrap_or(0),
        client: include_str!("../bundle/client").trim().into(),
        source: "builtin",
        files,
    }
}

pub struct Shell {
    dir: PathBuf,
    key: [u8; 32],
    http: Http,
    api: String,
    account: Mutex<Account>,
    store: Mutex<Store>,
    bundle: RwLock<(Bundle, Rules)>,
    offer: Mutex<Option<ApkOffer>>,
    status: Mutex<Value>,
}

static SHELL: OnceLock<Shell> = OnceLock::new();

pub fn get() -> Option<&'static Shell> {
    SHELL.get()
}

/// Version name and Android versionCode (Tauri's: major·10⁶ + minor·10³ + patch).
pub fn version() -> (&'static str, u32) {
    let v = env!("CARGO_PKG_VERSION");
    let n: Vec<u32> = v.split('.').map(|x| x.parse().unwrap_or(0)).collect();
    (v, n.first().unwrap_or(&0) * 1_000_000 + n.get(1).unwrap_or(&0) * 1000 + n.get(2).unwrap_or(&0))
}

fn now() -> f64 {
    lumen_tv_core::now_ms()
}

/// Debug builds only: `files/dev.json` points the release check and the sign-in at local fakes
/// (`{"release_api": "http://10.0.2.2:8765/repos/o/r/releases", "oauth": "http://10.0.2.2:8765"}`).
fn dev(dir: &std::path::Path) -> Value {
    if !cfg!(debug_assertions) {
        return Value::Null;
    }
    std::fs::read(dir.join("dev.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

pub fn init(dir: PathBuf) {
    if SHELL.get().is_some() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let key = release::parse_key(RELEASE_KEY).expect("release key");
    let dev = dev(&dir);
    // `LUMEN_TV_RELEASE_API` at build time replaces GitHub entirely (test builds against a local fake).
    let api = dev
        .get("release_api")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| option_env!("LUMEN_TV_RELEASE_API").map(String::from))
        .unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases?per_page=10"));
    let ep = Endpoints {
        base: dev.get("oauth").and_then(Value::as_str).unwrap_or(lumen_tv_core::auth::GOOGLE).to_string(),
    };
    // The account: the refresh token sealed by the Keystore (host::seal); a device id per install.
    let device_id = match std::fs::read_to_string(dir.join("device-id")) {
        Ok(s) if s.trim().len() == 36 => s.trim().to_string(),
        _ => {
            let id = lumen_tv_core::auth::new_device_id();
            let _ = std::fs::write(dir.join("device-id"), &id);
            id
        }
    };
    let sealed = dir.join("account.bin");
    let refresh = std::fs::read(&sealed).ok().and_then(|b| host::open(&b)).and_then(|b| String::from_utf8(b).ok());
    let persist = {
        let sealed = sealed.clone();
        Box::new(move |r: Option<&str>| match r.and_then(|r| host::seal(r.as_bytes())) {
            Some(b) => {
                let _ = lumen_tv_core::write_atomic(&sealed, &b);
            }
            None => {
                let _ = std::fs::remove_file(&sealed);
            }
        })
    };
    let account = Account::new(ep, device_id, refresh, persist);
    let bundle = release::choose(release::load_cached(&dir.join("bundle"), &key), builtin());
    let rules = Rules::parse(bundle.file("rules.json")).unwrap_or_else(|_| Rules::parse("[]").unwrap());
    let store = Store::open(dir.join("storage.json"));
    let status = json!({"state": "idle"});
    let _ = SHELL.set(Shell {
        dir,
        key,
        http: Http::new(Duration::from_secs(30)),
        api,
        account: Mutex::new(account),
        store: Mutex::new(store),
        bundle: RwLock::new((bundle, rules)),
        offer: Mutex::new(None),
        status: Mutex::new(status),
    });
    // Home reload, early start: a new process holds no access token, and the client's first account
    // request (Home) waits for `auth_token`, which would refresh it only then. The refresh starts
    // now, in parallel with the WebView, the document and the client's Wasm; the page's
    // `auth_token` call waits on the same lock and gets the fresh token without a second request.
    std::thread::spawn(|| {
        if let Some(s) = get() {
            let _ = s.account.lock().unwrap().token(&s.http, now());
        }
    });
}

impl Shell {
    pub fn script(&self) -> String {
        let b = self.bundle.read().unwrap();
        crate::script::compose(&b.0, version().0)
    }

    pub fn decide(&self, url: &str, main_frame: bool, top: &str) -> Decision {
        self.bundle.read().unwrap().1.decide(url, main_frame, top)
    }

    pub fn call(&self, cmd: &str, arg: &str) -> Result<String, String> {
        let arg: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
        match cmd {
            "auth_token" => Ok(lumen_tv_core::auth::token_json(self.account.lock().unwrap().token(&self.http, now())?.as_ref())),
            "auth_start" => self.account.lock().unwrap().start(&self.http, now()),
            "auth_poll" => Ok(self.account.lock().unwrap().poll(&self.http, now())),
            "auth_signout" => {
                self.account.lock().unwrap().sign_out(&self.http);
                Ok("{}".into())
            }
            "store_get" => Ok(self.store.lock().unwrap().get(&arg).to_string()),
            "store_set" => self.store.lock().unwrap().set(&arg).map(|_| "{}".into()),
            "store_remove" => self.store.lock().unwrap().remove(&arg).map(|_| "{}".into()),
            "shell_info" => {
                let b = self.bundle.read().unwrap();
                let (v, code) = version();
                Ok(json!({"version": v, "version_code": code, "bundle": {"seq": b.0.seq, "client": b.0.client, "source": b.0.source}, "update": *self.status.lock().unwrap(), "repo": REPO}).to_string())
            }
            // Checks for updates now (the TV settings' "check for updates"; tests).
            "shell_check" => {
                self.check();
                self.call("shell_info", "null")
            }
            _ => Err(format!("unknown command {cmd}")),
        }
    }

    /// One update check (blocking; the update thread).
    pub fn check(&self) {
        let seq = self.bundle.read().unwrap().0.seq;
        let code = version().1;
        *self.status.lock().unwrap() = json!({"state": "checking", "at": now()});
        match release::check(&self.http, &self.api, &self.key, &self.dir.join("bundle"), seq, code) {
            Ok(out) => {
                let mut st = json!({"state": "ok", "at": now(), "needs_shell": out.needs_shell});
                if let Some(b) = out.bundle {
                    log::info!("client bundle {} (seq {}) installed", b.client, b.seq);
                    st["installed"] = json!({"client": b.client, "seq": b.seq});
                    let rules = Rules::parse(b.file("rules.json")).unwrap_or_else(|_| Rules::parse("[]").unwrap());
                    *self.bundle.write().unwrap() = (b, rules);
                    host::bundle_ready();
                }
                if let Some(o) = out.apk {
                    st["apk"] = json!({"version": o.apk.version, "version_code": o.apk.version_code});
                    let ask = self.should_offer(&o.apk.version);
                    *self.offer.lock().unwrap() = Some(o.clone());
                    if ask {
                        host::offer_update(&o.apk.version);
                    }
                }
                *self.status.lock().unwrap() = st;
            }
            Err(e) => {
                log::warn!("update check: {e}");
                *self.status.lock().unwrap() = json!({"state": "failed", "at": now(), "error": e});
            }
        }
    }

    fn should_offer(&self, version: &str) -> bool {
        let p = self.dir.join("offer.json");
        let last: Value = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        if last.get("version").and_then(Value::as_str) == Some(version) && now() - last.get("at").and_then(Value::as_f64).unwrap_or(0.0) < OFFER_AGAIN_MS {
            return false;
        }
        let _ = std::fs::write(&p, json!({"version": version, "at": now()}).to_string());
        true
    }

    /// The owner accepted the shell update: download, check, hand to the installer.
    pub fn accept_update(&self) {
        let Some(o) = self.offer.lock().unwrap().clone() else { return };
        match release::download_apk(&self.http, &o, &self.dir.join("updates")) {
            Ok(p) => host::install_apk(&p.to_string_lossy()),
            Err(e) => {
                log::warn!("shell update: {e}");
                *self.status.lock().unwrap() = json!({"state": "failed", "at": now(), "error": e});
            }
        }
    }
}

/// The update thread: a check shortly after launch (the first page goes first), then every 6 hours.
pub fn spawn_updates() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(8));
        loop {
            if let Some(s) = get() {
                s.check();
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
}
