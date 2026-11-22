const COMMANDS: &[&str] = &[
    "list",
    "language",
    "describe",
    "add",
    "remove",
    "set_enabled",
    "take_links",
    "autostart",
    "set_autostart",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build configuration");
}
