fn main() {
    // the app's own command (main.rs helper) gets a permission, allow-helper, which capabilities/default.json gives the app's site
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&["helper"])))
        .expect("tauri build");
}
