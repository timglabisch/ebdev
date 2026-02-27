mod download;
mod github;
mod install;
mod platform;

pub use github::{install_gh, GhInstallError};
pub use install::{install_binary, InstallBinaryOptions, InstallError};

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BinaryEnvError {
    #[error("Binary '{name}' installation not found at {path}")]
    NotFound { name: String, path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct BinaryEnv {
    install_dir: PathBuf,
    name: String,
    version: String,
}

impl BinaryEnv {
    pub fn new(base_path: &Path, name: &str, version: &str) -> Result<Self, BinaryEnvError> {
        let install_dir = base_path
            .join(".ebdev")
            .join("toolchain")
            .join("binary")
            .join(name)
            .join(format!("v{version}"));

        if !install_dir.exists() {
            return Err(BinaryEnvError::NotFound {
                name: name.to_string(),
                path: install_dir,
            });
        }

        Ok(Self {
            install_dir,
            name: name.to_string(),
            version: version.to_string(),
        })
    }

    pub fn bin_path(&self) -> PathBuf {
        self.install_dir.join(&self.name)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }
}
