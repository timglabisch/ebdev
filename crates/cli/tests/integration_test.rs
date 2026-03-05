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
        .stdout(predicate::str::contains("Development environment tool"));

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
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("9.14.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/pnpm/node_22.12.0/pnpm_9.14.0").exists());

    println!("Testing mutagen version override...");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "--mutagen-version", "0.17.5", "mutagen", "version"])
        .timeout(std::time::Duration::from_secs(30))
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
fn test_types_command() {
    // types braucht keine Config — funktioniert überall
    let temp_dir = TempDir::new().unwrap();

    ebdev()
        .current_dir(temp_dir.path())
        .args(["types"])
        .assert()
        .success()
        .stdout(predicate::str::contains("declare module"))
        .stdout(predicate::str::contains("defineConfig"));
}

#[test]
fn test_types_shows_in_help() {
    ebdev()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("types"))
        .stdout(predicate::str::contains("TypeScript type definitions"));
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
// Rust Toolchain Tests (require network access to download rustup)
// =============================================================================

#[test]
fn test_rust_toolchain() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
    rust: "1.84.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // ========================================================================
    // Toolchain Install
    // ========================================================================
    println!("Testing rust toolchain install...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust v1.84.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0/cargo_home/bin/rustc").exists());
    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0/cargo_home/bin/cargo").exists());
    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0/rustup_home").exists());

    // Already installed
    println!("Testing rust already installed...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust v1.84.0 already installed"));

    // ========================================================================
    // Toolchain Info
    // ========================================================================
    println!("Testing toolchain info shows rust...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust:    1.84.0"));

    // ========================================================================
    // Run Commands
    // ========================================================================
    println!("Testing run rustc...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "rustc", "--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.84.0"));

    println!("Testing run cargo...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "cargo", "--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cargo 1.84.0"));

    // ========================================================================
    // RUSTUP_HOME / CARGO_HOME Environment
    // ========================================================================
    println!("Testing RUSTUP_HOME and CARGO_HOME are set...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log('RUSTUP_HOME=' + process.env.RUSTUP_HOME)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("RUSTUP_HOME="))
        .stdout(predicate::str::contains("rust/v1.84.0/rustup_home"));

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log('CARGO_HOME=' + process.env.CARGO_HOME)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CARGO_HOME="))
        .stdout(predicate::str::contains("rust/v1.84.0/cargo_home"));

    // ========================================================================
    // PATH contains rust bin dir
    // ========================================================================
    println!("Testing PATH contains rust bin dir...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log(process.env.PATH)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust/v1.84.0/cargo_home/bin"));

    println!("All rust tests passed!");
}

#[test]
fn test_rust_version_override() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
    rust: "1.84.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    println!("Testing rust version override...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "--rust-version", "1.83.0", "rustc", "--version"])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.83.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.83.0/cargo_home/bin/rustc").exists());

    println!("Rust version override test passed!");
}

#[test]
fn test_rust_auto_install_on_run() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
    rust: "1.84.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // Should auto-install rust when running
    println!("Testing rust auto-install on run...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "rustc", "--version"])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.84.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0").exists());

    println!("Rust auto-install test passed!");
}

#[test]
fn test_rust_optional_not_configured() {
    let temp_dir = TempDir::new().unwrap();

    // Config without rust - should still work fine
    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // toolchain info should not show Rust
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Rust").not());

    // run should work without rust
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-v"])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success()
        .stdout(predicate::str::contains("v22.12.0"));
}

/// Write a .ebdev.ts config with the given rust version
fn write_rust_config(dir: &Path, rust_version: &str) {
    let config = format!(
        r#"import {{ defineConfig }} from "ebdev";

export default defineConfig({{
  toolchain: {{
    ebdev: "0.1.0",
    node: "22.12.0",
    rust: "{}",
  }},
}});
"#,
        rust_version
    );
    fs::write(dir.join(".ebdev.ts"), config).unwrap();
}

#[test]
fn test_rust_upgrade_and_downgrade() {
    let temp_dir = TempDir::new().unwrap();

    // ========================================================================
    // Install 1.83.0
    // ========================================================================
    println!("Installing Rust 1.83.0...");
    write_rust_config(temp_dir.path(), "1.83.0");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "rustc", "--version"])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.83.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.83.0/cargo_home/bin/rustc").exists());

    // ========================================================================
    // UPGRADE: 1.83.0 → 1.84.0
    // ========================================================================
    println!("Upgrading to Rust 1.84.0...");
    write_rust_config(temp_dir.path(), "1.84.0");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "rustc", "--version"])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.84.0"));

    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0/cargo_home/bin/rustc").exists());

    // Verify RUSTUP_HOME/CARGO_HOME point to the new version
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log(process.env.RUSTUP_HOME)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust/v1.84.0/rustup_home"));

    // ========================================================================
    // DOWNGRADE: 1.84.0 → 1.83.0
    // ========================================================================
    println!("Downgrading back to Rust 1.83.0...");
    write_rust_config(temp_dir.path(), "1.83.0");

    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "rustc", "--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rustc 1.83.0"));

    // Verify RUSTUP_HOME/CARGO_HOME point back to the old version
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log(process.env.RUSTUP_HOME)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rust/v1.83.0/rustup_home"));

    // ========================================================================
    // Both versions coexist
    // ========================================================================
    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.83.0/cargo_home/bin/rustc").exists());
    assert!(temp_dir.path().join(".ebdev/toolchain/rust/v1.84.0/cargo_home/bin/rustc").exists());

    println!("Rust upgrade and downgrade test passed!");
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

// =============================================================================
// Shell Completion Tests
// =============================================================================

/// Write a .ebdev.ts config with tasks (including defineTask with args)
fn write_config_with_tasks(dir: &Path) {
    let config = r#"import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
  },
});

export async function build() {
  console.log("building");
}

export async function test_unit() {
  console.log("testing");
}

export const deploy = defineTask({
  description: "Deploy to environment",
  args: {
    target: arg.string("Target environment").required().complete(async () => {
      return ["staging", "production", "dev"];
    }),
    dryRun: arg.boolean("Simulate without changes"),
  },
  async run({ target, dryRun }) {
    console.log(`deploying to ${target}, dry=${dryRun}`);
  },
});

export const greet = defineTask({
  description: "Greet someone",
  args: {
    name: arg.string("Name to greet").required().complete(() => ["Alice", "Bob", "Charlie"]),
    loud: arg.boolean("Shout the greeting"),
  },
  async run({ name, loud }) {
    const msg = loud ? name.toUpperCase() : name;
    console.log(msg);
  },
});
"#;
    fs::write(dir.join(".ebdev.ts"), config).unwrap();
}

#[test]
fn test_completions_no_args() {
    ebdev()
        .args(["completions"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ebdev completions zsh"));
}

#[test]
fn test_completions_zsh_script() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef ebdev"))
        .stdout(predicate::str::contains("./ebdev"))
        .stdout(predicate::str::contains("compdef _clap_dynamic_completer_ebdev ./ebdev"))
        // Must NOT contain absolute binary path
        .stdout(predicate::str::contains("target/debug/ebdev").not());
}

#[test]
fn test_completions_bash_script() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("./ebdev"))
        .stdout(predicate::str::contains("complete"))
        .stdout(predicate::str::contains("target/debug/ebdev").not());
}

#[test]
fn test_completion_subcommands() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // Complete subcommands: "ebdev <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "1")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("task:"))
        .stdout(predicate::str::contains("tasks:"))
        .stdout(predicate::str::contains("completions:"));
}

#[test]
fn test_completion_task_names() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // Complete task names: "ebdev task <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "2")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("build"))
        .stdout(predicate::str::contains("test_unit"))
        .stdout(predicate::str::contains("deploy:Deploy to environment"));
}

#[test]
fn test_completion_task_names_with_prefix() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // Complete with prefix: "ebdev task de<TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "2")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "de"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy"))
        // Should not include non-matching tasks
        .stdout(predicate::str::contains("build\n").not());
}

#[test]
fn test_completion_task_args() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // Complete task args: "ebdev task deploy -- <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "deploy", "--", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target:Target environment"))
        .stdout(predicate::str::contains("--dry-run:Simulate without changes"));
}

#[test]
fn test_completion_task_args_with_prefix() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // Complete with prefix: "ebdev task deploy -- --t<TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "deploy", "--", "--t"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target"))
        .stdout(predicate::str::contains("--dry-run").not());
}

#[test]
fn test_completion_task_args_no_args_defined() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "build" has no defineTask args — should show empty or only help
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "build", "--", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target").not());
}

#[test]
fn test_completion_no_ebdev_ts() {
    let temp_dir = TempDir::new().unwrap();
    // No .ebdev.ts — completions should still work (just no task names)

    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "2")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", ""])
        .assert()
        .success()
        // Should still show flags even without tasks
        .stdout(predicate::str::contains("--no-tui"));
}

// =============================================================================
// complete-arg Subcommand Tests
// =============================================================================

#[test]
fn test_complete_arg_returns_values() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // ebdev complete-arg greet name → should return JSON array
    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-arg", "greet", "name"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob"))
        .stdout(predicate::str::contains("Charlie"));
}

#[test]
fn test_complete_arg_no_complete_fn() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({
  toolchain: { ebdev: "0.1.0", node: "22.12.0" },
});

export const greet = defineTask({
  args: { name: arg.string("Name").required() },
  async run({ name }) {},
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // No .complete() → should return empty array
    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-arg", "greet", "name"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[]"));
}

#[test]
fn test_complete_arg_nonexistent_task() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-arg", "nonexistent", "foo"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[]"));
}

#[test]
fn test_complete_arg_no_ebdev_ts() {
    let temp_dir = TempDir::new().unwrap();
    // No .ebdev.ts file

    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-arg", "greet", "name"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[]"));
}

// =============================================================================
// Value Completion Tests (space-separated and equals-separated)
// =============================================================================

#[test]
fn test_completion_value_space_separated() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task greet -- --name <TAB>" → should return bare values
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--name", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob"))
        .stdout(predicate::str::contains("Charlie"))
        // Must NOT contain --name= prefix (space mode returns bare values)
        .stdout(predicate::str::contains("--name=").not());
}

#[test]
fn test_completion_value_space_with_prefix() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task greet -- --name A<TAB>" → should return only "Alice"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--name", "A"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob").not())
        .stdout(predicate::str::contains("--name=").not());
}

#[test]
fn test_completion_value_equals_separated() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task greet -- --name=<TAB>" → should return --name=value candidates
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--name="])
        .assert()
        .success()
        .stdout(predicate::str::contains("--name=Alice"))
        .stdout(predicate::str::contains("--name=Bob"))
        .stdout(predicate::str::contains("--name=Charlie"));
}

#[test]
fn test_completion_value_equals_with_prefix() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task deploy -- --target=st<TAB>" → should return --target=staging
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "deploy", "--", "--target=st"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--target=staging"))
        .stdout(predicate::str::contains("--target=production").not());
}

#[test]
fn test_completion_value_no_complete_fn() {
    let temp_dir = TempDir::new().unwrap();

    // Task with args but no .complete()
    let config = r#"import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({
  toolchain: { ebdev: "0.1.0", node: "22.12.0" },
});

export const greet = defineTask({
  args: { name: arg.string("Name").required() },
  async run({ name }) {},
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // "ebdev task greet -- --name <TAB>" → should return no values
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--name", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice").not());
}

#[test]
fn test_completion_boolean_flag_no_value() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // After a boolean flag, should still show flag names (not values)
    // "ebdev task greet -- --loud <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--loud", ""])
        .assert()
        .success()
        // Should show flag names, not values
        .stdout(predicate::str::contains("--name"))
        .stdout(predicate::str::contains("Alice").not());
}

// =============================================================================
// --flag=value Parsing Tests (runtime, not just completion)
// =============================================================================

#[test]
fn test_task_equals_syntax_runs() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // ebdev task greet -- --name=Alice should work
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "greet", "--", "--name=Alice"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success();
}

#[test]
fn test_task_equals_syntax_with_boolean() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // ebdev task greet -- --name=Alice --loud should work
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "greet", "--", "--name=Alice", "--loud"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success();
}

// =============================================================================
// Completion Edge Cases (Integration)
// =============================================================================

/// Config with oneOf + .complete() to test merging/dedup of static + dynamic values
fn write_config_with_oneof_complete(dir: &Path) {
    let config = r#"import { defineConfig, defineTask, arg } from "ebdev";

export default defineConfig({
  toolchain: { ebdev: "0.1.0", node: "22.12.0" },
});

export const release = defineTask({
  description: "Release to environment",
  args: {
    env: arg.oneOf(["staging", "prod"], "Environment")
      .complete(() => ["staging", "prod", "dev-preview"]),
  },
  async run({ env }) {
    console.log(`Releasing to ${env}`);
  },
});
"#;
    fs::write(dir.join(".ebdev.ts"), config).unwrap();
}

#[test]
fn test_completion_oneof_with_complete_merges() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_oneof_complete(temp_dir.path());

    // "ebdev task release -- --env <TAB>" → should show merged values:
    // "staging", "prod" (from choices) + "dev-preview" (from complete, deduplicated)
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "release", "--", "--env", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("staging"))
        .stdout(predicate::str::contains("prod"))
        .stdout(predicate::str::contains("dev-preview"));
}

#[test]
fn test_completion_oneof_with_complete_filters() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_oneof_complete(temp_dir.path());

    // "ebdev task release -- --env dev<TAB>" → should show only "dev-preview"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "5")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "release", "--", "--env", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dev-preview"))
        .stdout(predicate::str::contains("staging").not())
        .stdout(predicate::str::contains("prod\n").not());
}

#[test]
fn test_completion_after_value_shows_flags() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task greet -- --name Alice <TAB>" → should show remaining flags, not values
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "6")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--name", "Alice", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("--loud"))
        // Should NOT show values — "Alice" is not a flag
        .stdout(predicate::str::contains("Bob").not());
}

#[test]
fn test_completion_after_boolean_then_new_flag() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path());

    // "ebdev task greet -- --loud --name <TAB>" → value completion for --name
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "6")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "greet", "--", "--loud", "--name", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Bob"))
        .stdout(predicate::str::contains("Charlie"))
        .stdout(predicate::str::contains("--name=").not());
}

fn file_hash(path: &Path) -> u32 {
    let data = fs::read(path).expect("failed to read file");
    crc32fast::hash(&data)
}

// =============================================================================
// Feature Flag Tests
// =============================================================================

/// Write a .ebdev.ts config with feature flags
fn write_config_with_flags(dir: &Path) {
    // Create fake node toolchain dir so ensure_toolchain skips download
    fs::create_dir_all(dir.join(".ebdev/toolchain/node/v22.12.0/bin")).unwrap();

    let config = r#"import { defineConfig, defineTask, flag, arg } from "ebdev";

const config = defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
  },
  flags: {
    search: flag("Elasticsearch").config({
      engine: arg.oneOf(["elasticsearch", "meilisearch"] as const, "Search engine").default("elasticsearch"),
      index: arg.string("Index name").default("main").complete(async () => ["main", "products", "orders"]),
    }),
    clickhouse: flag("ClickHouse Analytics").default(true),
    mailhog: flag("Mail Catcher").default(false),
    tidb: flag("TiDB Cluster").default(true),
    analytics: flag("Analytics").default(true).requires("tidb"),
  },
});
export default config;

export async function dev() {
  const services: string[] = ["php", "nginx"];
  if (config.flags.search) services.push(config.flags.search.engine);
  if (config.flags.clickhouse) services.push("clickhouse");
  console.log("services:" + services.join(","));
  console.log("search:" + JSON.stringify(config.flags.search));
  console.log("clickhouse:" + JSON.stringify(config.flags.clickhouse));
  console.log("mailhog:" + JSON.stringify(config.flags.mailhog));
}

export const build = defineTask({
  description: "Build with selected services",
  flags: config.pick("search", "clickhouse"),
  async run(_args: Record<string, never>, flags: { search: { engine: string; index: string } | false; clickhouse: boolean }) {
    console.log("build:search:" + JSON.stringify(flags.search));
    console.log("build:clickhouse:" + JSON.stringify(flags.clickhouse));
  },
});
"#;
    fs::write(dir.join(".ebdev.ts"), config).unwrap();
}

// =============================================================================
// Binary Toolchain Tests
// =============================================================================

/// Start a minimal HTTP test server that serves files from a HashMap.
fn start_test_server(files: std::collections::HashMap<String, Vec<u8>>) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    use std::sync::Arc;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let files = Arc::new(files);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let files = files.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { return; }
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.lines().next()
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
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            });
        }
    });

    addr
}

#[test]
fn test_binary_toolchain() {
    let temp_dir = TempDir::new().unwrap();

    // Start test HTTP server with a shell script as "binary"
    let binary_content = b"#!/bin/sh\necho \"mytool v1.0.0 working\"\n";
    let mut files = std::collections::HashMap::new();
    files.insert("/mytool-v1.0.0".to_string(), binary_content.to_vec());
    let addr = start_test_server(files);

    let config = format!(
        r#"import {{ defineConfig }} from "ebdev";

export default defineConfig({{
  toolchain: {{
    ebdev: "0.1.0",
    node: "22.12.0",
    binary: {{
      mytool: {{
        version: "1.0.0",
        url: "http://127.0.0.1:{port}/mytool-v{{version}}",
      }},
    }},
  }},
}});
"#,
        port = addr.port()
    );
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // ========================================================================
    // Toolchain Info
    // ========================================================================
    println!("Testing toolchain info shows binary...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mytool"))
        .stdout(predicate::str::contains("1.0.0"))
        .stdout(predicate::str::contains("(binary)"));

    // ========================================================================
    // Toolchain Install
    // ========================================================================
    println!("Testing toolchain install with binary...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success();

    // Verify binary installed at correct path
    assert!(
        temp_dir.path().join(".ebdev/toolchain/binary/mytool/v1.0.0/mytool").exists(),
        "Binary should be installed at .ebdev/toolchain/binary/mytool/v1.0.0/mytool"
    );

    // Verify NOT at old github path
    assert!(
        !temp_dir.path().join(".ebdev/toolchain/github").exists(),
        "Should not use old 'github' cache path"
    );

    // ========================================================================
    // Already Installed
    // ========================================================================
    println!("Testing binary already installed...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already installed"));

    // ========================================================================
    // Run: binary is in PATH
    // ========================================================================
    println!("Testing run with binary toolchain...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "mytool"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mytool v1.0.0 working"));

    // ========================================================================
    // PATH contains binary toolchain dir
    // ========================================================================
    println!("Testing PATH includes binary toolchain...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "node", "-e", "console.log(process.env.PATH)"])
        .assert()
        .success()
        .stdout(predicate::str::contains("toolchain/binary/mytool/v1.0.0"));

    println!("All binary toolchain tests passed!");
}

#[test]
fn test_binary_toolchain_not_configured() {
    let temp_dir = TempDir::new().unwrap();

    // Config without binary — should not show binary in info
    let config = r#"import { defineConfig } from "ebdev";

export default defineConfig({
  toolchain: {
    ebdev: "0.1.0",
    node: "22.12.0",
  },
});
"#;
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(binary)").not());
}

#[test]
fn test_binary_toolchain_tar_gz() {
    use std::io::Write;

    let temp_dir = TempDir::new().unwrap();

    // Create a tar.gz archive with a shell script
    let binary_content = b"#!/bin/sh\necho \"archived-tool v2.0.0\"\n";
    let tar_data = {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(binary_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, "archived-tool", &binary_content[..]).unwrap();
        builder.into_inner().unwrap()
    };
    let gz_data = {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_data).unwrap();
        encoder.finish().unwrap()
    };

    let mut files = std::collections::HashMap::new();
    files.insert("/archived-tool-v2.0.0.tar.gz".to_string(), gz_data);
    let addr = start_test_server(files);

    let config = format!(
        r#"import {{ defineConfig }} from "ebdev";

export default defineConfig({{
  toolchain: {{
    ebdev: "0.1.0",
    node: "22.12.0",
    binary: {{
      "archived-tool": {{
        version: "2.0.0",
        url: "http://127.0.0.1:{port}/archived-tool-v{{version}}.tar.gz",
      }},
    }},
  }},
}});
"#,
        port = addr.port()
    );
    fs::write(temp_dir.path().join(".ebdev.ts"), config).unwrap();

    // Install
    println!("Testing tar.gz binary toolchain install...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["toolchain", "install"])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .success();

    assert!(
        temp_dir.path().join(".ebdev/toolchain/binary/archived-tool/v2.0.0/archived-tool").exists()
    );

    // Run
    println!("Testing tar.gz binary toolchain run...");
    ebdev()
        .current_dir(temp_dir.path())
        .args(["run", "archived-tool"])
        .assert()
        .success()
        .stdout(predicate::str::contains("archived-tool v2.0.0"));

    println!("tar.gz binary toolchain tests passed!");
}

// =============================================================================
// Feature Flag Tests — `ebdev flags` and `ebdev flag`
// =============================================================================

#[test]
fn test_flags_list() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["flags"])
        .assert()
        .success()
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("Elasticsearch"))
        .stdout(predicate::str::contains("clickhouse"))
        .stdout(predicate::str::contains("mailhog"))
        .stdout(predicate::str::contains("ON"))
        .stdout(predicate::str::contains("OFF"));
}

#[test]
fn test_flags_list_json() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    let output = ebdev()
        .current_dir(temp_dir.path())
        .args(["flags", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed.is_array(), "flags --json should output a JSON array");

    let flags = parsed.as_array().unwrap();
    let names: Vec<&str> = flags.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search"));
    assert!(names.contains(&"clickhouse"));
    assert!(names.contains(&"mailhog"));

    // Check active values (active = saved value or metadata default)
    let search = flags.iter().find(|f| f["name"] == "search").unwrap();
    // Config flag with no saved state: active = default (true)
    assert_eq!(search["active"], true, "search active should be true (default, config object resolved at runtime)");
    // Config fields should be listed in the flag's config array
    assert!(search["config"].is_array(), "search should have config fields");

    let mailhog = flags.iter().find(|f| f["name"] == "mailhog").unwrap();
    assert_eq!(mailhog["active"], false, "mailhog should be OFF by default");
}

#[test]
fn test_flag_set_on_off() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Turn mailhog ON
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog", "on"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog = ON"));

    // Verify persistence
    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();
    assert_eq!(saved["mailhog"], true);

    // Turn mailhog OFF
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog", "off"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog = OFF"));
}

#[test]
fn test_flag_toggle() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // mailhog default is false, toggle should turn ON
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog = ON"));

    // Toggle again should turn OFF
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog = OFF"));
}

#[test]
fn test_flag_set_config_field() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Set search.engine to meilisearch
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.engine", "meilisearch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("search.engine = meilisearch"));

    // Verify persistence
    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();
    assert_eq!(saved["search"]["engine"], "meilisearch");
}

#[test]
fn test_flag_config_field_invalid_choice() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Invalid choice for engine
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.engine", "solr"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid value"))
        .stderr(predicate::str::contains("Must be one of"));
}

#[test]
fn test_flag_unknown_flag() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "nonexistent", "on"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown flag"));
}

#[test]
fn test_flag_unknown_field() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.nonexistent", "value"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown field"));
}

#[test]
fn test_flag_boolean_no_config_field() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // clickhouse is boolean, not config — can't set fields
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "clickhouse.engine", "x"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("boolean flag"));
}

#[test]
fn test_flag_dependency_auto_enable() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // First disable tidb
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "tidb", "off"])
        .assert()
        .success();

    // Now enable analytics (requires tidb) → should auto-enable tidb
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "analytics", "on"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tidb = ON (required by analytics)"))
        .stdout(predicate::str::contains("analytics = ON"));
}

#[test]
fn test_flag_dependency_auto_disable() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Disable tidb → should auto-disable analytics (requires tidb)
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "tidb", "off"])
        .assert()
        .success()
        .stdout(predicate::str::contains("analytics = OFF (requires tidb)"))
        .stdout(predicate::str::contains("tidb = OFF"));
}

#[test]
fn test_flag_persistence_only_diffs() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Set mailhog ON (differs from default false)
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog", "on"])
        .assert()
        .success();

    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();

    // Should only contain mailhog (the only diff from defaults)
    assert_eq!(saved["mailhog"], true);
    // clickhouse should NOT be in saved (it's at default)
    assert!(saved.get("clickhouse").is_none(), "clickhouse should not be in saved (at default)");
}

#[test]
fn test_flag_no_flags_defined() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_tasks(temp_dir.path()); // uses existing helper without flags

    ebdev()
        .current_dir(temp_dir.path())
        .args(["flags"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No feature flags defined"));
}

// =============================================================================
// Feature Flag Tests — Task with --with / --without
// =============================================================================

#[test]
fn test_task_with_flag_override() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // mailhog is OFF by default, --with should enable it for this run
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--with", "mailhog"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog:true"));
}

#[test]
fn test_task_without_flag_override() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // clickhouse is ON by default, --without should disable it for this run
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--without", "clickhouse"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("clickhouse:false"));
}

#[test]
fn test_task_without_config_flag() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // search is ON by default (config flag), --without should set it to false
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--without", "search"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("search:false"));
}

#[test]
fn test_task_with_config_override_colon_syntax() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // --with search:engine=meilisearch
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--with", "search:engine=meilisearch"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"engine\":\"meilisearch\""));
}

#[test]
fn test_task_with_config_override_dot_syntax() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // --with search.engine=meilisearch
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--with", "search.engine=meilisearch"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"engine\":\"meilisearch\""));
}

#[test]
fn test_task_with_picked_flags_and_override() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // build task picks search + clickhouse, override clickhouse to off
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "build", "--without", "clickhouse"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("build:clickhouse:false"));
}

// =============================================================================
// Feature Flag Tests — Completions
// =============================================================================

#[test]
fn test_completion_flag_names() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev flag <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "2")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "flag", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("clickhouse"))
        .stdout(predicate::str::contains("mailhog"))
        // Config flag should also show dotted fields
        .stdout(predicate::str::contains("search.engine"))
        .stdout(predicate::str::contains("search.index"));
}

#[test]
fn test_completion_with_flags_on_task() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev task dev --with <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "dev", "--with", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("clickhouse"))
        .stdout(predicate::str::contains("mailhog"));
}

#[test]
fn test_completion_without_flags_on_task() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev task dev --without <TAB>"
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "4")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "task", "dev", "--without", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("clickhouse"));
}

#[test]
fn test_complete_flag_subcommand() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // ebdev complete-flag search index → should return dynamic completions
    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-flag", "search", "index"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("products"))
        .stdout(predicate::str::contains("orders"));
}

#[test]
fn test_complete_flag_subcommand_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // ebdev complete-flag nonexistent field → should return empty
    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-flag", "nonexistent", "field"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[]"));
}

#[test]
fn test_complete_flag_no_ebdev_ts() {
    let temp_dir = TempDir::new().unwrap();
    // No .ebdev.ts

    ebdev()
        .current_dir(temp_dir.path())
        .args(["complete-flag", "search", "engine"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[]"));
}

#[test]
fn test_completion_flag_value_boolean() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev flag clickhouse <TAB>" → on, off
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "3")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "flag", "clickhouse", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("on"))
        .stdout(predicate::str::contains("off"));
}

#[test]
fn test_completion_flag_value_config_field_choices() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev flag search.engine <TAB>" → elasticsearch, meilisearch
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "3")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "flag", "search.engine", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("elasticsearch"))
        .stdout(predicate::str::contains("meilisearch"));
}

#[test]
fn test_completion_flag_value_config_field_dynamic() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev flag search.index <TAB>" → dynamic completions (main, products, orders)
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "3")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "flag", "search.index", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("products"))
        .stdout(predicate::str::contains("orders"));
}

#[test]
fn test_completion_flag_value_config_flag_on_off() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev flag search <TAB>" (config flag without dotted field) → on, off
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "3")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", "flag", "search", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("on"))
        .stdout(predicate::str::contains("off"));
}

// =============================================================================
// Feature Flag Tests — Subcommands
// =============================================================================

#[test]
fn test_flags_subcommand_shows_in_help() {
    ebdev()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("flags"))
        .stdout(predicate::str::contains("flag"));
}

#[test]
fn test_completion_subcommands_include_flags() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // "ebdev <TAB>" should include flags/flag subcommands
    ebdev()
        .current_dir(temp_dir.path())
        .env("COMPLETE", "zsh")
        .env("_CLAP_COMPLETE_INDEX", "1")
        .env("_CLAP_IFS", "\n")
        .args(["--", "ebdev", ""])
        .assert()
        .success()
        .stdout(predicate::str::contains("flags:"))
        .stdout(predicate::str::contains("flag:"));
}

// =============================================================================
// Feature Flag Tests — Additional Coverage
// =============================================================================

#[test]
fn test_flag_toggle_config_flag() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // search is a config flag, default ON. Toggle should turn it OFF.
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search"])
        .assert()
        .success()
        .stdout(predicate::str::contains("search = OFF"));

    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();
    assert_eq!(saved["search"], false);

    // Toggle again should turn it back ON (with default config values)
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search"])
        .assert()
        .success()
        .stdout(predicate::str::contains("search = ON"));

    // After toggling ON again, search should be removed from saved (back to default)
    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();
    assert!(saved.get("search").is_none(), "search should be removed from saved (back to default ON)");
}

#[test]
fn test_task_with_and_without_combined() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // mailhog OFF by default → --with enables, clickhouse ON by default → --without disables
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--with", "mailhog", "--without", "clickhouse"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("mailhog:true"))
        .stdout(predicate::str::contains("clickhouse:false"));
}

#[test]
fn test_task_with_config_flag_enable() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // First persist search=off
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search", "off"])
        .assert()
        .success();

    // Now --with search should re-enable it for this run
    ebdev()
        .current_dir(temp_dir.path())
        .args(["task", "dev", "--with", "search"])
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        // search should be truthy (not false)
        .stdout(predicate::str::contains("search:false").not());
}

#[test]
fn test_flag_set_string_field_without_choices() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // search.index is a string field without choices — any value should be accepted
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.index", "products"])
        .assert()
        .success()
        .stdout(predicate::str::contains("search.index = products"));

    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp_dir.path().join(".ebdev/flags.json")).unwrap()
    ).unwrap();
    assert_eq!(saved["search"]["index"], "products");
}

#[test]
fn test_flags_json_with_saved_state() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Save some state
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "mailhog", "on"])
        .assert()
        .success();
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.engine", "meilisearch"])
        .assert()
        .success();

    let output = ebdev()
        .current_dir(temp_dir.path())
        .args(["flags", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let flags = parsed.as_array().unwrap();

    // mailhog should now be active=true (saved: true, overriding default false)
    let mailhog = flags.iter().find(|f| f["name"] == "mailhog").unwrap();
    assert_eq!(mailhog["active"], true, "mailhog should be ON after flag set");

    // search should have saved config with engine=meilisearch
    let search = flags.iter().find(|f| f["name"] == "search").unwrap();
    assert_eq!(search["active"]["engine"], "meilisearch",
        "search.engine should be meilisearch after flag set");
}

#[test]
fn test_flag_invalid_boolean_value() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "clickhouse", "maybe"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid value"));
}

#[test]
fn test_flag_config_field_no_value() {
    let temp_dir = TempDir::new().unwrap();
    write_config_with_flags(temp_dir.path());

    // Setting a config field without a value should show usage
    ebdev()
        .current_dir(temp_dir.path())
        .args(["flag", "search.engine"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"));
}
