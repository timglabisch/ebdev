use crate::platform;
use futures_util::StreamExt;
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

pub async fn install_binary(
    name: &str,
    version: &str,
    url_template: &str,
    binary_path: Option<&str>,
    base_path: &Path,
) -> Result<PathBuf, InstallError> {
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

    let url = resolve_url(url_template, version);
    let archive_type = detect_archive_type(&url);

    println!("Downloading {url}...");

    let response = reqwest::get(&url).await?;

    if !response.status().is_success() {
        return Err(InstallError::NotFound {
            name: name.to_string(),
            version: version.to_string(),
            url,
        });
    }

    let temp_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("binary")
        .join(name)
        .join(".downloads");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let temp_file = temp_dir.join(format!("{name}-v{version}.tmp"));

    // Stream download to temp file
    let mut file = tokio::fs::File::create(&temp_file).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    tokio::fs::create_dir_all(&install_dir).await?;

    let binary_name = binary_path.unwrap_or(name);

    match archive_type {
        ArchiveType::TarGz => {
            println!("Extracting (tar.gz)...");
            let temp_extract = temp_dir.join(format!("{name}-v{version}-extract"));
            tokio::fs::create_dir_all(&temp_extract).await?;

            extract_tar_gz(&temp_file, &temp_extract)?;

            let src = temp_extract.join(binary_name);
            let dest = install_dir.join(name);
            tokio::fs::rename(&src, &dest).await?;

            #[cfg(unix)]
            set_executable(&dest)?;

            tokio::fs::remove_dir_all(&temp_extract).await.ok();
        }
        ArchiveType::TarXz => {
            println!("Extracting (tar.xz)...");
            let temp_extract = temp_dir.join(format!("{name}-v{version}-extract"));
            tokio::fs::create_dir_all(&temp_extract).await?;

            extract_tar_xz(&temp_file, &temp_extract)?;

            let src = temp_extract.join(binary_name);
            let dest = install_dir.join(name);
            tokio::fs::rename(&src, &dest).await?;

            #[cfg(unix)]
            set_executable(&dest)?;

            tokio::fs::remove_dir_all(&temp_extract).await.ok();
        }
        ArchiveType::Plain => {
            let dest = install_dir.join(name);
            tokio::fs::rename(&temp_file, &dest).await?;

            #[cfg(unix)]
            set_executable(&dest)?;
        }
    }

    // Cleanup temp file (may already be moved for Plain)
    tokio::fs::remove_file(&temp_file).await.ok();

    println!("Installed {name} v{version} to {}", install_dir.display());

    Ok(install_dir)
}

fn extract_tar_gz(archive_path: &Path, dest: &Path) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

fn extract_tar_xz(archive_path: &Path, dest: &Path) -> Result<(), std::io::Error> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
