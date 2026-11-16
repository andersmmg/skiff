//! User settings, persisted as JSON under the platform config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub download_dir: PathBuf,
    /// Mirrors the engine's `--max-concurrent-downloads`.
    pub max_concurrent: u32,
    /// Mirrors `--max-connection-per-server`.
    pub connection_limit: u32,
    /// Mirrors `--split`.
    pub split: u32,
    /// The engine's JSON-RPC port on 127.0.0.1. Stable across launches so
    /// browser extensions can be pointed at it once and left alone.
    pub rpc_port: u16,
    /// The engine's JSON-RPC secret, generated on first launch and persisted;
    /// extensions pass it as the first RPC parameter (`token:…`).
    pub rpc_secret: String,
    /// Explicit engine binary (e.g. `~/Downloads/aria2-next`); when empty the
    /// engine is discovered via `SKIFF_ARIA2_NEXT` or `PATH`.
    pub engine_binary: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: dirs::download_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(std::env::temp_dir),
            max_concurrent: 4,
            connection_limit: 8,
            split: 8,
            // aria2's conventional port, so extensions defaulting to 6801
            // work out of the box.
            rpc_port: 6801,
            rpc_secret: String::new(),
            engine_binary: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let mut settings: Self = Self::path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        // First run: mint the extension-facing RPC secret once and keep it.
        if settings.rpc_secret.is_empty() {
            settings.rpc_secret = (0..32).map(|_| fastrand::alphanumeric()).collect();
        }
        settings.save();
        settings
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
            restrict(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(error) = std::fs::write(&path, text) {
                    log::warn!(target: "settings", "cannot write {}: {error}", path.display());
                }
                // The file holds the RPC secret; owner-only for owner and dir.
                restrict(&path);
            }
            Err(error) => log::warn!(target: "settings", "cannot serialize settings: {error}"),
        }
    }

    fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("skiff").join("settings.json"))
    }

    /// The engine's launch configuration for these settings — used at
    /// startup and re-sent to the supervisor on every save, so engine-bound
    /// options (port, secret, dir, limits, binary) apply without restarting
    /// the app.
    pub fn engine_config(&self) -> engine::EngineConfig {
        let mut config = engine::EngineConfig::standard();
        config.port = self.rpc_port;
        config.secret = self.rpc_secret.clone();
        config.download_dir = self.download_dir.clone();
        config.max_concurrent = self.max_concurrent;
        config.connection_limit = self.connection_limit;
        config.split = self.split;
        config.binary = self.engine_binary.clone();
        config
    }
}

/// Best-effort owner-only permissions (0700 dir, 0600 file) on unix.
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings files written before the RPC fields existed must still load,
    /// gaining the default port and an empty (freshly generated) secret.
    #[test]
    fn legacy_settings_json_deserializes_with_rpc_defaults() {
        let legacy = r#"{
            "download_dir": "/home/user/Downloads",
            "max_concurrent": 8,
            "connection_limit": 4,
            "split": 4
        }"#;
        let settings: Settings = serde_json::from_str(legacy).expect("legacy json");
        assert_eq!(settings.rpc_port, 6801);
        assert_eq!(settings.rpc_secret, "");
        assert_eq!(settings.max_concurrent, 8);
    }

    #[test]
    fn settings_roundtrip_keeps_rpc_fields() {
        let mut settings = Settings::default();
        settings.rpc_secret = "hunter2".into();
        settings.rpc_port = 6899;
        let text = serde_json::to_string(&settings).expect("serialize");
        let back: Settings = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back.rpc_port, 6899);
        assert_eq!(back.rpc_secret, "hunter2");
    }

    /// The settings the form edits are exactly the engine's launch arguments
    /// — this is what a save re-sends to the supervisor.
    #[test]
    fn engine_config_mirrors_saved_settings() {
        let mut settings = Settings::default();
        settings.download_dir = PathBuf::from("/tmp/dl");
        settings.max_concurrent = 7;
        settings.connection_limit = 3;
        settings.split = 5;
        settings.rpc_port = 6902;
        settings.rpc_secret = "s3cret".into();
        settings.engine_binary = Some(PathBuf::from("/bin/aria2-next"));

        let config = settings.engine_config();
        assert_eq!(config.port, 6902);
        assert_eq!(config.secret, "s3cret");
        assert_eq!(config.download_dir, PathBuf::from("/tmp/dl"));
        assert_eq!(config.max_concurrent, 7);
        assert_eq!(config.connection_limit, 3);
        assert_eq!(config.split, 5);
        assert_eq!(config.binary, Some(PathBuf::from("/bin/aria2-next")));
    }
}
