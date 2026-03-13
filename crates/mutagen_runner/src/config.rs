//! Configuration types for mutagen synchronization

use serde::{Deserialize, Serialize};

/// Sync mode for mutagen synchronization.
///
/// Maps to mutagen's `--sync-mode` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    #[default]
    TwoWaySafe,
    TwoWayResolved,
    OneWaySafe,
    OneWayReplica,
}

impl SyncMode {
    /// Parses a mode string from the .ebdev.ts config.
    /// Returns None for unknown modes.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "two-way-safe" | "two-way" => Some(Self::TwoWaySafe),
            "two-way-resolved" => Some(Self::TwoWayResolved),
            "one-way-safe" | "one-way-create" => Some(Self::OneWaySafe),
            "one-way-replica" => Some(Self::OneWayReplica),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TwoWaySafe => "two-way-safe",
            Self::TwoWayResolved => "two-way-resolved",
            Self::OneWaySafe => "one-way-safe",
            Self::OneWayReplica => "one-way-replica",
        }
    }

    /// All known mode strings, for error messages.
    pub fn known_modes() -> &'static [&'static str] {
        &["two-way-safe", "two-way-resolved", "one-way-safe", "one-way-replica"]
    }
}

/// Polling configuration for mutagen sync
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingConfig {
    /// Enable polling instead of filesystem watching
    #[serde(default)]
    pub enabled: bool,
    /// Polling interval in seconds
    #[serde(default = "default_polling_interval")]
    pub interval: u32,
}

fn default_polling_interval() -> u32 {
    10
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: default_polling_interval(),
        }
    }
}

/// Permissions configuration for mutagen sync
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsConfig {
    /// Default file mode (e.g., 666 for rw-rw-rw-)
    #[serde(default = "default_file_mode", rename = "defaultFileMode")]
    pub default_file_mode: u32,
    /// Default directory mode (e.g., 777 for rwxrwxrwx)
    #[serde(default = "default_directory_mode", rename = "defaultDirectoryMode")]
    pub default_directory_mode: u32,
}

fn default_file_mode() -> u32 {
    0o666
}

fn default_directory_mode() -> u32 {
    0o777
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            default_file_mode: default_file_mode(),
            default_directory_mode: default_directory_mode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_mode_default() {
        assert_eq!(SyncMode::default(), SyncMode::TwoWaySafe);
    }

    #[test]
    fn test_sync_mode_parse() {
        assert_eq!(SyncMode::parse("two-way-safe"), Some(SyncMode::TwoWaySafe));
        assert_eq!(SyncMode::parse("two-way"), Some(SyncMode::TwoWaySafe));
        assert_eq!(SyncMode::parse("two-way-resolved"), Some(SyncMode::TwoWayResolved));
        assert_eq!(SyncMode::parse("one-way-safe"), Some(SyncMode::OneWaySafe));
        assert_eq!(SyncMode::parse("one-way-create"), Some(SyncMode::OneWaySafe));
        assert_eq!(SyncMode::parse("one-way-replica"), Some(SyncMode::OneWayReplica));
        assert_eq!(SyncMode::parse("unknown"), None);
    }

    #[test]
    fn test_sync_mode_as_str() {
        assert_eq!(SyncMode::TwoWaySafe.as_str(), "two-way-safe");
        assert_eq!(SyncMode::TwoWayResolved.as_str(), "two-way-resolved");
        assert_eq!(SyncMode::OneWaySafe.as_str(), "one-way-safe");
        assert_eq!(SyncMode::OneWayReplica.as_str(), "one-way-replica");
    }

    #[test]
    fn test_polling_config_default() {
        let config = PollingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.interval, 10);
    }

    #[test]
    fn test_permissions_config_default() {
        let config = PermissionsConfig::default();
        assert_eq!(config.default_file_mode, 0o666);
        assert_eq!(config.default_directory_mode, 0o777);
    }
}
