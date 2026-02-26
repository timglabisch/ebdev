use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use tempfile::TempDir;

use ebdev_toolchain_binary::{install_binary, BinaryEnv};

// =============================================================================
// Test HTTP Server
// =============================================================================

/// Starts a minimal HTTP server that serves files from a HashMap.
/// Returns the address. The server runs until the process exits.
fn start_test_server(files: HashMap<String, Vec<u8>>) -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let files = Arc::new(files);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let files = files.clone();
            std::thread::spawn(move || {
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .ok();
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();

                if let Some(data) = files.get(&path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(data);
                } else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            });
        }
    });

    addr
}

// =============================================================================
// Archive Helpers
// =============================================================================

/// Create a tar.gz archive containing a single file.
fn create_tar_gz(file_name: &str, content: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let tar_data = create_tar(file_name, content);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&tar_data).unwrap();
    encoder.finish().unwrap()
}

/// Create a tar.xz archive containing a single file.
fn create_tar_xz(file_name: &str, content: &[u8]) -> Vec<u8> {
    use xz2::write::XzEncoder;

    let tar_data = create_tar(file_name, content);
    let mut encoder = XzEncoder::new(Vec::new(), 1);
    encoder.write_all(&tar_data).unwrap();
    encoder.finish().unwrap()
}

fn create_tar(file_name: &str, content: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, file_name, content)
        .unwrap();
    builder.into_inner().unwrap()
}

// =============================================================================
// BinaryEnv Tests
// =============================================================================

#[test]
fn test_binary_env_found() {
    let temp_dir = TempDir::new().unwrap();
    let install_dir = temp_dir
        .path()
        .join(".ebdev/toolchain/binary/mytool/v1.0.0");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::write(install_dir.join("mytool"), b"binary content").unwrap();

    let env = BinaryEnv::new(temp_dir.path(), "mytool", "1.0.0").unwrap();

    assert_eq!(env.name(), "mytool");
    assert_eq!(env.version(), "1.0.0");
    assert_eq!(env.bin_path(), install_dir.join("mytool"));
    assert_eq!(env.install_dir(), install_dir);
}

#[test]
fn test_binary_env_not_found() {
    let temp_dir = TempDir::new().unwrap();

    let err = BinaryEnv::new(temp_dir.path(), "nonexistent", "1.0.0").unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "Expected 'not found' in error: {}",
        err
    );
}

#[test]
fn test_binary_env_cache_path_structure() {
    let temp_dir = TempDir::new().unwrap();
    let expected_dir = temp_dir
        .path()
        .join(".ebdev/toolchain/binary/tool/v2.5.0");
    std::fs::create_dir_all(&expected_dir).unwrap();

    let env = BinaryEnv::new(temp_dir.path(), "tool", "2.5.0").unwrap();

    // Verify the path contains "binary" (not "github")
    let path_str = env.install_dir().to_string_lossy();
    assert!(
        path_str.contains("toolchain/binary/"),
        "Cache path should use 'binary': {}",
        path_str
    );
    assert!(
        !path_str.contains("github"),
        "Cache path should not contain 'github': {}",
        path_str
    );
}

// =============================================================================
// Install: Plain Binary
// =============================================================================

#[tokio::test]
async fn test_install_plain_binary() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho hello\n";

    let mut files = HashMap::new();
    files.insert("/mytool-v1.0.0".to_string(), binary_content.to_vec());
    let addr = start_test_server(files);

    let url_template = format!("http://127.0.0.1:{}/mytool-v{{version}}", addr.port());

    let install_dir = install_binary("mytool", "1.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    assert!(install_dir.join("mytool").exists());

    let path_str = install_dir.to_string_lossy();
    assert!(
        path_str.contains("toolchain/binary/mytool/v1.0.0"),
        "Install path should be .ebdev/toolchain/binary/mytool/v1.0.0: {}",
        path_str
    );
}

// =============================================================================
// Install: tar.gz
// =============================================================================

#[tokio::test]
async fn test_install_tar_gz() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho tar.gz\n";
    let archive = create_tar_gz("mytool", binary_content);

    let mut files = HashMap::new();
    files.insert("/mytool-v2.0.0.tar.gz".to_string(), archive);
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/mytool-v{{version}}.tar.gz",
        addr.port()
    );

    let install_dir = install_binary("mytool", "2.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    assert!(install_dir.join("mytool").exists());
    let installed = std::fs::read(install_dir.join("mytool")).unwrap();
    assert_eq!(installed, binary_content);
}

#[tokio::test]
async fn test_install_tgz() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho tgz\n";
    let archive = create_tar_gz("mytool", binary_content);

    let mut files = HashMap::new();
    files.insert("/mytool-v1.0.0.tgz".to_string(), archive);
    let addr = start_test_server(files);

    let url_template = format!("http://127.0.0.1:{}/mytool-v{{version}}.tgz", addr.port());

    let install_dir = install_binary("mytool", "1.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    assert!(install_dir.join("mytool").exists());
    let installed = std::fs::read(install_dir.join("mytool")).unwrap();
    assert_eq!(installed, binary_content);
}

// =============================================================================
// Install: tar.xz
// =============================================================================

#[tokio::test]
async fn test_install_tar_xz() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho tar.xz\n";
    let archive = create_tar_xz("mytool", binary_content);

    let mut files = HashMap::new();
    files.insert("/mytool-v3.0.0.tar.xz".to_string(), archive);
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/mytool-v{{version}}.tar.xz",
        addr.port()
    );

    let install_dir = install_binary("mytool", "3.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    assert!(install_dir.join("mytool").exists());
    let installed = std::fs::read(install_dir.join("mytool")).unwrap();
    assert_eq!(installed, binary_content);
}

// =============================================================================
// Install: Custom binary path inside archive
// =============================================================================

#[tokio::test]
async fn test_install_custom_binary_path() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho custom\n";
    let archive = create_tar_gz("bin/actual-binary", binary_content);

    let mut files = HashMap::new();
    files.insert("/tool-v1.0.0.tar.gz".to_string(), archive);
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/tool-v{{version}}.tar.gz",
        addr.port()
    );

    let install_dir = install_binary(
        "tool",
        "1.0.0",
        &url_template,
        Some("bin/actual-binary"),
        temp_dir.path(),
    )
    .await
    .unwrap();

    // The binary is renamed to the tool name
    assert!(install_dir.join("tool").exists());
    let installed = std::fs::read(install_dir.join("tool")).unwrap();
    assert_eq!(installed, binary_content);
}

// =============================================================================
// Install: Already installed (skip download)
// =============================================================================

#[tokio::test]
async fn test_already_installed() {
    let temp_dir = TempDir::new().unwrap();

    // Pre-create the install directory with existing binary
    let install_dir = temp_dir
        .path()
        .join(".ebdev/toolchain/binary/mytool/v1.0.0");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::write(install_dir.join("mytool"), b"existing content").unwrap();

    // URL is invalid — if it tried to download, it would fail
    let result = install_binary(
        "mytool",
        "1.0.0",
        "http://should-not-be-called.invalid/",
        None,
        temp_dir.path(),
    )
    .await;

    assert!(result.is_ok());

    // Original content is preserved
    let content = std::fs::read(install_dir.join("mytool")).unwrap();
    assert_eq!(content, b"existing content");
}

// =============================================================================
// Install: 404 error
// =============================================================================

#[tokio::test]
async fn test_install_not_found() {
    let temp_dir = TempDir::new().unwrap();

    // Server with no files → everything returns 404
    let addr = start_test_server(HashMap::new());
    let url_template = format!(
        "http://127.0.0.1:{}/nonexistent-v{{version}}",
        addr.port()
    );

    let err = install_binary("nonexistent", "1.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("not found"),
        "Error should mention 'not found': {}",
        err
    );
}

// =============================================================================
// Install: URL template resolves {version}
// =============================================================================

#[tokio::test]
async fn test_url_resolves_version() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"versioned";

    let mut files = HashMap::new();
    files.insert(
        "/releases/download/v5.0.0/tool".to_string(),
        binary_content.to_vec(),
    );
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/releases/download/v{{version}}/tool",
        addr.port()
    );

    let result = install_binary("tool", "5.0.0", &url_template, None, temp_dir.path()).await;
    assert!(result.is_ok(), "Should resolve {{version}} in URL: {:?}", result.err());
}

// =============================================================================
// Install: Executable permissions (Unix)
// =============================================================================

#[cfg(unix)]
#[tokio::test]
async fn test_installed_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho hello\n";

    let mut files = HashMap::new();
    files.insert("/tool-v1.0.0".to_string(), binary_content.to_vec());
    let addr = start_test_server(files);

    let url_template = format!("http://127.0.0.1:{}/tool-v{{version}}", addr.port());

    let install_dir = install_binary("tool", "1.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    let perms = std::fs::metadata(install_dir.join("tool"))
        .unwrap()
        .permissions();
    assert_eq!(perms.mode() & 0o777, 0o755);
}

#[cfg(unix)]
#[tokio::test]
async fn test_tar_gz_binary_is_executable() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho hello\n";
    let archive = create_tar_gz("tool", binary_content);

    let mut files = HashMap::new();
    files.insert("/tool-v1.0.0.tar.gz".to_string(), archive);
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/tool-v{{version}}.tar.gz",
        addr.port()
    );

    let install_dir = install_binary("tool", "1.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    let perms = std::fs::metadata(install_dir.join("tool"))
        .unwrap()
        .permissions();
    assert_eq!(perms.mode() & 0o777, 0o755);
}

// =============================================================================
// Install + BinaryEnv roundtrip
// =============================================================================

#[tokio::test]
async fn test_install_then_binary_env() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"#!/bin/sh\necho roundtrip\n";

    let mut files = HashMap::new();
    files.insert("/tool-v4.0.0".to_string(), binary_content.to_vec());
    let addr = start_test_server(files);

    let url_template = format!("http://127.0.0.1:{}/tool-v{{version}}", addr.port());

    // BinaryEnv should fail before install
    assert!(BinaryEnv::new(temp_dir.path(), "tool", "4.0.0").is_err());

    // Install
    install_binary("tool", "4.0.0", &url_template, None, temp_dir.path())
        .await
        .unwrap();

    // BinaryEnv should succeed after install
    let env = BinaryEnv::new(temp_dir.path(), "tool", "4.0.0").unwrap();
    assert_eq!(env.name(), "tool");
    assert_eq!(env.version(), "4.0.0");
    assert!(env.bin_path().exists());
}

// =============================================================================
// Multiple binaries coexist
// =============================================================================

#[tokio::test]
async fn test_multiple_binaries_coexist() {
    let temp_dir = TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("/tool-a-v1.0.0".to_string(), b"tool-a".to_vec());
    files.insert("/tool-b-v2.0.0".to_string(), b"tool-b".to_vec());
    let addr = start_test_server(files);

    let url_a = format!("http://127.0.0.1:{}/tool-a-v{{version}}", addr.port());
    let url_b = format!("http://127.0.0.1:{}/tool-b-v{{version}}", addr.port());

    install_binary("tool-a", "1.0.0", &url_a, None, temp_dir.path())
        .await
        .unwrap();
    install_binary("tool-b", "2.0.0", &url_b, None, temp_dir.path())
        .await
        .unwrap();

    // Both should be independently accessible
    let env_a = BinaryEnv::new(temp_dir.path(), "tool-a", "1.0.0").unwrap();
    let env_b = BinaryEnv::new(temp_dir.path(), "tool-b", "2.0.0").unwrap();

    assert!(env_a.bin_path().exists());
    assert!(env_b.bin_path().exists());
    assert_ne!(env_a.install_dir(), env_b.install_dir());
}

// =============================================================================
// Multiple versions coexist
// =============================================================================

#[tokio::test]
async fn test_multiple_versions_coexist() {
    let temp_dir = TempDir::new().unwrap();

    let mut files = HashMap::new();
    files.insert("/tool-v1.0.0".to_string(), b"v1".to_vec());
    files.insert("/tool-v2.0.0".to_string(), b"v2".to_vec());
    let addr = start_test_server(files);

    let url = format!("http://127.0.0.1:{}/tool-v{{version}}", addr.port());

    install_binary("tool", "1.0.0", &url, None, temp_dir.path())
        .await
        .unwrap();
    install_binary("tool", "2.0.0", &url, None, temp_dir.path())
        .await
        .unwrap();

    let env_v1 = BinaryEnv::new(temp_dir.path(), "tool", "1.0.0").unwrap();
    let env_v2 = BinaryEnv::new(temp_dir.path(), "tool", "2.0.0").unwrap();

    assert_ne!(env_v1.install_dir(), env_v2.install_dir());

    let content_v1 = std::fs::read(env_v1.bin_path()).unwrap();
    let content_v2 = std::fs::read(env_v2.bin_path()).unwrap();
    assert_eq!(content_v1, b"v1");
    assert_eq!(content_v2, b"v2");
}
