//! Configuration types for mutagen synchronization

use serde::{Deserialize, Serialize};

/// Sync mode for mutagen synchronization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    /// Two-way synchronization (default)
    #[default]
    TwoWay,
    /// One-way synchronization, create files on beta
    OneWayCreate,
    /// One-way synchronization, replicate alpha to beta
    OneWayReplica,
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
        assert_eq!(SyncMode::default(), SyncMode::TwoWay);
    }

    #[test]
    fn test_parse_sync_mode() {
        let json_str = r#"{"mode": "one-way-create"}"#;
        #[derive(Deserialize)]
        struct Test {
            mode: SyncMode,
        }
        let t: Test = serde_json::from_str(json_str).unwrap();
        assert_eq!(t.mode, SyncMode::OneWayCreate);
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
