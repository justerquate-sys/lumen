//! The ready Home response kept across app starts (owner decision, 3 Nov 2026; CONTRIBUTING "Privacy"). The page
//! keeps YouTube's latest answer to its Home request ready for the next Home (`src/ready.rs`); on the TV the shell also
//! keeps that record on disk, so a cold app start paints Home on its first frame instead of waiting for YouTube:
//!
//! - sealed with the Android Keystore key the shell already uses for the account (`seal`/`open` are the host's);
//!   without a working seal nothing is written;
//! - only for the account it was fetched for ([`crate::auth::Account::fingerprint`]): another account, or signing out,
//!   deletes it;
//! - only while younger than [`MAX_AGE_MS`] (the page's own limit);
//! - read once: [`take`] deletes the file whatever it finds.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "ready.bin";
/// The page's `ready::MAX_AGE_MS`: older, it is not what YouTube would answer now.
pub const MAX_AGE_MS: f64 = 10.0 * 60_000.0;
/// Largest record kept (the page's record is ~50 KB of parsed items, at most 150 of them).
pub const MAX_RECORD: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Entry {
    /// The account's fingerprint.
    a: String,
    /// When the shell got it (epoch ms).
    t: f64,
    /// The page's record (`ready::to_json`), opaque here.
    r: String,
}

/// Keeps `record` for `account` (replacing what was there). Err (and nothing on disk) when it cannot be sealed.
pub fn put(path: &Path, account: &str, now: f64, record: &str, seal: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Result<(), String> {
    if record.is_empty() || record.len() > MAX_RECORD {
        remove(path);
        return Err("no record".into());
    }
    let plain = serde_json::to_vec(&Entry { a: account.into(), t: now, r: record.into() }).map_err(|e| e.to_string())?;
    let Some(sealed) = seal(&plain) else {
        remove(path);
        return Err("cannot seal".into());
    };
    crate::write_atomic(path, &sealed).map_err(|e| e.to_string())
}

/// The record kept for `account`, if it is younger than [`MAX_AGE_MS`]; the file is deleted in every case (read once).
pub fn take(path: &Path, account: &str, now: f64, open: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Option<String> {
    let sealed = std::fs::read(path).ok()?;
    remove(path);
    let e: Entry = serde_json::from_slice(&open(&sealed)?).ok()?;
    (e.a == account && now >= e.t - 60_000.0 && now - e.t < MAX_AGE_MS && !e.r.is_empty()).then_some(e.r)
}

/// Deletes it (signed out, another account, the page dropped it: something personalizing happened).
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    // A stand-in for the Keystore: XOR with a key byte plus a tag, so tampering and plain text show.
    fn seal(b: &[u8]) -> Option<Vec<u8>> {
        let mut v: Vec<u8> = b.iter().map(|x| x ^ 0x5a).collect();
        v.extend_from_slice(b"TAG");
        Some(v)
    }
    fn open(b: &[u8]) -> Option<Vec<u8>> {
        let body = b.strip_suffix(b"TAG")?;
        Some(body.iter().map(|x| x ^ 0x5a).collect())
    }

    fn dir(n: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-ready-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const REC: &str = r#"{"v":1,"t":1000,"a":"t:en","i":[{"k":"v","id":"abcdefghijk","ti":"A title"}]}"#;

    #[test]
    fn sealed_on_disk_and_read_once() {
        let d = dir("once");
        let p = d.join(FILE);
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("A title") && !String::from_utf8_lossy(&raw).contains("in:aaaa"), "sealed, not plain");
        assert_eq!(take(&p, "in:aaaa", 1000.0 + 9.0 * 60_000.0, open).as_deref(), Some(REC));
        assert!(!p.exists(), "deleted once read");
        assert_eq!(take(&p, "in:aaaa", 1000.0, open), None, "used once");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn expiry_account_and_damage() {
        let d = dir("rules");
        let p = d.join(FILE);
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        assert_eq!(take(&p, "in:aaaa", 1000.0 + 10.0 * 60_000.0, open), None, "10 min old: expired");
        assert!(!p.exists(), "an expired one is deleted too");
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        assert_eq!(take(&p, "in:bbbb", 1000.0, open), None, "another account");
        assert!(!p.exists());
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        assert_eq!(take(&p, "out", 1000.0, open), None, "signed out since");
        put(&p, "in:aaaa", 1_000_000.0, REC, seal).unwrap();
        assert_eq!(take(&p, "in:aaaa", 1000.0, open), None, "from the future (a clock that jumped back)");
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        let mut raw = std::fs::read(&p).unwrap();
        raw.truncate(raw.len() - 1);
        std::fs::write(&p, raw).unwrap();
        assert_eq!(take(&p, "in:aaaa", 1000.0, open), None, "a damaged file");
        assert!(!p.exists());
        // No seal (off Android, a broken Keystore): nothing is written, an old file goes.
        put(&p, "in:aaaa", 1000.0, REC, seal).unwrap();
        assert!(put(&p, "in:aaaa", 1000.0, REC, |_| None).is_err() && !p.exists());
        assert!(put(&p, "in:aaaa", 1000.0, "", seal).is_err() && !p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
