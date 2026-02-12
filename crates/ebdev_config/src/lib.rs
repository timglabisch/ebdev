use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("No config file found (.ebdev.ts)")]
    NotFound,

    #[error("Failed to load TypeScript config: {0}")]
    TypeScript(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub toolchain: ToolchainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainConfig {
    pub ebdev: EbdevSelfConfig,
    pub node: NodeConfig,
    #[serde(default)]
    pub pnpm: Option<PnpmConfig>,
    #[serde(default)]
    pub mutagen: Option<MutagenConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnpmConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutagenConfig {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbdevSelfConfig {
    pub version: String,
}

impl Config {
    /// Load config from a directory (looks for .ebdev.ts)
    pub async fn load_from_dir(dir: &Path) -> Result<Self, ConfigError> {
        let ts_path = dir.join(".ebdev.ts");
        if ts_path.exists() {
            return ebdev_toolchain_deno::load_ts_config(&ts_path).await
                .map_err(|e| ConfigError::TypeScript(e.to_string()));
        }
        Err(ConfigError::NotFound)
    }
}
