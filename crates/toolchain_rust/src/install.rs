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

    #[error("rustup-init not found for {triple}")]
    NotFound { triple: &'static str },

    #[error("rustup-init failed with status: {0}")]
    RustupInitFailed(std::process::ExitStatus),
}

fn rustup_init_url() -> String {
    let triple = platform::target_triple();
    format!("https://static.rust-lang.org/rustup/dist/{triple}/rustup-init")
}

pub async fn install_rust(version: &str, base_path: &Path) -> Result<PathBuf, InstallError> {
    let install_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("rust")
        .join(format!("v{version}"));

    let cargo_home = install_dir.join("cargo_home");
    let bin_dir = cargo_home.join("bin");

    // Already installed?
    if bin_dir.join("rustc").exists() {
        println!("Rust v{version} already installed");
        return Ok(install_dir);
    }

    let rustup_home = install_dir.join("rustup_home");

    // Download rustup-init
    let url = rustup_init_url();
    println!("Downloading {url}...");

    let response = reqwest::get(&url).await?;
    if !response.status().is_success() {
        return Err(InstallError::NotFound {
            triple: platform::target_triple(),
        });
    }

    let temp_dir = base_path
        .join(".ebdev")
        .join("toolchain")
        .join("rust")
        .join(".downloads");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let init_path = temp_dir.join("rustup-init");
    let mut file = tokio::fs::File::create(&init_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    }
    drop(file);

    // chmod +x
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&init_path, perms)?;
    }

    // Prepare directories
    tokio::fs::create_dir_all(&rustup_home).await?;
    tokio::fs::create_dir_all(&cargo_home).await?;

    // Run rustup-init
    println!("Installing Rust v{version} via rustup...");
    let status = tokio::process::Command::new(&init_path)
        .args([
            "--default-toolchain",
            version,
            "-y",
            "--no-modify-path",
        ])
        .env("RUSTUP_HOME", &rustup_home)
        .env("CARGO_HOME", &cargo_home)
        .status()
        .await?;

    if !status.success() {
        return Err(InstallError::RustupInitFailed(status));
    }

    // Clean up rustup-init
    tokio::fs::remove_file(&init_path).await.ok();

    println!("Installed Rust v{version} to {}", install_dir.display());
    Ok(install_dir)
}
