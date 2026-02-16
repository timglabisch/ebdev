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

    #[error("ebdev version {version} not found for {platform}-{arch} (url: {url})")]
    VersionNotFound {
        version: String,
        platform: &'static str,
        arch: &'static str,
        url: String,
    },

    #[error("Downloaded binary verification failed: {message}")]
    VerificationFailed { message: String },
}

fn download_url(version: &str, platform: Platform, arch: Arch) -> String {
    format!(
        "https://github.com/timglabisch/ebdev/releases/download/v{version}/ebdev-{platform}-{arch}",
        platform = platform.as_str(),
        arch = arch.as_str(),
    )
}

/// Simple semver comparison: returns true if desired > current
fn is_upgrade(current: &str, desired: &str) -> bool {
    let parse = |v: &str| -> (u64, u64, u64) {
        let parts: Vec<u64> = v.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse(desired) > parse(current)
}

pub async fn self_update(
    desired_version: &str,
    current_version: &str,
) -> Result<PathBuf, InstallError> {
    let exe_path = std::env::current_exe()?;

    if desired_version == current_version {
        return Ok(exe_path);
    }

    let platform = Platform::current();
    let arch = Arch::current();
    let url = download_url(desired_version, platform, arch);

    let direction = if is_upgrade(current_version, desired_version) {
        "upgrade"
    } else {
        "DOWNGRADE"
    };

    eprintln!(
        "ebdev self-update: {current_version} -> {desired_version} ({direction})",
    );
    eprintln!("  binary: {}", exe_path.display());
    eprintln!("  url: {url}");

    let response = reqwest::get(&url).await?;

    if !response.status().is_success() {
        return Err(InstallError::VersionNotFound {
            version: desired_version.to_string(),
            platform: platform.as_str(),
            arch: arch.as_str(),
            url,
        });
    }

    // Download to a temp file next to the current binary
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let temp_path = exe_dir.join(".ebdev-update.tmp");

    let mut file = tokio::fs::File::create(&temp_path).await?;
    let mut stream = response.bytes_stream();
    let mut total_bytes: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total_bytes += chunk.len() as u64;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    if total_bytes < 1024 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(InstallError::VerificationFailed {
            message: format!(
                "downloaded file is only {total_bytes} bytes (expected a binary), url: {url}"
            ),
        });
    }

    // chmod +x
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&temp_path, perms)?;
    }

    // Verify the downloaded binary actually runs
    match std::process::Command::new(&temp_path)
        .arg("--version")
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            eprintln!("  verified: {}", stdout.trim());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(InstallError::VerificationFailed {
                message: format!(
                    "binary exited with {} (stdout: {}, stderr: {})",
                    output.status,
                    stdout.trim(),
                    stderr.trim(),
                ),
            });
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(InstallError::VerificationFailed {
                message: format!("failed to execute downloaded binary: {e}"),
            });
        }
    }

    // Atomic replace: rename temp file over current binary
    std::fs::rename(&temp_path, &exe_path)?;

    eprintln!(
        "ebdev self-update: updated to v{desired_version} ({total_bytes} bytes)"
    );

    Ok(exe_path)
}
