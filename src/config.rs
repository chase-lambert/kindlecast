use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_INDENT_DEPTH: usize = 5;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub kindle_email: String,
    pub from_email: String,
    #[serde(default = "default_smtp_host")]
    pub smtp_host: String,
    pub smtp_username: String,
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
        .join("kindlecast")
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
        let path = config_dir().join("kindle.css");
        if path.exists() {
            Ok(Some(fs::read_to_string(&path).with_context(|| {
                format!("failed to read CSS override {}", path.display())
            })?))
        } else {
            Ok(None)
        }
    }
}

pub fn init_config() -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = config_path();
    if !path.exists() {
        let sample = Config {
            kindle_email: "you@kindle.com".to_string(),
            from_email: "you@gmail.com".to_string(),
            smtp_host: default_smtp_host(),
            smtp_username: "you@gmail.com".to_string(),
            smtp_password: "gmail-app-password".to_string(),
            output_dir: default_output_dir_string(),
            max_indent_depth: DEFAULT_MAX_INDENT_DEPTH,
        };
        fs::write(&path, toml::to_string_pretty(&sample)?)
            .with_context(|| format!("failed to write {}", path.display()))?;
        set_private_permissions(&path)?;
    }

    let css_path = dir.join("kindle.css");
    if !css_path.exists() {
        fs::write(&css_path, include_str!("../assets/kindle.css"))
            .with_context(|| format!("failed to write {}", css_path.display()))?;
    }

    println!("Wrote {}", path.display());
    println!("Edit smtp_username/from_email and smtp_password with a Gmail app password.");
    println!("Make sure from_email is approved in Amazon's Personal Document E-mail List.");
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
