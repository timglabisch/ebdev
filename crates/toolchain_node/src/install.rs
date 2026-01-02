use crate::platform::{Arch, Platform};
use futures_util::StreamExt;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Node version {version} not found for {platform}-{arch}")]
    VersionNotFound {
        version: String,
        platform: &'static str,
        arch: &'static str,
    },

    #[error("Version already installed at {0}")]
    AlreadyInstalled(PathBuf),
}

const NODE_DIST_URL: &str = "https://nodejs.org/dist";

fn download_url(version: &str, platform: Platform, arch: Arch, ext: &str) -> String {
    format!(
        "{NODE_DIST_URL}/v{version}/node-v{version}-{platform}-{arch}.{ext}",
        platform = platform.as_str(),
        arch = arch.as_str(),
    )
}

pub async fn install_node(version: &str, base_path: &Path) -> Result<PathBuf, InstallError> {
    let platform = Platform::current();
    let arch = Arch::current();

    let install_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("node")
        .join(format!("v{version}"));

    if install_dir.exists() {
        println!("Node.js v{version} already installed");
        return Ok(install_dir);
    }

    let extensions = ["tar.xz", "tar.gz"];

    for ext in extensions {
        let url = download_url(version, platform, arch, ext);
        println!("Downloading {url}...");

        let response = reqwest::get(&url).await?;

        if !response.status().is_success() {
            continue;
        }

        let temp_dir = base_path
            .join(".ebdev")
            .join("toolchain")
            .join("node")
            .join(".downloads");
        tokio::fs::create_dir_all(&temp_dir).await?;

        let archive_path = temp_dir.join(format!("node-v{version}.{ext}"));

        let mut file = tokio::fs::File::create(&archive_path).await?;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        }
        drop(file);

        println!("Extracting...");

        let extract_dir = temp_dir.join(format!("node-v{version}-extract"));
        tokio::fs::create_dir_all(&extract_dir).await?;

        extract_archive(&archive_path, &extract_dir, ext)?;

        let extracted_folder_name = format!("node-v{version}-{}-{}", platform.as_str(), arch.as_str());
        let extracted_path = extract_dir.join(&extracted_folder_name);

        tokio::fs::create_dir_all(install_dir.parent().unwrap()).await?;
        tokio::fs::rename(&extracted_path, &install_dir).await?;

        tokio::fs::remove_file(&archive_path).await.ok();
        tokio::fs::remove_dir_all(&extract_dir).await.ok();

        println!("Installed Node.js v{version} to {}", install_dir.display());

        return Ok(install_dir);
    }

    Err(InstallError::VersionNotFound {
        version: version.to_string(),
        platform: platform.as_str(),
        arch: arch.as_str(),
    })
}

fn extract_archive(archive_path: &Path, dest: &Path, ext: &str) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;

    let decoder: Box<dyn Read> = match ext {
        "tar.xz" => Box::new(xz2::read::XzDecoder::new(file)),
        "tar.gz" => Box::new(flate2::read::GzDecoder::new(file)),
        _ => unreachable!(),
    };

    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;

    Ok(())
}
