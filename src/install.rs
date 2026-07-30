use crate::cli::InstallArgs;
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const HOST_NAME: &str = "com.chaselambert.rustypub";

pub fn install(args: InstallArgs) -> Result<()> {
    let exe = std::env::current_exe()
        .context("failed to locate current executable")?
        .canonicalize()
        .context("failed to canonicalize current executable")?;

    let chrome_manifest = chrome_manifest(&exe, &args.extension_id);
    write_manifest(
        &native_host_dir("google-chrome").join(format!("{HOST_NAME}.json")),
        &chrome_manifest,
        args.dry_run,
    )?;
    write_manifest(
        &native_host_dir("chromium").join(format!("{HOST_NAME}.json")),
        &chrome_manifest,
        args.dry_run,
    )?;

    if let Some(firefox_id) = args.firefox_id {
        let firefox_manifest = firefox_manifest(&exe, &firefox_id);
        let path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".mozilla/native-messaging-hosts")
            .join(format!("{HOST_NAME}.json"));
        write_manifest(&path, &firefox_manifest, args.dry_run)?;
    }
    Ok(())
}

fn chrome_manifest(exe: &Path, extension_id: &str) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "RustyPub native host",
        "path": exe,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{extension_id}/")],
    })
}

fn firefox_manifest(exe: &Path, extension_id: &str) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "RustyPub native host",
        "path": exe,
        "type": "stdio",
        "allowed_extensions": [extension_id],
    })
}

fn native_host_dir(browser: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join(browser)
        .join("NativeMessagingHosts")
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
    use super::{HOST_NAME, chrome_manifest, firefox_manifest};
    use std::path::Path;

    #[test]
    fn manifests_use_rustypub_host_identity() {
        let exe = Path::new("/opt/rustypub");
        let chrome = chrome_manifest(exe, "chrome-id");
        let firefox = firefox_manifest(exe, "rustypub@example.com");

        assert_eq!(HOST_NAME, "com.chaselambert.rustypub");
        assert_eq!(chrome["name"], HOST_NAME);
        assert_eq!(firefox["name"], HOST_NAME);
        assert_eq!(
            chrome["allowed_origins"][0],
            "chrome-extension://chrome-id/"
        );
        assert_eq!(firefox["allowed_extensions"][0], "rustypub@example.com");
    }
}
