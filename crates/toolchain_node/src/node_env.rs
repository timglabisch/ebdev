use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum NodeEnvError {
    #[error("Node installation not found at {0}")]
    NotFound(PathBuf),

    #[error("Command failed: {0}")]
    CommandFailed(#[from] std::io::Error),

    #[error("Command exited with status: {0}")]
    NonZeroExit(ExitStatus),
}

#[derive(Debug, Clone)]
pub struct NodeEnv {
    toolchain_dir: PathBuf,
    node_version: String,
}

impl NodeEnv {
    pub fn new(base_path: &Path, node_version: &str) -> Result<Self, NodeEnvError> {
        let toolchain_dir = base_path.join(".ebdev").join("toolchain");
        let node_dir = toolchain_dir.join("node").join(format!("v{node_version}"));

        if !node_dir.exists() {
            return Err(NodeEnvError::NotFound(node_dir));
        }

        Ok(Self {
            toolchain_dir,
            node_version: node_version.to_string(),
        })
    }

    pub fn node_dir(&self) -> PathBuf {
        self.toolchain_dir.join("node").join(format!("v{}", self.node_version))
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.node_dir().join("bin")
    }

    pub fn npm_bin(&self) -> PathBuf {
        self.bin_dir().join("npm")
    }

    /// .ebdev/toolchain/pnpm/node_22.12.0/pnpm_9.15.0/
    pub fn pnpm_dir(&self, pnpm_version: &str) -> PathBuf {
        self.toolchain_dir
            .join("pnpm")
            .join(format!("node_{}", self.node_version))
            .join(format!("pnpm_{pnpm_version}"))
    }

    pub fn pnpm_bin_dir(&self, pnpm_version: &str) -> PathBuf {
        self.pnpm_dir(pnpm_version).join("bin")
    }

    pub fn build_path(&self, pnpm_version: Option<&str>) -> OsString {
        let mut paths: Vec<PathBuf> = Vec::new();

        // pnpm bin dir first (if configured)
        if let Some(v) = pnpm_version {
            paths.push(self.pnpm_bin_dir(v));
        }

        // node bin dir
        paths.push(self.bin_dir());

        // existing PATH
        if let Some(existing) = std::env::var_os("PATH") {
            for path in std::env::split_paths(&existing) {
                paths.push(path);
            }
        }

        std::env::join_paths(paths).unwrap_or_default()
    }

    pub async fn run(&self, cmd: &str, args: &[&str], path: &OsString) -> Result<ExitStatus, NodeEnvError> {
        let status = Command::new(cmd)
            .args(args)
            .env("PATH", path)
            .status()
            .await?;

        if !status.success() {
            return Err(NodeEnvError::NonZeroExit(status));
        }

        Ok(status)
    }

    pub async fn install_pnpm(&self, pnpm_version: &str) -> Result<PathBuf, NodeEnvError> {
        let pnpm_dir = self.pnpm_dir(pnpm_version);

        if pnpm_dir.exists() {
            println!("pnpm {} already installed", pnpm_version);
            return Ok(pnpm_dir);
        }

        println!("Installing pnpm {}...", pnpm_version);

        let status = Command::new(self.npm_bin())
            .args([
                "install",
                "-g",
                &format!("pnpm@{pnpm_version}"),
                "--prefix",
                pnpm_dir.to_str().unwrap(),
            ])
            .status()
            .await?;

        if !status.success() {
            return Err(NodeEnvError::NonZeroExit(status));
        }

        println!("pnpm {} installed to {}", pnpm_version, pnpm_dir.display());

        Ok(pnpm_dir)
    }
}
