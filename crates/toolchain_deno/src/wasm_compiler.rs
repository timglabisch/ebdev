//! WASM Compiler - Löst .rs → .wasm Kompilierung und .wasm Laden auf

use std::path::{Path, PathBuf};

/// Löst ein WASM Modul aus einem Pfad auf
///
/// - `.wasm` Dateien werden direkt gelesen
/// - `.rs` Dateien werden mit cargo zu `.wasm` kompiliert (mit Caching)
pub async fn resolve_wasm_module(
    path: &Path,
    rust_env: Option<&(PathBuf, PathBuf)>,
) -> Result<Vec<u8>, String> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("wasm") => {
            tokio::fs::read(path)
                .await
                .map_err(|e| format!("Failed to read WASM module '{}': {}", path.display(), e))
        }
        Some("rs") => compile_rs_to_wasm(path, rust_env).await,
        _ => Err(format!(
            "Unsupported file extension for WASM module: {}. Use .wasm or .rs",
            path.display()
        )),
    }
}

/// Kompiliert eine .rs Datei zu .wasm mit Content-Hash-basiertem Caching
async fn compile_rs_to_wasm(
    path: &Path,
    rust_env: Option<&(PathBuf, PathBuf)>,
) -> Result<Vec<u8>, String> {
    // Source lesen
    let source = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("Failed to read source '{}': {}", path.display(), e))?;

    // Content-Hash für Cache
    let hash = simple_hash(&source);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("module");

    // Cache Verzeichnis
    let cache_dir = cache_base_dir().join(format!("wasm-build/{}", hash));
    let cached_wasm = cache_dir.join(format!("target/wasm32-wasip1/release/{}.wasm", stem));

    // Cache Hit?
    if cached_wasm.exists() {
        return tokio::fs::read(&cached_wasm)
            .await
            .map_err(|e| format!("Failed to read cached WASM: {}", e));
    }

    // Cache Miss: Cargo-Projekt generieren
    tokio::fs::create_dir_all(cache_dir.join("src"))
        .await
        .map_err(|e| format!("Failed to create build dir: {}", e))?;

    // Cargo.toml mit ebdev-wasm Dependency
    let guest_crate_path = guest_crate_dir();
    let cargo_toml = if guest_crate_path.exists() {
        format!(
            r#"[package]
name = "{stem}"
version = "0.1.0"
edition = "2021"

[dependencies]
ebdev-wasm = {{ path = "{}" }}

[profile.release]
opt-level = "z"
lto = true
"#,
            guest_crate_path.display()
        )
    } else {
        format!(
            r#"[package]
name = "{stem}"
version = "0.1.0"
edition = "2021"

[profile.release]
opt-level = "z"
lto = true
"#
        )
    };

    tokio::fs::write(cache_dir.join("Cargo.toml"), &cargo_toml)
        .await
        .map_err(|e| format!("Failed to write Cargo.toml: {}", e))?;

    tokio::fs::write(cache_dir.join("src/main.rs"), &source)
        .await
        .map_err(|e| format!("Failed to write source: {}", e))?;

    // Rust environment
    let (cargo_home, rustup_home) = match rust_env {
        Some((cargo, rustup)) => (cargo.clone(), rustup.clone()),
        None => {
            // Fallback: Standard-Pfade
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            (
                PathBuf::from(std::env::var("CARGO_HOME").unwrap_or_else(|_| format!("{}/.cargo", home))),
                PathBuf::from(std::env::var("RUSTUP_HOME").unwrap_or_else(|_| format!("{}/.rustup", home))),
            )
        }
    };

    let cargo_bin = cargo_home.join("bin/cargo");
    let rustup_bin = cargo_home.join("bin/rustup");

    // Ensure wasm32-wasip1 target is installed
    let target_check = tokio::process::Command::new(&rustup_bin)
        .args(["target", "add", "wasm32-wasip1"])
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        .output()
        .await
        .map_err(|e| format!("Failed to add wasm target: {}", e))?;

    if !target_check.status.success() {
        let stderr = String::from_utf8_lossy(&target_check.stderr);
        return Err(format!("Failed to add wasm32-wasip1 target: {}", stderr));
    }

    // Build
    let build_output = tokio::process::Command::new(&cargo_bin)
        .args(["build", "--target", "wasm32-wasip1", "--release"])
        .current_dir(&cache_dir)
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        .output()
        .await
        .map_err(|e| format!("Failed to run cargo build: {}", e))?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return Err(format!("WASM compilation failed:\n{}", stderr));
    }

    // Compiled WASM lesen
    tokio::fs::read(&cached_wasm)
        .await
        .map_err(|e| format!("Failed to read compiled WASM '{}': {}", cached_wasm.display(), e))
}

/// Einfacher Hash für Cache-Keys
fn simple_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Cache Base-Verzeichnis
fn cache_base_dir() -> PathBuf {
    PathBuf::from(".ebdev/cache")
}

/// Pfad zum Guest-Crate Source (im Projekt-Root)
fn guest_crate_dir() -> PathBuf {
    PathBuf::from("crates/ebdev_wasm_guest")
}
