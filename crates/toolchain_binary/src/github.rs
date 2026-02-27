use crate::download;
use crate::platform::{Arch, Platform};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GhInstallError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("gh CLI v{version} not found at {url}")]
    NotFound { version: String, url: String },

    #[error("Extraction error: {0}")]
    Extract(String),
}

fn gh_platform() -> (&'static str, &'static str) {
    match (Platform::current(), Arch::current()) {
        (Platform::Darwin, Arch::Arm64) => ("macOS", "arm64"),
        (Platform::Darwin, Arch::Amd64) => ("macOS", "amd64"),
        (Platform::Linux, Arch::Arm64) => ("linux", "arm64"),
        (Platform::Linux, Arch::Amd64) => ("linux", "amd64"),
    }
}

fn gh_download_url(version: &str) -> String {
    let (os, arch) = gh_platform();
    let ext = match Platform::current() {
        Platform::Darwin => "zip",
        Platform::Linux => "tar.gz",
    };
    format!(
        "https://github.com/cli/cli/releases/download/v{version}/gh_{version}_{os}_{arch}.{ext}"
    )
}

fn gh_binary_path_in_archive(version: &str) -> String {
    let (os, arch) = gh_platform();
    format!("gh_{version}_{os}_{arch}/bin/gh")
}

fn gh_install_dir(base_path: &Path, version: &str) -> PathBuf {
    base_path
        .join(".ebdev")
        .join("toolchain")
        .join("binary")
        .join("gh")
        .join(format!("v{version}"))
}

pub async fn install_gh(version: &str, base_path: &Path) -> Result<PathBuf, GhInstallError> {
    let install_dir = gh_install_dir(base_path, version);

    if install_dir.exists() {
        println!("gh v{version} already installed");
        return Ok(install_dir);
    }

    let url = gh_download_url(version);
    println!("Downloading {url}...");

    let response = reqwest::get(&url).await?;

    if !response.status().is_success() {
        return Err(GhInstallError::NotFound {
            version: version.to_string(),
            url,
        });
    }

    let temp_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("binary")
        .join("gh")
        .join(".downloads");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let temp_file = temp_dir.join(format!("gh-v{version}.tmp"));

    download::stream_to_file(response, &temp_file)
        .await
        .map_err(|e| GhInstallError::Extract(e.to_string()))?;

    tokio::fs::create_dir_all(&install_dir).await?;

    let binary_path_in_archive = gh_binary_path_in_archive(version);
    let dest = install_dir.join("gh");

    match Platform::current() {
        Platform::Darwin => {
            download::extract_zip_file(&temp_file, &binary_path_in_archive, &dest)
                .map_err(|e| GhInstallError::Extract(e.to_string()))?;
        }
        Platform::Linux => {
            let temp_extract = temp_dir.join(format!("gh-v{version}-extract"));
            tokio::fs::create_dir_all(&temp_extract).await?;
            download::extract_tar_gz(&temp_file, &temp_extract)?;
            let src = temp_extract.join(&binary_path_in_archive);
            tokio::fs::rename(&src, &dest).await?;
            tokio::fs::remove_dir_all(&temp_extract).await.ok();
        }
    }

    #[cfg(unix)]
    download::set_executable(&dest)?;

    tokio::fs::remove_file(&temp_file).await.ok();

    println!("Installed gh v{version} to {}", install_dir.display());
    Ok(install_dir)
}

/// Resolve a GitHub token for authenticated downloads.
///
/// Tries in order:
/// 1. System `gh` in PATH → `gh auth token`
/// 2. Managed `gh` under `.ebdev/toolchain/binary/gh/v{version}/gh` → `gh auth token`
/// 3. `GITHUB_TOKEN` env var
/// 4. `GH_TOKEN` env var
/// 5. None
pub async fn resolve_github_token(
    base_path: &Path,
    gh_version: Option<&str>,
) -> Option<String> {
    // 1. System gh
    if let Ok(output) = tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
    {
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }

    // 2. Managed gh
    if let Some(version) = gh_version {
        let managed_gh = gh_install_dir(base_path, version).join("gh");
        if managed_gh.exists() {
            if let Ok(output) = tokio::process::Command::new(&managed_gh)
                .args(["auth", "token"])
                .output()
                .await
            {
                if output.status.success() {
                    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !token.is_empty() {
                        return Some(token);
                    }
                }
            }
        }
    }

    // 3. GITHUB_TOKEN
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    // 4. GH_TOKEN
    if let Ok(token) = std::env::var("GH_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }

    None
}
