use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use tempfile::TempDir;

use ebdev_toolchain_binary::{install_binary, InstallBinaryOptions, BinaryEnv};

/// Shorthand for creating InstallBinaryOptions with common defaults.
fn opts<'a>(
    name: &'a str,
    version: &'a str,
    url_template: &'a str,
    binary_path: Option<&'a str>,
    base_path: &'a std::path::Path,
) -> InstallBinaryOptions<'a> {
    InstallBinaryOptions {
        name,
        version,
        url_template,
        binary_path,
        base_path,
        gh_version: None,
    }
}

// =============================================================================
// Test HTTP Server
// =============================================================================

struct TestServer {
    addr: std::net::SocketAddr,
}

impl TestServer {
    /// Start a server that serves files unconditionally (no auth).
    fn open(files: HashMap<String, Vec<u8>>) -> Self {
        Self::start(files, None)
    }

    /// Start a server that requires `Authorization: token <expected>` — returns 404 without it.
    fn authenticated(files: HashMap<String, Vec<u8>>, expected_token: &str) -> Self {
        Self::start(files, Some(expected_token.to_string()))
    }

    fn start(files: HashMap<String, Vec<u8>>, required_token: Option<String>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let files = Arc::new(files);
        let required_token = Arc::new(required_token);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let files = files.clone();
                let required_token = required_token.clone();
                std::thread::spawn(move || {
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .ok();
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();

                    // Check auth if required
                    if let Some(ref expected) = *required_token {
                        let expected_header = format!("token {expected}");
                        let has_auth = request.lines().any(|line| {
                            let line_lower = line.to_ascii_lowercase();
                            if let Some(value) = line_lower.strip_prefix("authorization:") {
                                value.trim() == expected_header.to_ascii_lowercase()
                            } else {
                                false
                            }
                        });
                        if !has_auth {
                            let _ = stream.write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                            return;
                        }
                    }

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

        Self { addr }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// Compat wrapper for existing tests.
fn start_test_server(files: HashMap<String, Vec<u8>>) -> std::net::SocketAddr {
    TestServer::open(files).addr
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

    let install_dir = install_binary(&opts("mytool", "1.0.0", &url_template, None, temp_dir.path()))
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

    let install_dir = install_binary(&opts("mytool", "2.0.0", &url_template, None, temp_dir.path()))
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

    let install_dir = install_binary(&opts("mytool", "1.0.0", &url_template, None, temp_dir.path()))
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

    let install_dir = install_binary(&opts("mytool", "3.0.0", &url_template, None, temp_dir.path()))
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

    let install_dir = install_binary(&opts("tool", "1.0.0", &url_template, Some("bin/actual-binary"), temp_dir.path()))
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
    let result = install_binary(&opts(
        "mytool",
        "1.0.0",
        "http://should-not-be-called.invalid/",
        None,
        temp_dir.path(),
    ))
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

    let err = install_binary(&opts("nonexistent", "1.0.0", &url_template, None, temp_dir.path()))
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

    let result = install_binary(&opts("tool", "5.0.0", &url_template, None, temp_dir.path())).await;
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

    let install_dir = install_binary(&opts("tool", "1.0.0", &url_template, None, temp_dir.path()))
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

    let install_dir = install_binary(&opts("tool", "1.0.0", &url_template, None, temp_dir.path()))
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
    install_binary(&opts("tool", "4.0.0", &url_template, None, temp_dir.path()))
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

    install_binary(&opts("tool-a", "1.0.0", &url_a, None, temp_dir.path()))
        .await
        .unwrap();
    install_binary(&opts("tool-b", "2.0.0", &url_b, None, temp_dir.path()))
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

    install_binary(&opts("tool", "1.0.0", &url, None, temp_dir.path()))
        .await
        .unwrap();
    install_binary(&opts("tool", "2.0.0", &url, None, temp_dir.path()))
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

// =============================================================================
// Install: GitHub-style URL with {version} in path and filename (no {target})
// =============================================================================

#[tokio::test]
async fn test_install_github_release_style_url() {
    let temp_dir = TempDir::new().unwrap();
    let binary_content = b"\x7fELF fake binary";

    let mut files = HashMap::new();
    files.insert(
        "/easybill/mysql_clickhouse_cdc_tool/releases/download/v0.0.2/mysql_clickhouse_schema-v0.0.2-aarch64-unknown-linux-musl".to_string(),
        binary_content.to_vec(),
    );
    let addr = start_test_server(files);

    let url_template = format!(
        "http://127.0.0.1:{}/easybill/mysql_clickhouse_cdc_tool/releases/download/v{{version}}/mysql_clickhouse_schema-v{{version}}-aarch64-unknown-linux-musl",
        addr.port()
    );

    let install_dir = install_binary(&opts(
        "mysql_clickhouse_schema_linux_arm64",
        "0.0.2",
        &url_template,
        None,
        temp_dir.path(),
    ))
    .await
    .unwrap();

    // Binary is renamed to the tool name
    let bin = install_dir.join("mysql_clickhouse_schema_linux_arm64");
    assert!(bin.exists(), "Binary should exist at {}", bin.display());

    let installed = std::fs::read(&bin).unwrap();
    assert_eq!(installed, binary_content);

    // BinaryEnv roundtrip
    let env = BinaryEnv::new(temp_dir.path(), "mysql_clickhouse_schema_linux_arm64", "0.0.2").unwrap();
    assert_eq!(env.name(), "mysql_clickhouse_schema_linux_arm64");
    assert_eq!(env.version(), "0.0.2");
    assert!(env.bin_path().exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(&bin).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o755);
    }
}

// =============================================================================
// Install: Second install is a no-op (already installed)
// =============================================================================

#[tokio::test]
async fn test_install_github_release_already_installed() {
    let temp_dir = TempDir::new().unwrap();

    // Pre-create install directory
    let install_dir = temp_dir
        .path()
        .join(".ebdev/toolchain/binary/mysql_clickhouse_schema_linux_arm64/v0.0.2");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::write(
        install_dir.join("mysql_clickhouse_schema_linux_arm64"),
        b"existing",
    )
    .unwrap();

    // URL would 404 — but install should skip the download
    let result = install_binary(&opts(
        "mysql_clickhouse_schema_linux_arm64",
        "0.0.2",
        "http://should-not-be-called.invalid/v{version}/bin",
        None,
        temp_dir.path(),
    ))
    .await;

    assert!(result.is_ok());
    let content = std::fs::read(
        install_dir.join("mysql_clickhouse_schema_linux_arm64"),
    )
    .unwrap();
    assert_eq!(content, b"existing");
}

// =============================================================================
// gh: prefix — authentication tests
//
// These tests manipulate process-wide env vars (GITHUB_TOKEN, GH_TOKEN)
// and must run sequentially. They are combined into a single test to avoid
// races with parallel test execution.
// =============================================================================

#[tokio::test]
async fn test_gh_prefix_auth() {
    // --- Setup: clear env vars so system `gh` doesn't interfere ---
    // (resolve_github_token checks system gh first; we override via PATH
    //  to ensure only env vars are used)
    let orig_github_token = std::env::var("GITHUB_TOKEN").ok();
    let orig_gh_token = std::env::var("GH_TOKEN").ok();
    std::env::remove_var("GITHUB_TOKEN");
    std::env::remove_var("GH_TOKEN");

    // --- 1. GITHUB_TOKEN sends auth, download succeeds ---
    {
        let temp_dir = TempDir::new().unwrap();
        let binary_content = b"private binary";
        let token = "ghp_test_token_12345";

        let mut files = HashMap::new();
        files.insert(
            "/org/repo/releases/download/v0.0.2/tool".to_string(),
            binary_content.to_vec(),
        );
        let server = TestServer::authenticated(files, token);

        let url_template = format!(
            "gh:http://127.0.0.1:{}/org/repo/releases/download/v{{version}}/tool",
            server.port()
        );

        std::env::set_var("GITHUB_TOKEN", token);

        let result = install_binary(&InstallBinaryOptions {
            name: "private-tool",
            version: "0.0.2",
            url_template: &url_template,
            binary_path: None,
            base_path: temp_dir.path(),
            gh_version: None,
        })
        .await;

        std::env::remove_var("GITHUB_TOKEN");

        let install_dir = result.expect("gh: with valid GITHUB_TOKEN should succeed");
        let bin = install_dir.join("private-tool");
        assert!(bin.exists(), "Binary should be downloaded at {}", bin.display());
        assert_eq!(std::fs::read(&bin).unwrap(), binary_content);
    }

    // --- 2. GH_TOKEN works too (GITHUB_TOKEN unset) ---
    {
        let temp_dir = TempDir::new().unwrap();
        let binary_content = b"gh_token binary";
        let token = "ghp_from_gh_token_var";

        let mut files = HashMap::new();
        files.insert("/download/v2.0.0/bin".to_string(), binary_content.to_vec());
        let server = TestServer::authenticated(files, token);

        let url_template = format!(
            "gh:http://127.0.0.1:{}/download/v{{version}}/bin",
            server.port()
        );

        std::env::remove_var("GITHUB_TOKEN");
        std::env::set_var("GH_TOKEN", token);

        let result = install_binary(&InstallBinaryOptions {
            name: "gh-token-tool",
            version: "2.0.0",
            url_template: &url_template,
            binary_path: None,
            base_path: temp_dir.path(),
            gh_version: None,
        })
        .await;

        std::env::remove_var("GH_TOKEN");

        let install_dir = result.expect("gh: with valid GH_TOKEN should succeed");
        assert!(install_dir.join("gh-token-tool").exists());
        assert_eq!(
            std::fs::read(install_dir.join("gh-token-tool")).unwrap(),
            binary_content
        );
    }

    // --- 3. No token → 404 with auth hint ---
    {
        let temp_dir = TempDir::new().unwrap();

        let mut files = HashMap::new();
        files.insert(
            "/org/repo/releases/download/v1.0.0/tool".to_string(),
            b"secret".to_vec(),
        );
        let server = TestServer::authenticated(files, "ghp_real_secret");

        let url_template = format!(
            "gh:http://127.0.0.1:{}/org/repo/releases/download/v{{version}}/tool",
            server.port()
        );

        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("GH_TOKEN");

        let err = install_binary(&InstallBinaryOptions {
            name: "no-token-tool",
            version: "1.0.0",
            url_template: &url_template,
            binary_path: None,
            base_path: temp_dir.path(),
            gh_version: None,
        })
        .await
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("gh auth login") || msg.contains("GITHUB_TOKEN"),
            "Error should contain auth hint, got: {msg}"
        );
    }

    // --- 4. Wrong token → 404 with auth hint ---
    {
        let temp_dir = TempDir::new().unwrap();

        let mut files = HashMap::new();
        files.insert(
            "/org/repo/releases/download/v1.0.0/tool".to_string(),
            b"secret".to_vec(),
        );
        let server = TestServer::authenticated(files, "ghp_correct_token");

        let url_template = format!(
            "gh:http://127.0.0.1:{}/org/repo/releases/download/v{{version}}/tool",
            server.port()
        );

        std::env::set_var("GITHUB_TOKEN", "ghp_wrong_token");

        let err = install_binary(&InstallBinaryOptions {
            name: "wrong-token-tool",
            version: "1.0.0",
            url_template: &url_template,
            binary_path: None,
            base_path: temp_dir.path(),
            gh_version: None,
        })
        .await
        .unwrap_err();

        std::env::remove_var("GITHUB_TOKEN");

        let msg = err.to_string();
        assert!(
            msg.contains("gh auth login") || msg.contains("GITHUB_TOKEN"),
            "Error should contain auth hint, got: {msg}"
        );
    }

    // --- 5. Without gh: prefix — no auth header sent ---
    {
        let temp_dir = TempDir::new().unwrap();

        let mut files = HashMap::new();
        files.insert(
            "/download/v1.0.0/tool".to_string(),
            b"public binary".to_vec(),
        );
        // Server requires auth, but URL has no gh: prefix
        let server = TestServer::authenticated(files, "ghp_some_token");

        let url_template = format!(
            "http://127.0.0.1:{}/download/v{{version}}/tool",
            server.port()
        );

        // Even with token set, non-gh: URL must NOT send auth
        std::env::set_var("GITHUB_TOKEN", "ghp_some_token");

        let err = install_binary(&opts("no-prefix-tool", "1.0.0", &url_template, None, temp_dir.path()))
            .await
            .unwrap_err();

        std::env::remove_var("GITHUB_TOKEN");

        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "Should fail with 'not found' (no auth sent): {msg}"
        );
        assert!(
            !msg.contains("gh auth login"),
            "Non-gh URL error should not suggest gh auth: {msg}"
        );
    }

    // --- Restore original env ---
    match orig_github_token {
        Some(v) => std::env::set_var("GITHUB_TOKEN", v),
        None => std::env::remove_var("GITHUB_TOKEN"),
    }
    match orig_gh_token {
        Some(v) => std::env::set_var("GH_TOKEN", v),
        None => std::env::remove_var("GH_TOKEN"),
    }
}

/// Real end-to-end test: download from a private GitHub repo via the API.
/// Requires `gh auth login` (uses system gh for token).
/// Run with: cargo test -p ebdev-toolchain-binary --test integration test_gh_private_repo_e2e -- --ignored
#[tokio::test]
#[ignore]
async fn test_gh_private_repo_e2e() {
    let temp_dir = TempDir::new().unwrap();

    let result = install_binary(&InstallBinaryOptions {
        name: "mysql_clickhouse_schema",
        version: "0.0.2",
        url_template: "gh:https://github.com/easybill/mysql_clickhouse_cdc_tool/releases/download/v{version}/mysql_clickhouse_schema-v{version}-aarch64-unknown-linux-musl",
        binary_path: None,
        base_path: temp_dir.path(),
        gh_version: None,
    })
    .await;

    let install_dir = result.expect("Should download from private GitHub repo");
    let binary = install_dir.join("mysql_clickhouse_schema");
    assert!(binary.exists(), "Binary should exist at {}", binary.display());

    let metadata = std::fs::metadata(&binary).unwrap();
    assert!(metadata.len() > 0, "Binary should not be empty");
    println!("Downloaded {} bytes to {}", metadata.len(), binary.display());
}
