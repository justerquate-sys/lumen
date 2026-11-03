fn main() {
    // Optional bundle files (an empty one stands in for a missing one): the telemetry variant
    // (`./build.sh --tv --dev`: content-dev.js + client-dev.wasm; tv/scripts/build.sh builds it with every APK) and
    // the format-2 Wasm (`./build.sh --tv`: client.wasm, served to the page by the shell).
    for (file, var) in [("content-dev.js", "LUMEN_TV_DEV_BUNDLE"), ("client.wasm", "LUMEN_TV_WASM"), ("client-dev.wasm", "LUMEN_TV_DEV_WASM")] {
        let f = std::path::Path::new("bundle").join(file);
        println!("cargo:rerun-if-changed=bundle/{file}");
        let path = if f.exists() {
            std::fs::canonicalize(&f).expect("bundle path")
        } else {
            let p = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join(file);
            std::fs::write(&p, "").expect("placeholder");
            p
        };
        println!("cargo:rustc-env={var}={}", path.display());
    }
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&["ytc"]))).expect("tauri build");
}
