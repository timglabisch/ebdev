use crate::{download, platform};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Binary '{name}' v{version} not found at {url}")]
    NotFound {
        name: String,
        version: String,
        url: String,
    },

    #[error("Binary '{name}' v{version} not found at {url} (HTTP 404)\n  Hint: Run `gh auth login` or set GITHUB_TOKEN env var.")]
    NotFoundAuth {
        name: String,
        version: String,
        url: String,
    },

    #[error("Download error: {0}")]
    Download(String),

    #[error("GitHub API error: {0}")]
    GhApi(#[from] crate::github::GhApiError),
}

pub struct InstallBinaryOptions<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub url_template: &'a str,
    pub binary_path: Option<&'a str>,
    pub base_path: &'a Path,
    pub gh_version: Option<&'a str>,
}

fn resolve_url(url_template: &str, version: &str) -> String {
    let target = platform::target_triple();
    url_template
        .replace("{version}", version)
        .replace("{target}", target)
}

/// Detect archive type from URL
enum ArchiveType {
    TarGz,
    TarXz,
    Plain,
}

fn detect_archive_type(url: &str) -> ArchiveType {
    if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        ArchiveType::TarGz
    } else if url.ends_with(".tar.xz") {
        ArchiveType::TarXz
    } else {
        ArchiveType::Plain
    }
}

pub async fn install_binary(opts: &InstallBinaryOptions<'_>) -> Result<PathBuf, InstallError> {
    let InstallBinaryOptions {
        name,
        version,
        url_template,
        binary_path,
        base_path,
        gh_version,
    } = opts;

    let install_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("binary")
        .join(name)
        .join(format!("v{version}"));

    if install_dir.exists() {
        println!("{name} v{version} already installed");
        return Ok(install_dir.clone());
    }

    let is_gh_url = url_template.starts_with("gh:");
    let effective_template = if is_gh_url {
        &url_template[3..]
    } else {
        url_template
    };

    let url = resolve_url(effective_template, version);
    let archive_type = detect_archive_type(&url);

    println!("Downloading {url}...");

    let response = if is_gh_url {
        // For gh: URLs, resolve auth token and use GitHub API for private repos
        let token = crate::github::resolve_github_token(base_path, *gh_version).await;
        let token = token.ok_or_else(|| InstallError::NotFoundAuth {
            name: name.to_string(),
            version: version.to_string(),
            url: url.clone(),
        })?;

        if url.starts_with("https://github.com/") {
            // Use GitHub API (required for private repos)
            crate::github::download_gh_release_asset(&url, &token).await?
        } else {
            // Non-github.com URL: direct download with auth header
            let client = reqwest::Client::new();
            let response = client
                .get(&url)
                .header("Authorization", format!("token {token}"))
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(InstallError::NotFoundAuth {
                    name: name.to_string(),
                    version: version.to_string(),
                    url,
                });
            }
            response
        }
    } else {
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(InstallError::NotFound {
                name: name.to_string(),
                version: version.to_string(),
                url,
            });
        }
        response
    };

    let temp_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("binary")
        .join(name)
        .join(".downloads");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let temp_file = temp_dir.join(format!("{name}-v{version}.tmp"));

    download::stream_to_file(response, &temp_file)
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    tokio::fs::create_dir_all(&install_dir).await?;

    let binary_name = binary_path.unwrap_or(name);

    match archive_type {
        ArchiveType::TarGz => {
            println!("Extracting (tar.gz)...");
            let temp_extract = temp_dir.join(format!("{name}-v{version}-extract"));
            tokio::fs::create_dir_all(&temp_extract).await?;

            download::extract_tar_gz(&temp_file, &temp_extract)?;

            let src = temp_extract.join(binary_name);
            let dest = install_dir.join(name);
            tokio::fs::rename(&src, &dest).await?;

            #[cfg(unix)]
            download::set_executable(&dest)?;

            tokio::fs::remove_dir_all(&temp_extract).await.ok();
        }
        ArchiveType::TarXz => {
            println!("Extracting (tar.xz)...");
            let temp_extract = temp_dir.join(format!("{name}-v{version}-extract"));
            tokio::fs::create_dir_all(&temp_extract).await?;

            download::extract_tar_xz(&temp_file, &temp_extract)?;

            let src = temp_extract.join(binary_name);
            let dest = install_dir.join(name);
            tokio::fs::rename(&src, &dest).await?;

            #[cfg(unix)]
            download::set_executable(&dest)?;

            tokio::fs::remove_dir_all(&temp_extract).await.ok();
        }
        ArchiveType::Plain => {
            let dest = install_dir.join(name);
            tokio::fs::rename(&temp_file, &dest).await?;

            #[cfg(unix)]
            download::set_executable(&dest)?;
        }
    }

    // Cleanup temp file (may already be moved for Plain)
    tokio::fs::remove_file(&temp_file).await.ok();

    println!("Installed {name} v{version} to {}", install_dir.display());

    Ok(install_dir)
}
