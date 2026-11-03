//! Client updates from GitHub Releases.
//!
//! A release carries the shared client's files (`content.js`, `client.css`, `native.css`,
//! `rules.json`), the APK,
//! and `lumen-tv-manifest.json` + `lumen-tv-manifest.json.sig`: the SHA-256 and size of every file, a sequence
//! number (`seq`, release time in seconds: higher = newer) and the oldest shell able to run the
//! bundle (`min_shell`), signed with the release key (ed25519; its public key is compiled into the
//! app, `tv/keys/release.pub`). Nothing is used unless the signature and every hash check out.
//!
//! The app runs the newest verified bundle it has: the cached download or the copy built into the
//! APK (a new APK brings a newer built-in copy, so a stale cache never wins over it). Offline it
//! runs what it has.

use crate::net::Http;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "lumen-tv-manifest.json";
pub const SIG: &str = "lumen-tv-manifest.json.sig";
/// The bundle's files (all required).
pub const FILES: [&str; 4] = ["content.js", "client.css", "native.css", "rules.json"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Apk {
    pub version_code: u32,
    pub version: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: u32,
    pub seq: u64,
    /// Client version and build id (`<version> (<bundle hash>)`, written by `browser/build.cjs --target=tv`).
    pub client: String,
    pub min_shell: u32,
    pub files: BTreeMap<String, FileEntry>,
    #[serde(default)]
    pub apk: Option<Apk>,
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The manifest, if `sig` (base64 ed25519 over the exact bytes) is the release key's.
pub fn verify_manifest(bytes: &[u8], sig: &str, key: &[u8; 32]) -> Result<Manifest, String> {
    let key = VerifyingKey::from_bytes(key).map_err(|e| format!("bad key: {e}"))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(sig.trim())
        .map_err(|_| "signature is not base64".to_string())?;
    let sig = Signature::from_slice(&raw).map_err(|_| "bad signature length".to_string())?;
    key.verify(bytes, &sig).map_err(|_| "signature does not match the release key".to_string())?;
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("manifest: {e}"))?;
    if m.schema != 1 {
        return Err(format!("unknown manifest schema {}", m.schema));
    }
    for f in FILES {
        if !m.files.contains_key(f) {
            return Err(format!("manifest lacks {f}"));
        }
    }
    Ok(m)
}

/// A bundle's file checked against the manifest.
pub fn check_file(m: &Manifest, name: &str, bytes: &[u8]) -> Result<(), String> {
    let e = m.files.get(name).ok_or_else(|| format!("{name} not in the manifest"))?;
    if bytes.len() as u64 != e.size || sha256_hex(bytes) != e.sha256 {
        return Err(format!("{name}: hash mismatch"));
    }
    Ok(())
}

/// A verified client bundle.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub seq: u64,
    pub client: String,
    /// "builtin" or "release"
    pub source: &'static str,
    pub files: BTreeMap<String, String>,
}

impl Bundle {
    pub fn file(&self, name: &str) -> &str {
        self.files.get(name).map(String::as_str).unwrap_or("")
    }
}

/// The bundle cached at `dir` (re-verified: signature and hashes), if any.
pub fn load_cached(dir: &Path, key: &[u8; 32]) -> Option<Bundle> {
    let m = std::fs::read(dir.join(MANIFEST)).ok()?;
    let sig = std::fs::read_to_string(dir.join(SIG)).ok()?;
    let m = verify_manifest(&m, &sig, key).ok()?;
    let mut files = BTreeMap::new();
    for f in FILES {
        let b = std::fs::read(dir.join(f)).ok()?;
        check_file(&m, f, &b).ok()?;
        files.insert(f.to_string(), String::from_utf8(b).ok()?);
    }
    Some(Bundle {
        seq: m.seq,
        client: m.client,
        source: "release",
        files,
    })
}

/// The newer of the cached and the built-in bundle.
pub fn choose(cached: Option<Bundle>, builtin: Bundle) -> Bundle {
    match cached {
        Some(c) if c.seq > builtin.seq => c,
        _ => builtin,
    }
}

/// Download URLs of the newest GitHub release that carries the TV files: `GET
/// /repos/{owner}/{repo}/releases` lists them newest first (a release without them, e.g. a
/// desktop-only one, is skipped); a single release object is read as it is.
pub fn assets(answer: &serde_json::Value) -> BTreeMap<String, String> {
    let release = match answer.as_array() {
        Some(list) => match list
            .iter()
            .filter(|r| r.get("draft").and_then(|d| d.as_bool()) != Some(true))
            .find(|r| assets_of(r).contains_key(MANIFEST))
        {
            Some(r) => r,
            None => return BTreeMap::new(),
        },
        None => answer,
    };
    assets_of(release)
}

fn assets_of(release: &serde_json::Value) -> BTreeMap<String, String> {
    release
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a.get("name")?.as_str()?.to_string(), a.get("browser_download_url")?.as_str()?.to_string())))
        .collect()
}

/// A newer shell (APK) the release offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ApkOffer {
    pub apk: Apk,
    pub url: String,
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// A newer bundle, verified and now in the cache (applies on the next page load).
    pub bundle: Option<Bundle>,
    /// A newer APK (the owner is asked; installing needs their OK on the TV).
    pub apk: Option<ApkOffer>,
    /// The release's bundle needs a newer shell than this one.
    pub needs_shell: bool,
}

/// Checks the latest release at `api` and installs a newer bundle into `cache` (written to a
/// sibling directory first, swapped in only when complete and verified).
pub fn check(http: &Http, api: &str, key: &[u8; 32], cache: &Path, current_seq: u64, shell_code: u32) -> Result<Outcome, String> {
    let rel = http.get(api, &[("Accept", "application/vnd.github+json")])?;
    if rel.status != 200 {
        return Err(format!("release lookup: HTTP {}", rel.status));
    }
    let urls = assets(&rel.json().ok_or("release lookup: not JSON")?);
    let get = |name: &str| -> Result<Vec<u8>, String> {
        let url = urls.get(name).ok_or_else(|| format!("release has no {name}"))?;
        let a = http.get(url, &[])?;
        if a.status != 200 {
            return Err(format!("{name}: HTTP {}", a.status));
        }
        Ok(a.body)
    };
    let mbytes = get(MANIFEST)?;
    let sig = String::from_utf8(get(SIG)?).map_err(|_| "signature is not text")?;
    let m = verify_manifest(&mbytes, &sig, key)?;
    let mut out = Outcome::default();
    if let Some(apk) = &m.apk {
        if apk.version_code > shell_code {
            if let Some(url) = urls.get(&apk.file) {
                out.apk = Some(ApkOffer {
                    apk: apk.clone(),
                    url: url.clone(),
                });
            }
        }
    }
    if m.seq <= current_seq {
        return Ok(out);
    }
    if m.min_shell > shell_code {
        out.needs_shell = true;
        return Ok(out);
    }
    let stage = sibling(cache, "new");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let mut files = BTreeMap::new();
    for f in FILES {
        let b = get(f)?;
        check_file(&m, f, &b)?;
        std::fs::write(stage.join(f), &b).map_err(|e| e.to_string())?;
        files.insert(f.to_string(), String::from_utf8(b).map_err(|_| format!("{f} is not UTF-8"))?);
    }
    std::fs::write(stage.join(MANIFEST), &mbytes).map_err(|e| e.to_string())?;
    std::fs::write(stage.join(SIG), sig.as_bytes()).map_err(|e| e.to_string())?;
    let old = sibling(cache, "old");
    let _ = std::fs::remove_dir_all(&old);
    if cache.exists() {
        std::fs::rename(cache, &old).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&stage, cache).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&old);
    out.bundle = Some(Bundle {
        seq: m.seq,
        client: m.client,
        source: "release",
        files,
    });
    Ok(out)
}

/// Downloads an offered APK to `dir` and checks its hash; returns its path.
pub fn download_apk(http: &Http, offer: &ApkOffer, dir: &Path) -> Result<PathBuf, String> {
    download_apk_progress(http, offer, dir, &mut |_, _| {})
}

/// `download_apk` reporting `on(got, total)` while it downloads (total from the signed manifest).
pub fn download_apk_progress(http: &Http, offer: &ApkOffer, dir: &Path, on: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let size = offer.apk.size;
    let a = http.get_progress(&offer.url, &mut |got, total| on(got, if size > 0 { size } else { total }))?;
    if a.status != 200 {
        return Err(format!("APK: HTTP {}", a.status));
    }
    if a.body.len() as u64 != offer.apk.size || sha256_hex(&a.body) != offer.apk.sha256 {
        return Err("APK: hash mismatch".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = dir.join("update.apk");
    crate::write_atomic(&p, &a.body).map_err(|e| e.to_string())?;
    Ok(p)
}

fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(format!(".{suffix}"));
    PathBuf::from(s)
}

/// The public key from its base64 text form (`tv/keys/release.pub`).
pub fn parse_key(b64: &str) -> Result<[u8; 32], String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| "key is not base64".to_string())?;
    raw.try_into().map_err(|_| "key is not 32 bytes".to_string())
}
