use crate::download;
use crate::platform::{Arch, Platform};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Parsed components of a GitHub releases download URL.
struct GhReleaseUrl {
    owner: String,
    repo: String,
    tag: String,
    asset_name: String,
}

/// Parse `https://github.com/{owner}/{repo}/releases/download/{tag}/{asset_name}`
fn parse_gh_release_url(url: &str) -> Option<GhReleaseUrl> {
    let path = url.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = path.splitn(6, '/').collect();
    // parts: [owner, repo, "releases", "download", tag, asset_name]
    if parts.len() == 6 && parts[2] == "releases" && parts[3] == "download" {
        Some(GhReleaseUrl {
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            tag: parts[4].to_string(),
            asset_name: parts[5].to_string(),
        })
    } else {
        None
    }
}

/// Download a release asset from the GitHub API (works for private repos).
///
/// Steps:
/// 1. `GET /repos/{owner}/{repo}/releases/tags/{tag}` → find asset ID by name
/// 2. `GET /repos/{owner}/{repo}/releases/assets/{id}` with `Accept: application/octet-stream`
pub async fn download_gh_release_asset(
    url: &str,
    token: &str,
) -> Result<reqwest::Response, GhApiError> {
    let parsed = parse_gh_release_url(url).ok_or_else(|| {
        GhApiError::InvalidUrl(url.to_string())
    })?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    // Step 1: Resolve asset ID
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/releases/tags/{}",
        parsed.owner, parsed.repo, parsed.tag
    );

    let release_resp = client
        .get(&api_url)
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", "ebdev")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;

    if !release_resp.status().is_success() {
        return Err(GhApiError::ReleaseNotFound {
            owner: parsed.owner,
            repo: parsed.repo,
            tag: parsed.tag,
            status: release_resp.status().as_u16(),
        });
    }

    let bytes = release_resp.bytes().await?;
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| GhApiError::InvalidResponse(e.to_string()))?;
    let assets = body["assets"]
        .as_array()
        .ok_or_else(|| GhApiError::InvalidResponse("no assets array".into()))?;

    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&parsed.asset_name))
        .ok_or_else(|| GhApiError::AssetNotFound {
            asset_name: parsed.asset_name.clone(),
            tag: parsed.tag.clone(),
        })?;

    let asset_id = asset["id"]
        .as_u64()
        .ok_or_else(|| GhApiError::InvalidResponse("asset has no id".into()))?;

    // Step 2: Download asset via API
    let asset_url = format!(
        "https://api.github.com/repos/{}/{}/releases/assets/{}",
        parsed.owner, parsed.repo, asset_id
    );

    let download_resp = client
        .get(&asset_url)
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", "ebdev")
        .header("Accept", "application/octet-stream")
        .send()
        .await?;

    if !download_resp.status().is_success() {
        return Err(GhApiError::DownloadFailed {
            asset_name: parsed.asset_name,
            status: download_resp.status().as_u16(),
        });
    }

    Ok(download_resp)
}

#[derive(Debug, Error)]
pub enum GhApiError {
    #[error("Invalid GitHub release URL: {0}")]
    InvalidUrl(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Release not found: {owner}/{repo} tag {tag} (HTTP {status})")]
    ReleaseNotFound {
        owner: String,
        repo: String,
        tag: String,
        status: u16,
    },

    #[error("Asset '{asset_name}' not found in release {tag}")]
    AssetNotFound { asset_name: String, tag: String },

    #[error("Invalid API response: {0}")]
    InvalidResponse(String),

    #[error("Download failed for asset '{asset_name}' (HTTP {status})")]
    DownloadFailed { asset_name: String, status: u16 },
}

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
