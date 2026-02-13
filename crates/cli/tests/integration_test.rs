use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn ebdev() -> Command {
    let mut cmd = Command::cargo_bin("ebdev").unwrap();
    cmd.env("EBDEV_SKIP_SELF_UPDATE", "1");
    cmd
}

#[test]
fn test_full_integration() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
    pnpm: "9.15.0",
    mutagen: "0.17.6",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // ========================================================================
    // Help & Version
    // ========================================================================
    println!("Testing help...");
    ebdev()
        .current_dir(temp_dir.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("easybill development toolchain"));

    ebdev()
        .current_dir(temp_dir.path())
        .arg("--version")
        .assert()
        .success();

    // ========================================================================
    // Toolchain Install
    // ========================================================================
    println!("Testing toolchain install...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success()
        .stdout(predicate::str::contains("Node.js v22.12.0"))
        .stdout(predicate::str::contains("pnpm 9.15.0"))
        .stdout(predicate::str::contains("Mutagen v0.17.6"));

    assert!(temp_dir.path().join(".ebdev/toolchain/node/v22.12.0/bin/node").exists());
    assert!(temp_dir.path().join(".ebdev/toolchain/pnpm/node_22.12.0/pnpm_9.15.0/bin/pnpm").exists());
    assert!(temp_dir.path().join(".ebdev/toolchain/mutagen/v0.17.6/mutagen").exists());

    // Already installed
    println!("Testing already installed...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already installed"));

    // ========================================================================
    // Run Commands
    // ========================================================================
    println!("Testing run node...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-v"])
        .assert()
        .success()
        .stdout(predicate::str::contains("v22.12.0"));

    println!("Testing run npm...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "npm", "-v"])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"\d+\.\d+\.\d+").unwrap());

    println!("Testing run pnpm...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "pnpm", "-v"])
        .assert()
        .success()
        .stdout(predicate::str::contains("9.15.0"));

    println!("Testing run mutagen...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "mutagen", "version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0.17.6"));

    // ========================================================================
    // Argument Passing
    // ========================================================================
    println!("Testing argument passing...");

    // -h should go to pnpm, not ebdev
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "pnpm", "-h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: pnpm"));

    // Multiple arguments
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log('hello', 'world')"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));

    // Special characters
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log('$USER && echo')"])
        .assert()
        .success()
        .stdout(predicate::str::contains("$USER && echo"));

    // ========================================================================
    // Exit Codes
    // ========================================================================
    println!("Testing exit codes...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "process.exit(0)"])
        .assert()
        .code(0);

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "process.exit(1)"])
        .assert()
        .code(1);

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "process.exit(42)"])
        .assert()
        .code(42);

    // ========================================================================
    // Stdin/Stdout/Stderr
    // ========================================================================
    println!("Testing stdin/stdout/stderr...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node"])
        .write_stdin("console.log('piped')")
        .assert()
        .success()
        .stdout(predicate::str::contains("piped"));

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.error('stderr test')"])
        .assert()
        .success()
        .stderr(predicate::str::contains("stderr test"));

    // ========================================================================
    // Version Overrides
    // ========================================================================
    println!("Testing pnpm version override...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "--pnpm-version", "9.14.0", "pnpm", "-v"])
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success()
        .stdout(predicate::str::contains("9.14.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/pnpm/node_22.12.0/pnpm_9.14.0").exists());

    println!("Testing mutagen version override...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "--mutagen-version", "0.17.5", "mutagen", "version"])
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success()
        .stdout(predicate::str::contains("0.17.5"));

    assert!(temp_dir.path().join(".ebdev/toolchain/mutagen/v0.17.5/mutagen").exists());

    // ========================================================================
    // PATH Environment
    // ========================================================================
    println!("Testing PATH...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log(process.execPath)"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".ebdev/toolchain/node/v22.12.0"));

    // ========================================================================
    // Edge Cases
    // ========================================================================
    println!("Testing edge cases...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "nonexistent-command-xyz"])
        .assert()
        .failure();

    println!("All tests passed!");
}

#[test]
fn test_missing_config() {
    let temp_dir = TempDir::new().unwrap();

    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn test_invalid_config() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join(".ebdev.ts"), "invalid {{{").unwrap();

    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .assert()
        .failure();
}

#[test]
fn test_auto_install_on_run() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // Should auto-install when running
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-v"])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success()
        .stdout(predicate::str::contains("v22.12.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/node/v22.12.0").exists());
}

// =============================================================================
// Self-Update Tests (require network access to GitHub releases)
// =============================================================================

/// Download a specific ebdev release binary from GitHub
fn download_release(version: &str, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let url = format!(
        "https://github.com/timglabisch/ebdev/releases/download/v{}/ebdev-macos-{}",
        version, arch
    );
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "-o", dest.to_str().unwrap(), &url])
        .status()
        .expect("curl failed");
    assert!(status.success(), "Failed to download v{}", version);
    fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Write a .ebdev.ts config with the given ebdev version
fn write_ebdev_config(dir: &Path, ebdev_version: &str) {
    let config = format!(
        r#"import {{ defineConfig }} from "ebdev";

export default defineConfig({{
  toolchain: {{
    ebdev: "{}",
    node: "22.12.0",
  }},
}});
"#,
        ebdev_version
    );
    fs::write(dir.join(".ebdev.ts"), config).unwrap();
}

/// Run the binary expecting a self-update to happen.
/// Returns (stdout, stderr) of the process.
fn run_expecting_self_update(binary: &Path, dir: &Path) -> (String, String) {
    let output = std::process::Command::new(binary)
        .current_dir(dir)
        .args(["toolchain", "info"])
        .output()
        .expect("failed to execute binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        stderr.contains("self-update"),
        "Expected self-update message in stderr, got:\nstdout: {}\nstderr: {}",
        stdout, stderr
    );

    (stdout, stderr)
}

/// Get the --version output of a binary
fn get_version(binary: &Path) -> String {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .expect("failed to execute binary");
    assert!(output.status.success(), "--version failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
#[ignore] // Requires network access to GitHub releases
fn test_self_update_upgrade_and_downgrade() {
    let temp_dir = TempDir::new().unwrap();
    let binary_path = temp_dir.path().join("ebdev");

    // ========================================================================
    // Setup: Download v0.0.3 as starting point
    // ========================================================================
    println!("Downloading v0.0.3...");
    download_release("0.0.3", &binary_path);

    let initial_version = get_version(&binary_path);
    println!("Initial version: {}", initial_version);

    let initial_hash = file_hash(&binary_path);

    // ========================================================================
    // UPGRADE: v0.0.3 → v0.0.5
    // ========================================================================
    println!("Testing upgrade to v0.0.5...");
    write_ebdev_config(temp_dir.path(), "0.0.5");

    let (_stdout, stderr) = run_expecting_self_update(&binary_path, temp_dir.path());
    assert!(
        stderr.contains("0.0.5"),
        "Expected target version 0.0.5 in stderr: {}",
        stderr
    );

    // Binary must have changed
    let upgraded_hash = file_hash(&binary_path);
    assert_ne!(initial_hash, upgraded_hash, "Binary should have changed after upgrade");

    // v0.0.5 has git-tag based version display
    let upgraded_version = get_version(&binary_path);
    println!("After upgrade: {}", upgraded_version);
    assert!(
        upgraded_version.contains("0.0.5"),
        "Expected 0.0.5 in version output, got: {}",
        upgraded_version
    );

    // ========================================================================
    // DOWNGRADE: v0.0.5 → v0.0.3
    // ========================================================================
    println!("Testing downgrade to v0.0.3...");
    write_ebdev_config(temp_dir.path(), "0.0.3");

    let (_stdout, stderr) = run_expecting_self_update(&binary_path, temp_dir.path());
    assert!(
        stderr.contains("0.0.3"),
        "Expected target version 0.0.3 in stderr: {}",
        stderr
    );

    // Binary must have changed again
    let downgraded_hash = file_hash(&binary_path);
    assert_ne!(upgraded_hash, downgraded_hash, "Binary should have changed after downgrade");
    assert_eq!(initial_hash, downgraded_hash, "Downgraded binary should match original v0.0.3");

    // v0.0.3 should no longer show 0.0.5
    let downgraded_version = get_version(&binary_path);
    println!("After downgrade: {}", downgraded_version);
    assert!(
        !downgraded_version.contains("0.0.5"),
        "Should no longer be 0.0.5, got: {}",
        downgraded_version
    );

    println!("Upgrade and downgrade test passed!");
}

fn file_hash(path: &Path) -> u32 {
    let data = fs::read(path).expect("failed to read file");
    crc32fast::hash(&data)
}
