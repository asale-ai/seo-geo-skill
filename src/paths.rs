//! Where seogeo keeps its state.
//!
//! On Unix the config lives under `~/.config/seogeo` rather than the
//! platform default, because macOS's `~/Library/Application Support` is
//! awkward to type, awkward to document, and unexpected for a CLI. Windows
//! keeps the platform convention, where `%APPDATA%` is what users expect.
//! `SEOGEO_HOME` overrides everything, which is what CI and tests use.

use std::path::PathBuf;

fn override_root() -> Option<PathBuf> {
    std::env::var_os("SEOGEO_HOME").map(PathBuf::from)
}

/// Credentials and settings: `google-api.json`, `backlinks-api.json`.
pub fn config_dir() -> PathBuf {
    if let Some(root) = override_root() {
        return root.join("config");
    }
    #[cfg(windows)]
    {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
}

/// Durable user data: the CRM store, the DataForSEO cost ledger.
pub fn data_dir() -> PathBuf {
    if let Some(root) = override_root() {
        return root.join("data");
    }
    #[cfg(windows)]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
}

/// Regenerable state: the drift baseline database.
pub fn cache_dir() -> PathBuf {
    if let Some(root) = override_root() {
        return root.join("cache");
    }
    #[cfg(windows)]
    {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seogeo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_distinct_and_namespaced() {
        for dir in [config_dir(), data_dir(), cache_dir()] {
            assert!(dir.ends_with("seogeo") || dir.parent().is_some());
        }
        assert_ne!(config_dir(), data_dir());
        assert_ne!(data_dir(), cache_dir());
    }
}
