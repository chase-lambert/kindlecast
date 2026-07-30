use crate::cli::{InstallArgs, InstallBrowser};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub const HOST_NAME: &str = "com.chaselambert.rustypub";
pub const FIREFOX_EXTENSION_ID: &str = "@rustypub.chaselambert";

pub fn install(args: InstallArgs) -> Result<()> {
    let exe = std::env::current_exe()
        .context("failed to locate current executable")?
        .canonicalize()
        .context("failed to canonicalize current executable")?;

    if !args.dry_run {
        warn_if_ephemeral_helper_path(&exe);
    }

    let (directory, manifest) = match args.browser {
        InstallBrowser::Chrome(browser) => (
            chromium_native_host_dir("google-chrome")?,
            chromium_manifest(&exe, &browser.extension_id),
        ),
        InstallBrowser::Chromium(browser) => (
            chromium_native_host_dir("chromium")?,
            chromium_manifest(&exe, &browser.extension_id),
        ),
        InstallBrowser::Firefox => (firefox_native_host_dir()?, firefox_manifest(&exe)),
    };

    write_manifest(
        &directory.join(format!("{HOST_NAME}.json")),
        &manifest,
        args.dry_run,
    )
}

/// Native messaging stores an absolute path. A Cargo `target/` binary stops
/// working when that tree is cleaned or the project moves; prefer
/// `cargo install --path . --force --locked` so the host points at a stable
/// `~/.cargo/bin` path (same durable-host pattern as What Lobsters Says).
fn warn_if_ephemeral_helper_path(exe: &Path) {
    let is_target_tree = exe.components().any(|c| c.as_os_str() == "target");
    if is_target_tree {
        eprintln!(
            "warning: registering helper at {}; this is a Cargo target/ build path.\n\
             For a durable install: cargo install --path . --force --locked\n\
             then re-run `rustypub install …` from that installed binary.",
            exe.display()
        );
    }
}

fn chromium_manifest(exe: &Path, extension_id: &str) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "RustyPub native host",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{extension_id}/")],
    })
}

fn firefox_manifest(exe: &Path) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "RustyPub native host",
        "path": exe,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    })
}

fn chromium_native_host_dir(browser: &str) -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("failed to locate the user configuration directory")?
        .join(browser)
        .join("NativeMessagingHosts"))
}

fn firefox_native_host_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("failed to locate the user home directory")?
        .join(".mozilla")
        .join("native-messaging-hosts"))
}

fn write_manifest(path: &Path, value: &serde_json::Value, dry_run: bool) -> Result<()> {
    let rendered = serde_json::to_string_pretty(value)?;
    if dry_run {
        println!("Would write {}\n{}", path.display(), rendered);
        return Ok(());
    }

    let parent = path
        .parent()
        .with_context(|| format!("manifest path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn chromium_manifest_uses_an_extension_origin() {
        let manifest = chromium_manifest(Path::new("/opt/rustypub"), "abc123");

        assert_eq!(
            manifest["allowed_origins"],
            json!(["chrome-extension://abc123/"])
        );
        assert!(manifest.get("allowed_extensions").is_none());
        assert_eq!(manifest["name"], HOST_NAME);
    }

    #[test]
    fn firefox_manifest_uses_the_stable_add_on_id() {
        let manifest = firefox_manifest(Path::new("/opt/rustypub"));

        assert_eq!(
            manifest["allowed_extensions"],
            json!([FIREFOX_EXTENSION_ID])
        );
        assert!(manifest.get("allowed_origins").is_none());
        assert_eq!(manifest["name"], HOST_NAME);
    }

    #[test]
    fn firefox_add_on_and_native_host_ids_stay_in_sync() {
        // Chrome's load-unpacked manifest is service_worker-only; gecko identity
        // lives in the Firefox package manifest (see extension/prepare-firefox.sh).
        let extension_manifest: serde_json::Value =
            serde_json::from_str(include_str!("../extension/manifest.firefox.json")).unwrap();

        assert_eq!(
            extension_manifest["browser_specific_settings"]["gecko"]["id"],
            FIREFOX_EXTENSION_ID
        );
        assert_eq!(
            extension_manifest["background"]["scripts"],
            json!(["background.js"])
        );
        assert!(
            extension_manifest["background"]
                .get("service_worker")
                .is_none()
        );
    }

    #[test]
    fn chrome_manifest_is_service_worker_only() {
        let chrome: serde_json::Value =
            serde_json::from_str(include_str!("../extension/manifest.json")).unwrap();
        assert_eq!(chrome["background"]["service_worker"], "background.js");
        assert!(chrome["background"].get("scripts").is_none());
        assert!(chrome.get("browser_specific_settings").is_none());
    }

    #[test]
    fn host_name_stays_in_sync_with_extension_background() {
        let background = include_str!("../extension/background.js");
        let expected = format!("const HOST = \"{HOST_NAME}\";");
        assert!(
            background.lines().any(|line| line.trim() == expected),
            "background.js must declare HOST = {HOST_NAME:?}"
        );
    }

    #[test]
    fn target_tree_paths_are_treated_as_ephemeral() {
        assert!(
            Path::new("/home/me/projects/rustypub/target/release/rustypub")
                .components()
                .any(|c| c.as_os_str() == "target")
        );
        assert!(
            !Path::new("/home/me/.cargo/bin/rustypub")
                .components()
                .any(|c| c.as_os_str() == "target")
        );
    }
}
