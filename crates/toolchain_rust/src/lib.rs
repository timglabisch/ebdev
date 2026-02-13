mod install;
mod platform;

pub use install::{install_rust, InstallError};

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RustEnvError {
    #[error("Rust installation not found at {0}")]
    NotFound(PathBuf),
}

#[derive(Debug, Clone)]
pub struct RustEnv {
    install_dir: PathBuf,
    version: String,
}

impl RustEnv {
    pub fn new(base_path: &Path, version: &str) -> Result<Self, RustEnvError> {
        let install_dir = base_path
            .join(".ebdev")
            .join("toolchain")
            .join("rust")
            .join(format!("v{version}"));

        if !install_dir.join("cargo_home").join("bin").join("rustc").exists() {
            return Err(RustEnvError::NotFound(install_dir));
        }

        Ok(Self {
            install_dir,
            version: version.to_string(),
        })
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.install_dir.join("cargo_home").join("bin")
    }

    pub fn rustup_home(&self) -> PathBuf {
        self.install_dir.join("rustup_home")
    }

    pub fn cargo_home(&self) -> PathBuf {
        self.install_dir.join("cargo_home")
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}
