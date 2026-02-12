use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
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
