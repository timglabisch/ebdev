mod install;
mod platform;

pub use install::{install_mutagen, InstallError};

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MutagenEnvError {
    #[error("Mutagen installation not found at {0}")]
    NotFound(PathBuf),
}

#[derive(Debug, Clone)]
pub struct MutagenEnv {
    install_dir: PathBuf,
    version: String,
}

impl MutagenEnv {
    pub fn new(base_path: &Path, version: &str) -> Result<Self, MutagenEnvError> {
        let install_dir = base_path
            .join(".ebdev")
            .join("toolchain")
            .join("mutagen")
            .join(format!("v{version}"));

        if !install_dir.exists() {
            return Err(MutagenEnvError::NotFound(install_dir));
        }

        Ok(Self {
            install_dir,
            version: version.to_string(),
        })
    }

    pub fn bin_path(&self) -> PathBuf {
        self.install_dir.join("mutagen")
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }
}
