use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_INDENT_DEPTH: usize = 5;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub device_email: String,
    #[serde(default)]
    pub from_email: String,
    #[serde(default = "default_smtp_host")]
    pub smtp_host: String,
    #[serde(default)]
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_password: String,
    #[serde(default = "default_output_dir_string")]
    pub output_dir: String,
    #[serde(default = "default_max_indent_depth")]
    pub max_indent_depth: usize,
}

fn default_smtp_host() -> String {
    "smtp.gmail.com".to_string()
}

fn default_output_dir_string() -> String {
    "~/Downloads".to_string()
}

fn default_max_indent_depth() -> usize {
    DEFAULT_MAX_INDENT_DEPTH
}

pub fn default_output_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
        .join("rustypub")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    pub fn load_optional() -> Result<Option<Self>> {
        let path = config_path();
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&contents)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        if cfg.smtp_host.is_empty() {
            cfg.smtp_host = default_smtp_host();
        }
        if cfg.output_dir.is_empty() {
            cfg.output_dir = default_output_dir_string();
        }
        if cfg.max_indent_depth == 0 {
            cfg.max_indent_depth = DEFAULT_MAX_INDENT_DEPTH;
        }
        Ok(Some(cfg))
    }

    pub fn output_dir(&self) -> PathBuf {
        expand_tilde(&self.output_dir)
    }

    pub fn css_override(&self) -> Result<Option<String>> {
        let path = config_dir().join("reader.css");
        if path.exists() {
            Ok(Some(fs::read_to_string(&path).with_context(|| {
                format!("failed to read CSS override {}", path.display())
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn ensure_email_configured(&self) -> Result<()> {
        let missing = [
            ("device_email", self.device_email.as_str()),
            ("from_email", self.from_email.as_str()),
            ("smtp_host", self.smtp_host.as_str()),
            ("smtp_username", self.smtp_username.as_str()),
            ("smtp_password", self.smtp_password.as_str()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.trim().is_empty().then_some(name))
        .collect::<Vec<_>>();
        if !missing.is_empty() {
            anyhow::bail!(
                "email settings missing ({}); edit {} or use --no-email",
                missing.join(", "),
                config_path().display()
            );
        }
        Ok(())
    }
}

pub fn init_config() -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = config_path();
    if !path.exists() {
        let sample = Config {
            device_email: String::new(),
            from_email: String::new(),
            smtp_host: default_smtp_host(),
            smtp_username: String::new(),
            smtp_password: String::new(),
            output_dir: default_output_dir_string(),
            max_indent_depth: DEFAULT_MAX_INDENT_DEPTH,
        };
        fs::write(&path, toml::to_string_pretty(&sample)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
        set_private_permissions(&path)?;
    }

    let css_path = dir.join("reader.css");
    if !css_path.exists() {
        fs::write(&css_path, include_str!("../assets/reader.css"))
            .with_context(|| format!("failed to write {}", css_path.display()))?;
    }

    println!("Wrote {}", path.display());
    println!("Set device_email to the address that accepts EPUB attachments for your reader.");
    println!("Set the SMTP fields for the account that sends those attachments.");
    println!("Some delivery services require from_email to be approved before they accept mail.");
    Ok(())
}

fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(value));
    }
    PathBuf::from(value)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;

    fn complete_email_config() -> Config {
        toml::from_str(
            r#"
device_email = "reader@example.com"
from_email = "sender@example.com"
smtp_username = "sender@example.com"
smtp_password = "secret"
"#,
        )
        .unwrap()
    }

    #[test]
    fn save_only_config_can_omit_email_settings() {
        let config: Config = toml::from_str(
            r#"
output_dir = "~/Books"
max_indent_depth = 4
"#,
        )
        .unwrap();

        assert!(config.device_email.is_empty());
        assert!(config.from_email.is_empty());
        assert_eq!(config.smtp_host, "smtp.gmail.com");
        assert!(config.ensure_email_configured().is_err());
    }

    #[test]
    fn complete_email_settings_validate_together() {
        let config = complete_email_config();

        config.ensure_email_configured().unwrap();
    }

    #[test]
    fn every_email_setting_is_required_for_delivery() {
        for field in [
            "device_email",
            "from_email",
            "smtp_host",
            "smtp_username",
            "smtp_password",
        ] {
            let mut config = complete_email_config();
            match field {
                "device_email" => config.device_email.clear(),
                "from_email" => config.from_email.clear(),
                "smtp_host" => config.smtp_host.clear(),
                "smtp_username" => config.smtp_username.clear(),
                "smtp_password" => config.smtp_password.clear(),
                _ => unreachable!(),
            }

            let message = config.ensure_email_configured().unwrap_err().to_string();
            assert!(
                message.contains(field),
                "missing field not named: {message}"
            );
            assert!(
                message.contains("--no-email"),
                "save-only recovery not named: {message}"
            );
            assert!(
                message.contains("config.toml"),
                "config path not named: {message}"
            );
        }
    }
}
