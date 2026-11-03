fn main() {
    // The bundle's telemetry variant (`./build.sh --tv --dev` writes bundle/content-dev.js; tv/scripts/build.sh
    // builds it with every APK). Without it the app has none built in (an empty one stands in).
    let dev = std::path::Path::new("bundle/content-dev.js");
    println!("cargo:rerun-if-changed=bundle/content-dev.js");
    let path = if dev.exists() {
        std::fs::canonicalize(dev).expect("bundle path")
    } else {
        let p = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("content-dev.js");
        std::fs::write(&p, "").expect("placeholder");
        p
    };
    println!("cargo:rustc-env=LUMEN_TV_DEV_BUNDLE={}", path.display());
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&["ytc"]))).expect("tauri build");
}
