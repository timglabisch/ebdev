use crate::platform::{Arch, Platform};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Mutagen version {version} not found for {platform}-{arch}")]
    VersionNotFound {
        version: String,
        platform: &'static str,
        arch: &'static str,
    },
}

fn download_url(version: &str, platform: Platform, arch: Arch) -> String {
    format!(
        "https://github.com/mutagen-io/mutagen/releases/download/v{version}/mutagen_{platform}_{arch}_v{version}.tar.gz",
        platform = platform.as_str(),
        arch = arch.as_str(),
    )
}

pub async fn install_mutagen(version: &str, base_path: &Path) -> Result<PathBuf, InstallError> {
    let platform = Platform::current();
    let arch = Arch::current();

    let install_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("mutagen")
        .join(format!("v{version}"));

    if install_dir.exists() {
        println!("Mutagen v{version} already installed");
        return Ok(install_dir);
    }

    let url = download_url(version, platform, arch);
    println!("Downloading {url}...");

    let response = reqwest::get(&url).await?;

    if !response.status().is_success() {
        return Err(InstallError::VersionNotFound {
            version: version.to_string(),
            platform: platform.as_str(),
            arch: arch.as_str(),
        });
    }

    let temp_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("mutagen")
        .join(".downloads");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let archive_path = temp_dir.join(format!("mutagen-v{version}.tar.gz"));

    let mut file = tokio::fs::File::create(&archive_path).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    println!("Extracting...");

    tokio::fs::create_dir_all(&install_dir).await?;

    extract_archive(&archive_path, &install_dir)?;

    tokio::fs::remove_file(&archive_path).await.ok();

    println!("Installed Mutagen v{version} to {}", install_dir.display());

    Ok(install_dir)
}

fn extract_archive(archive_path: &Path, dest: &Path) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}
