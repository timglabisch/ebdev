use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum MutagenConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("WalkDir error: {0}")]
    WalkDir(#[from] walkdir::Error),
}

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A mutagen sync project definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutagenSyncProject {
    /// Name of the sync project
    pub name: String,
    /// Remote target (e.g., "user@host:/path")
    pub target: String,
    /// Local directory to sync (defaults to config file's directory)
    #[serde(default)]
    pub directory: Option<PathBuf>,
    /// Sync mode
    #[serde(default)]
    pub mode: SyncMode,
    /// Polling configuration
    #[serde(default)]
    pub polling: PollingConfig,
    /// Paths to ignore during sync
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Stage for ordering sync projects (lower runs first)
    #[serde(default)]
    pub stage: i32,
}

/// Mutagen section in .ebdev.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MutagenSyncConfig {
    /// List of sync projects
    #[serde(default)]
    pub sync: Vec<MutagenSyncProject>,
}

/// Partial config for reading only mutagen section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialConfig {
    #[serde(default)]
    mutagen: Option<MutagenSyncConfig>,
}

/// A discovered mutagen project with its resolved directory
#[derive(Debug, Clone)]
pub struct DiscoveredProject {
    /// The project configuration
    pub project: MutagenSyncProject,
    /// The resolved local directory (either explicit or derived from config location)
    pub resolved_directory: PathBuf,
    /// Path to the config file this project was found in
    pub config_path: PathBuf,
    /// Path to the root config file (the base_path .ebdev.toml)
    pub root_config_path: PathBuf,
}

impl DiscoveredProject {
    /// Compute CRC32 of the configuration content (for detecting config changes)
    pub fn config_crc32(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        // Hash all relevant config fields
        hasher.update(self.project.name.as_bytes());
        hasher.update(self.project.target.as_bytes());
        if let Some(ref dir) = self.project.directory {
            hasher.update(dir.to_string_lossy().as_bytes());
        }
        hasher.update(&[self.project.mode as u8]);
        hasher.update(&self.project.polling.enabled.to_string().as_bytes());
        hasher.update(&self.project.polling.interval.to_le_bytes());
        for pattern in &self.project.ignore {
            hasher.update(pattern.as_bytes());
        }
        hasher.update(&self.project.stage.to_le_bytes());
        hasher.finalize()
    }

    /// Compute CRC32 of the root config path (for scoping sessions to this installation)
    pub fn root_crc32(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(self.root_config_path.to_string_lossy().as_bytes());
        hasher.finalize()
    }

    /// Generate the session name with CRC32 suffixes
    /// Format: {project_name}-{config_crc32:08x}-{root_crc32:08x}
    /// Note: Mutagen doesn't allow underscores in session names, so we use hyphens
    pub fn session_name(&self) -> String {
        format!(
            "{}-{:08x}-{:08x}",
            self.project.name,
            self.config_crc32(),
            self.root_crc32()
        )
    }

    /// Extract root_crc32 suffix from a session name (last 8 hex chars)
    pub fn extract_root_crc32_suffix(session_name: &str) -> Option<&str> {
        // Format: name-configcrc-rootcrc
        // We need the last 8 characters (root_crc32)
        if session_name.len() < 18 {
            // minimum: a-xxxxxxxx-xxxxxxxx
            return None;
        }
        let parts: Vec<&str> = session_name.rsplitn(2, '-').collect();
        if parts.len() == 2 && parts[0].len() == 8 {
            Some(parts[0])
        } else {
            None
        }
    }

    /// Check if a session name belongs to this root installation
    pub fn session_belongs_to_root(&self, session_name: &str) -> bool {
        let root_suffix = format!("{:08x}", self.root_crc32());
        Self::extract_root_crc32_suffix(session_name)
            .map(|s| s == root_suffix)
            .unwrap_or(false)
    }
}

/// Discovers all mutagen sync projects by recursively walking from base_path
pub fn discover_projects(base_path: &Path) -> Result<Vec<DiscoveredProject>, MutagenConfigError> {
    // Canonicalize base_path to get absolute path for root_config_path
    let canonical_base = base_path.canonicalize().unwrap_or_else(|_| base_path.to_path_buf());
    let root_config_path = canonical_base.join(".ebdev.toml");

    let mut discovered = Vec::new();

    for entry in WalkDir::new(base_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
    {
        let entry = entry?;

        if entry.file_type().is_file() && entry.file_name() == ".ebdev.toml" {
            let config_path = entry.path().to_path_buf();
            let config_dir = config_path.parent().unwrap_or(base_path);

            let content = std::fs::read_to_string(&config_path)?;
            let partial: PartialConfig = toml::from_str(&content)?;

            if let Some(mutagen_config) = partial.mutagen {
                for project in mutagen_config.sync {
                    let resolved_directory = project
                        .directory
                        .clone()
                        .map(|d| {
                            if d.is_absolute() {
                                d
                            } else {
                                config_dir.join(d)
                            }
                        })
                        .unwrap_or_else(|| config_dir.to_path_buf());

                    // Normalize the path to remove . and .. components
                    let resolved_directory = normalize_path(&resolved_directory);

                    discovered.push(DiscoveredProject {
                        project,
                        resolved_directory,
                        config_path: config_path.clone(),
                        root_config_path: root_config_path.clone(),
                    });
                }
            }
        }
    }

    Ok(discovered)
}

/// Returns a list of all config file paths that were discovered
pub fn get_config_paths(projects: &[DiscoveredProject]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = projects.iter()
        .map(|p| p.config_path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            c => components.push(c),
        }
    }
    components.iter().collect()
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| {
            s.starts_with('.')
                && s != "."
                && s != ".."
                && s != ".ebdev.toml"
        })
        .unwrap_or(false)
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
        let toml_str = r#"mode = "one-way-create""#;
        #[derive(Deserialize)]
        struct Test {
            mode: SyncMode,
        }
        let t: Test = toml::from_str(toml_str).unwrap();
        assert_eq!(t.mode, SyncMode::OneWayCreate);
    }

    #[test]
    fn test_parse_mutagen_config_multiple_syncs() {
        let toml_str = r#"
[mutagen]
[[mutagen.sync]]
name = "frontend"
target = "user@host:/var/www/frontend"
mode = "two-way"
ignore = ["node_modules", ".git", "dist"]
stage = 1

[mutagen.sync.polling]
enabled = true
interval = 5

[[mutagen.sync]]
name = "backend"
target = "user@host:/var/www/backend"
directory = "./backend"
mode = "one-way-replica"
ignore = ["vendor", ".git"]
stage = 2

[mutagen.sync.polling]
enabled = false
"#;
        let config: PartialConfig = toml::from_str(toml_str).unwrap();
        let mutagen = config.mutagen.unwrap();

        // Should have 2 sync projects
        assert_eq!(mutagen.sync.len(), 2);

        // First sync: frontend
        assert_eq!(mutagen.sync[0].name, "frontend");
        assert_eq!(mutagen.sync[0].target, "user@host:/var/www/frontend");
        assert_eq!(mutagen.sync[0].mode, SyncMode::TwoWay);
        assert!(mutagen.sync[0].polling.enabled);
        assert_eq!(mutagen.sync[0].polling.interval, 5);
        assert_eq!(mutagen.sync[0].ignore, vec!["node_modules", ".git", "dist"]);
        assert_eq!(mutagen.sync[0].stage, 1);
        assert!(mutagen.sync[0].directory.is_none());

        // Second sync: backend
        assert_eq!(mutagen.sync[1].name, "backend");
        assert_eq!(mutagen.sync[1].target, "user@host:/var/www/backend");
        assert_eq!(mutagen.sync[1].mode, SyncMode::OneWayReplica);
        assert!(!mutagen.sync[1].polling.enabled);
        assert_eq!(mutagen.sync[1].ignore, vec!["vendor", ".git"]);
        assert_eq!(mutagen.sync[1].stage, 2);
        assert_eq!(mutagen.sync[1].directory, Some(PathBuf::from("./backend")));
    }
}
