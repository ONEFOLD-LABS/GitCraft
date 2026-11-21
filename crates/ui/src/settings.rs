//! The settings file (`settings.json` in the platform's config directory). Starts with the
//! avatar switch; theme, language and recent repositories follow (issue #7).
//!
//! Linux/BSD `$XDG_CONFIG_HOME/gitcraft` or `~/.config/gitcraft`, macOS
//! `~/Library/Application Support/GitCraft`, Windows `%APPDATA%\GitCraft`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Load author images from Gravatar (sends a hash of each author's e-mail address; off by
    /// default, docs/decisions/ADR-005).
    pub avatars: bool,
}

/// What loading the settings found.
#[derive(Debug, PartialEq, Eq)]
pub enum Loaded {
    /// No file yet: defaults.
    Missing,
    Ok,
    /// The file could not be read; defaults are used and the old file is kept as `.bak`.
    Broken(String),
}

impl Settings {
    /// Reads the settings from `dir`; defaults when there are none or they are broken.
    pub fn load(dir: &Path) -> (Settings, Loaded) {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Settings::default(), Loaded::Missing);
            }
            Err(e) => return (Settings::default(), Loaded::Broken(e.to_string())),
        };
        match serde_json::from_str(&text) {
            Ok(s) => (s, Loaded::Ok),
            Err(e) => {
                let _ = std::fs::rename(&path, dir.join(format!("{FILE}.bak")));
                (Settings::default(), Loaded::Broken(e.to_string()))
            }
        }
    }

    /// Writes the settings to `dir` atomically (temporary file, then rename).
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, text + "\n").map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
    }
}

/// The platform's config directory for GitCraft.
pub fn config_dir() -> Option<PathBuf> {
    platform_dir(
        "XDG_CONFIG_HOME",
        ".config",
        "Library/Application Support",
        "APPDATA",
    )
}

/// The platform's cache directory for GitCraft (avatar images).
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("GitCraft").join("cache"));
    }
    platform_dir("XDG_CACHE_HOME", ".cache", "Library/Caches", "LOCALAPPDATA")
}

fn platform_dir(xdg: &str, xdg_default: &str, mac: &str, windows: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os(windows).map(|d| PathBuf::from(d).join("GitCraft"));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join(mac).join("GitCraft"));
    }
    std::env::var_os(xdg)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(xdg_default)))
        .map(|d| d.join("gitcraft"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_missing_and_broken() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(dir.path()),
            (Settings::default(), Loaded::Missing)
        );
        let s = Settings { avatars: true };
        s.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), (s, Loaded::Ok));

        std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
        let (s, loaded) = Settings::load(dir.path());
        assert_eq!(s, Settings::default());
        assert!(matches!(loaded, Loaded::Broken(_)));
        assert!(
            dir.path().join("settings.json.bak").exists(),
            "broken file kept"
        );
    }

    #[test]
    fn unknown_and_missing_keys_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), r#"{"future": 1}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).0, Settings::default());
    }
}
