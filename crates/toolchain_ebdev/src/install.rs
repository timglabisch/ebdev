use crate::platform::{Arch, Platform};
use futures_util::StreamExt;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ebdev version {version} not found for {platform}-{arch}")]
    VersionNotFound {
        version: String,
        platform: &'static str,
        arch: &'static str,
    },
}

fn download_url(version: &str, platform: Platform, arch: Arch) -> String {
    format!(
        "https://github.com/timglabisch/ebdev/releases/download/v{version}/ebdev-{platform}-{arch}",
        platform = platform.as_str(),
        arch = arch.as_str(),
    )
}

pub async fn self_update(desired_version: &str) -> Result<PathBuf, InstallError> {
    let current_version = env!("CARGO_PKG_VERSION");
    let exe_path = std::env::current_exe()?;

    if desired_version == current_version {
        return Ok(exe_path);
    }

    let platform = Platform::current();
    let arch = Arch::current();

    let url = download_url(desired_version, platform, arch);
    eprintln!(
        "ebdev self-update: {} -> {} (downloading...)",
        current_version, desired_version
    );

    let response = reqwest::get(&url).await?;

    if !response.status().is_success() {
        return Err(InstallError::VersionNotFound {
            version: desired_version.to_string(),
            platform: platform.as_str(),
            arch: arch.as_str(),
        });
    }

    // Download to a temp file next to the current binary
    let exe_dir = exe_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let temp_path = exe_dir.join(".ebdev-update.tmp");

    let mut file = tokio::fs::File::create(&temp_path).await?;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    // chmod +x
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    // Atomic replace: rename temp file over current binary
    std::fs::rename(&temp_path, &exe_path)?;

    eprintln!("ebdev self-update: updated to v{}", desired_version);

    Ok(exe_path)
}
