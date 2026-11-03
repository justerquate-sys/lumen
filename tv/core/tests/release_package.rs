mod mock;

use lumen_tv_core::{net::Http, release};
use serde_json::json;
use std::{collections::BTreeMap, path::Path, sync::{Arc, Mutex}, time::Duration};

fn package_version(toml: &str) -> &str {
    toml.lines().find_map(|line| line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"'))).unwrap()
}

fn signed_manifest(dir: &Path, key: &[u8; 32]) -> release::Manifest {
    let bytes = std::fs::read(dir.join(release::MANIFEST)).unwrap();
    let sig = std::fs::read_to_string(dir.join(release::SIG)).unwrap();
    release::verify_manifest(&bytes, &sig, key).expect("the shipped app accepts the release signature")
}

/// Run against the staged, signed artifacts and the previous published manifest before publishing.
/// Both directories are supplied explicitly; this test never contacts GitHub or uses a private key.
#[test]
#[ignore = "requires LUMEN_TV_RELEASE_DIR and LUMEN_TV_PREVIOUS_RELEASE_DIR"]
fn signed_package_updates_the_previous_release() {
    let dir = std::path::PathBuf::from(std::env::var_os("LUMEN_TV_RELEASE_DIR").expect("staged release directory"));
    let previous_dir = std::path::PathBuf::from(std::env::var_os("LUMEN_TV_PREVIOUS_RELEASE_DIR").expect("previous release directory"));
    let key = release::parse_key(include_str!("../../keys/release.pub")).unwrap();
    let current = signed_manifest(&dir, &key);
    let previous = signed_manifest(&previous_dir, &key);
    let apk = current.apk.as_ref().expect("the release includes an APK");
    let old_apk = previous.apk.as_ref().expect("the previous app version is known");
    let shell_version = package_version(include_str!("../../shell/src-tauri/Cargo.toml"));
    let client_version = package_version(include_str!("../../../Cargo.toml"));
    assert_eq!(apk.version, shell_version, "APK manifest names the current app");
    assert!(current.client.starts_with(&format!("{client_version} (")), "interface version names this source");
    assert!(current.seq > previous.seq, "installed interfaces must see a newer sequence");
    assert!(apk.version_code > old_apk.version_code, "Android and the updater must offer the new APK");
    let mut files = BTreeMap::new();
    for name in current.files.keys() {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        release::check_file(&current, name, &bytes).expect("signed bundle matches the staged file");
        files.insert(name.clone(), bytes);
    }
    for name in [release::MANIFEST, release::SIG, apk.file.as_str(), "lumen-tv.apk"] {
        files.insert(name.into(), std::fs::read(dir.join(name)).unwrap());
    }
    assert_eq!(files[&apk.file], files["lumen-tv.apk"], "the first-install link serves the same APK");
    assert_eq!(files[&apk.file].len() as u64, apk.size);
    assert_eq!(release::sha256_hex(&files[&apk.file]), apk.sha256);
    let files = Arc::new(files);
    let served = files.clone();
    let base = Arc::new(Mutex::new(String::new()));
    let host = base.clone();
    let server = mock::start(Arc::new(move |_, path, _| {
        if path == "/repos/o/r/releases" {
            let base = host.lock().unwrap();
            let assets: Vec<_> = served.keys().map(|name| json!({"name": name, "browser_download_url": format!("{base}/dl/{name}")})).collect();
            return (200, "application/json".into(), json!([{"tag_name": "verified-package", "draft": false, "assets": assets}]).to_string().into_bytes());
        }
        match path.strip_prefix("/dl/").and_then(|name| served.get(name)) {
            Some(bytes) => (200, "application/octet-stream".into(), bytes.clone()),
            None => (404, "text/plain".into(), Vec::new()),
        }
    }));
    *base.lock().unwrap() = server.base.clone();
    let temp = std::env::temp_dir().join(format!("lumen-release-package-{}", std::process::id()));
    std::fs::create_dir_all(&temp).unwrap();
    let http = Http::new(Duration::from_secs(15));
    let api = format!("{}/repos/o/r/releases", server.base);
    let cache = temp.join("cache");
    let update = release::check(&http, &api, &key, &cache, previous.seq, old_apk.version_code).unwrap();
    let offer = update.apk.expect("the installed app offers this APK");
    assert_eq!(offer.apk, *apk);
    if current.min_shell <= old_apk.version_code {
        let bundle = update.bundle.expect("the installed app can use the new interface immediately");
        assert_eq!(bundle.client, current.client);
        assert_eq!(bundle.seq, current.seq);
        assert_eq!(release::load_cached(&cache, &key).unwrap().seq, current.seq);
    } else {
        assert!(update.needs_shell && update.bundle.is_none());
    }
    let downloaded = release::download_apk(&http, &offer, &temp).unwrap();
    assert_eq!(std::fs::read(downloaded).unwrap(), files[&apk.file]);
    let installed = release::check(&http, &api, &key, &cache, current.seq, apk.version_code).unwrap();
    assert!(installed.bundle.is_none() && installed.apk.is_none(), "the installed version is up to date");
    std::fs::remove_dir_all(temp).unwrap();
}
