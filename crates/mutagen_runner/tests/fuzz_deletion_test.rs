//! Aggressive fuzz test that tries to reproduce mutagen deleting host directories.
//!
//! This test creates real mutagen sessions and exercises various stress scenarios
//! to provoke directory deletion on the host (alpha) side.
//!
//! Run with: cargo test -p ebdev-mutagen-runner --test fuzz_deletion_test -- --ignored --nocapture
//!
//! WARNING: This test creates real mutagen sessions and Docker containers.
//! It cleans up after itself but may leave artifacts if interrupted.

use ebdev_mutagen_runner::{
    reconcile_sessions, state::DesiredSession, MutagenBackend, RealMutagen, SyncMode,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const TEST_IMAGE: &str = "alpine:latest";
const CONTAINER_PREFIX: &str = "ebdev-fuzz-test";

// ============================================================================
// Test Infrastructure
// ============================================================================

fn find_mutagen() -> Option<PathBuf> {
    let paths = [
        PathBuf::from("/usr/local/bin/mutagen"),
        PathBuf::from("/opt/homebrew/bin/mutagen"),
        dirs::home_dir()
            .map(|h| h.join(".ebdev/toolchains/mutagen/0.18.1/mutagen"))
            .unwrap_or_default(),
    ];

    for path in paths {
        if path.exists() {
            return Some(path);
        }
    }

    which::which("mutagen").ok()
}

fn create_container(name: &str) -> Result<String, String> {
    let container_name = format!("{}-{}", CONTAINER_PREFIX, name);

    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output();

    let output = Command::new("docker")
        .args([
            "run", "-d", "--name", &container_name, TEST_IMAGE, "sleep", "3600",
        ])
        .output()
        .map_err(|e| format!("Failed to create container: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Create sync directory
    let output = Command::new("docker")
        .args(["exec", &container_name, "mkdir", "-p", "/sync"])
        .output()
        .map_err(|e| format!("mkdir failed: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "mkdir failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(container_name)
}

fn remove_container(name: &str) {
    let _ = Command::new("docker").args(["rm", "-f", name]).output();
}

/// Creates a populated directory tree for testing.
/// Returns a list of all created file paths (relative to root).
fn create_test_files(root: &Path, count: usize) -> Vec<String> {
    let mut files = Vec::new();

    // Create a realistic directory structure
    let dirs = [
        "src",
        "src/components",
        "src/components/ui",
        "src/lib",
        "src/utils",
        "config",
        "public",
        "public/images",
        "tests",
        "tests/unit",
        "docs",
    ];

    for dir in &dirs {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }

    for i in 0..count {
        let dir = dirs[i % dirs.len()];
        let filename = format!("{}/file_{}.txt", dir, i);
        let content = format!(
            "// File {} - content for fuzz testing\n{}\n",
            i,
            "x".repeat(100 + (i % 500))
        );
        std::fs::write(root.join(&filename), content).unwrap();
        files.push(filename);
    }

    // Also create some "important" root-level files
    for name in ["README.md", "package.json", "tsconfig.json", ".gitignore"] {
        std::fs::write(root.join(name), format!("// {}", name)).unwrap();
        files.push(name.to_string());
    }

    files
}

/// Verifies all expected files still exist in the directory.
/// Returns a list of missing files.
fn verify_files_exist(root: &Path, expected: &[String]) -> Vec<String> {
    expected
        .iter()
        .filter(|f| !root.join(f).exists())
        .cloned()
        .collect()
}

/// Verifies the directory structure is intact (no directories were deleted).
fn verify_dirs_exist(root: &Path) -> Vec<String> {
    let dirs = [
        "src",
        "src/components",
        "src/components/ui",
        "src/lib",
        "src/utils",
        "config",
        "public",
        "public/images",
        "tests",
        "tests/unit",
        "docs",
    ];

    dirs.iter()
        .filter(|d| !root.join(d).exists())
        .map(|d| d.to_string())
        .collect()
}

/// Wait for mutagen sync to reach a stable state
async fn wait_for_sync(
    mutagen_bin: &Path,
    session_name: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    let backend = RealMutagen::new(mutagen_bin.to_path_buf());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() > deadline {
            return Err(format!(
                "Timeout waiting for session '{}' to reach watching state",
                session_name
            ));
        }

        let sessions = backend
            .list_sessions()
            .await
            .map_err(|e| format!("list_sessions failed: {}", e))?;

        if let Some(s) = sessions.iter().find(|s| s.name == session_name) {
            if s.status == "watching" || s.status == "waiting-for-rescan" {
                return Ok(());
            }
            if s.status.starts_with("halted") {
                return Err(format!("Session halted: {}", s.status));
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn cleanup_all_fuzz_sessions(mutagen_bin: &Path) {
    let backend = RealMutagen::new(mutagen_bin.to_path_buf());
    if let Ok(sessions) = backend.list_sessions().await {
        for s in sessions {
            if s.name.contains("fuzz") {
                let _ = backend.pause_session(&s.identifier).await;
                let _ = backend.terminate_session(&s.identifier).await;
            }
        }
    }
}

fn make_desired_session(
    name: &str,
    alpha: &Path,
    container: &str,
    beta_subdir: &str,
    project_crc32: u32,
    mode: SyncMode,
) -> DesiredSession {
    DesiredSession::new(
        format!("{}-{:08x}", name, project_crc32),
        name.to_string(),
        alpha.to_path_buf(),
        format!("docker://{}//sync/{}", container, beta_subdir),
        mode,
        vec![],
    )
}

macro_rules! assert_no_deletion {
    ($root:expr, $files:expr, $context:expr) => {
        let missing_files = verify_files_exist($root, $files);
        let missing_dirs = verify_dirs_exist($root);
        if !missing_files.is_empty() || !missing_dirs.is_empty() {
            panic!(
                "DIRECTORY DELETION DETECTED during: {}\n  Missing dirs: {:?}\n  Missing files ({}/{}): {:?}",
                $context,
                missing_dirs,
                missing_files.len(),
                $files.len(),
                &missing_files[..missing_files.len().min(20)]
            );
        }
    };
}

// ============================================================================
// Fuzz Test Scenarios
// ============================================================================

/// Scenario 1: Rapid reconcile cycles
///
/// Rapidly call reconcile_sessions() many times in succession.
/// This tests whether the pause-all → create/resume → wait pattern
/// has any windows where files could be deleted.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_rapid_reconcile_cycles() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 50);

    let container = match create_container("fuzz-rapid") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220001;

    println!("=== Fuzz: Rapid reconcile cycles ===");
    println!("Alpha: {:?}", alpha);
    println!("Files: {}", files.len());

    // Initial reconcile to establish session
    let session = make_desired_session("fuzz-rapid", &alpha, &container, "rapid", project_crc32, SyncMode::TwoWaySafe);
    let session_name = session.name.clone();

    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    assert_no_deletion!(&alpha, &files, "initial reconcile");

    // Wait for sync to stabilize
    wait_for_sync(&mutagen_bin, &session_name, 30).await.unwrap();
    assert_no_deletion!(&alpha, &files, "after initial sync");

    // Now: rapid reconcile cycles (same desired state)
    for i in 0..20 {
        println!("  Rapid reconcile cycle {}/20", i + 1);
        let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        assert!(result.is_ok(), "Reconcile cycle {} failed: {:?}", i, result);
        assert_no_deletion!(&alpha, &files, format!("rapid reconcile cycle {}", i));

        // Very short delay to create maximum pressure
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Wait for final sync
    wait_for_sync(&mutagen_bin, &session_name, 30).await.unwrap();
    assert_no_deletion!(&alpha, &files, "after rapid reconcile cycles");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 2: Beta side gets emptied while session is active
///
/// This simulates what happens when the container's sync directory is cleared
/// while mutagen is actively syncing. In two-way-safe mode, mutagen should
/// halt rather than propagate the deletion to alpha.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_beta_emptied_during_sync() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 100);

    let container = match create_container("fuzz-empty-beta") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220002;

    println!("=== Fuzz: Beta emptied during sync ===");

    // Initial reconcile
    let session = make_desired_session("fuzz-betaempty", &alpha, &container, "betaempty", project_crc32, SyncMode::TwoWaySafe);
    let session_name = session.name.clone();

    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);

    // Wait for initial sync to complete
    wait_for_sync(&mutagen_bin, &session_name, 60).await.unwrap();
    assert_no_deletion!(&alpha, &files, "after initial sync");
    println!("  Initial sync complete");

    // Now empty the beta side repeatedly
    for i in 0..5 {
        println!("  Beta empty cycle {}/5", i + 1);

        // Clear beta
        let output = Command::new("docker")
            .args(["exec", &container, "sh", "-c", "rm -rf /sync/betaempty/*"])
            .output()
            .unwrap();
        assert!(output.status.success(), "Failed to clear beta");

        // Wait a moment for mutagen to notice
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Check alpha is still intact!
        assert_no_deletion!(&alpha, &files, format!("after beta empty cycle {}", i));

        // Re-populate beta from alpha by triggering a file change on alpha
        // (touch a file to trigger resync)
        std::fs::write(alpha.join("trigger.txt"), format!("trigger-{}", i)).unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        assert_no_deletion!(&alpha, &files, format!("after re-trigger cycle {}", i));
    }

    assert_no_deletion!(&alpha, &files, "final check after beta empty cycles");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 3: Rapid session recreation (config changes)
///
/// Rapidly change the desired state so sessions get terminated and recreated.
/// This tests the window between terminate-old and create-new.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_rapid_session_recreation() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 50);

    let container = match create_container("fuzz-recreate") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220003;

    println!("=== Fuzz: Rapid session recreation ===");

    // Rapidly cycle through different session configs
    for i in 0..10 {
        println!("  Recreation cycle {}/10", i + 1);

        // Each cycle uses a different beta subdir → different session name
        let beta_subdir = format!("recreate-v{}", i);

        // Create the beta subdir in the container
        let _ = Command::new("docker")
            .args([
                "exec",
                &container,
                "mkdir",
                "-p",
                &format!("/sync/{}", beta_subdir),
            ])
            .output();

        let session = make_desired_session(
            &format!("fuzz-recreate-v{}", i),
            &alpha,
            &container,
            &beta_subdir,
            project_crc32,
            SyncMode::TwoWaySafe,
        );

        let result =
            reconcile_sessions(&mutagen_bin, vec![session], project_crc32, |_| {}).await;
        assert!(result.is_ok(), "Reconcile cycle {} failed: {:?}", i, result);
        assert_no_deletion!(&alpha, &files, format!("recreation cycle {}", i));
    }

    assert_no_deletion!(&alpha, &files, "final check after recreation cycles");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 4: Concurrent reconcile calls
///
/// Start multiple reconcile_sessions() calls concurrently to test for race conditions.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_concurrent_reconciles() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 50);

    let container = match create_container("fuzz-concurrent") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220004;

    println!("=== Fuzz: Concurrent reconcile calls ===");

    let session = make_desired_session("fuzz-concurrent", &alpha, &container, "concurrent", project_crc32, SyncMode::TwoWaySafe);

    // Launch multiple concurrent reconcile calls
    let mut handles = Vec::new();
    for i in 0..5 {
        let bin = mutagen_bin.clone();
        let s = session.clone();
        let handle = tokio::spawn(async move {
            println!("  Concurrent reconcile {} starting", i);
            let result = reconcile_sessions(&bin, vec![s], project_crc32, |_| {}).await;
            println!("  Concurrent reconcile {} done: {:?}", i, result.is_ok());
            result
        });
        handles.push(handle);
    }

    // Wait for all to complete
    for (i, handle) in handles.into_iter().enumerate() {
        let result = handle.await;
        println!("  Concurrent task {} result: {:?}", i, result.is_ok());
    }

    assert_no_deletion!(&alpha, &files, "after concurrent reconciles");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 5: Reconcile with multiple sessions, remove some
///
/// Start with 3 sessions, then reconcile down to 1. The removed sessions
/// get terminated — verify this doesn't affect the remaining session's alpha.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_shrink_session_count() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();

    // Three separate alpha directories
    let alpha1 = temp_dir.path().join("alpha1");
    let alpha2 = temp_dir.path().join("alpha2");
    let alpha3 = temp_dir.path().join("alpha3");
    std::fs::create_dir_all(&alpha1).unwrap();
    std::fs::create_dir_all(&alpha2).unwrap();
    std::fs::create_dir_all(&alpha3).unwrap();

    let files1 = create_test_files(&alpha1, 30);
    let files2 = create_test_files(&alpha2, 30);
    let files3 = create_test_files(&alpha3, 30);

    let container = match create_container("fuzz-shrink") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    // Create beta subdirs
    for sub in ["shrink1", "shrink2", "shrink3"] {
        let _ = Command::new("docker")
            .args(["exec", &container, "mkdir", "-p", &format!("/sync/{}", sub)])
            .output();
    }

    let project_crc32: u32 = 0xf0220005;

    println!("=== Fuzz: Shrink session count ===");

    let s1 = make_desired_session("fuzz-shrink1", &alpha1, &container, "shrink1", project_crc32, SyncMode::TwoWaySafe);
    let s2 = make_desired_session("fuzz-shrink2", &alpha2, &container, "shrink2", project_crc32, SyncMode::TwoWaySafe);
    let s3 = make_desired_session("fuzz-shrink3", &alpha3, &container, "shrink3", project_crc32, SyncMode::TwoWaySafe);

    // Phase 1: All three sessions
    println!("  Phase 1: Create 3 sessions");
    let result = reconcile_sessions(
        &mutagen_bin,
        vec![s1.clone(), s2.clone(), s3.clone()],
        project_crc32,
        |_| {},
    )
    .await;
    assert!(result.is_ok(), "Phase 1 failed: {:?}", result);
    assert_no_deletion!(&alpha1, &files1, "phase 1 alpha1");
    assert_no_deletion!(&alpha2, &files2, "phase 1 alpha2");
    assert_no_deletion!(&alpha3, &files3, "phase 1 alpha3");

    // Phase 2: Remove session 2 and 3
    println!("  Phase 2: Shrink to 1 session");
    let result = reconcile_sessions(&mutagen_bin, vec![s1.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Phase 2 failed: {:?}", result);
    assert_no_deletion!(&alpha1, &files1, "phase 2 alpha1 (kept)");
    assert_no_deletion!(&alpha2, &files2, "phase 2 alpha2 (session terminated)");
    assert_no_deletion!(&alpha3, &files3, "phase 2 alpha3 (session terminated)");

    // Phase 3: Add them back
    println!("  Phase 3: Grow back to 3 sessions");
    let result = reconcile_sessions(
        &mutagen_bin,
        vec![s1.clone(), s2.clone(), s3.clone()],
        project_crc32,
        |_| {},
    )
    .await;
    assert!(result.is_ok(), "Phase 3 failed: {:?}", result);
    assert_no_deletion!(&alpha1, &files1, "phase 3 alpha1");
    assert_no_deletion!(&alpha2, &files2, "phase 3 alpha2");
    assert_no_deletion!(&alpha3, &files3, "phase 3 alpha3");

    // Phase 4: Remove all
    println!("  Phase 4: Remove all sessions");
    let result = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Phase 4 failed: {:?}", result);
    assert_no_deletion!(&alpha1, &files1, "phase 4 alpha1 (all terminated)");
    assert_no_deletion!(&alpha2, &files2, "phase 4 alpha2 (all terminated)");
    assert_no_deletion!(&alpha3, &files3, "phase 4 alpha3 (all terminated)");

    // Cleanup
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 6: File modifications on alpha during reconcile
///
/// Continuously modify files on the alpha side while reconcile loops run.
/// Tests for race conditions between filesystem operations and mutagen sync.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_alpha_modifications_during_reconcile() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 50);

    let container = match create_container("fuzz-alphamod") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220006;

    println!("=== Fuzz: Alpha modifications during reconcile ===");

    let session = make_desired_session("fuzz-alphamod", &alpha, &container, "alphamod", project_crc32, SyncMode::TwoWaySafe);
    let session_name = session.name.clone();

    // Initial reconcile
    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    wait_for_sync(&mutagen_bin, &session_name, 60).await.unwrap();

    // Start a background task that continuously modifies files
    let alpha_clone = alpha.clone();
    let modifier = tokio::spawn(async move {
        for i in 0..50 {
            // Add a new file
            let path = alpha_clone.join(format!("dynamic/file_{}.txt", i));
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            std::fs::write(&path, format!("dynamic content {}", i)).ok();

            // Modify an existing file
            let existing = alpha_clone.join("src/components/file_1.txt");
            if existing.exists() {
                std::fs::write(&existing, format!("modified content {}", i)).ok();
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });

    // Simultaneously run rapid reconcile cycles
    for i in 0..10 {
        println!("  Reconcile during modification {}/10", i + 1);
        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        assert!(result.is_ok(), "Reconcile {} failed: {:?}", i, result);
        assert_no_deletion!(&alpha, &files, format!("modification reconcile {}", i));
    }

    // Wait for modifier to finish
    modifier.await.unwrap();
    assert_no_deletion!(&alpha, &files, "after all modifications");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 7: Container restart during active sync
///
/// Restart the Docker container while mutagen is actively syncing.
/// This is the classic scenario where beta disappears and reappears.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_container_restart_during_sync() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 100);

    let container = match create_container("fuzz-restart") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220007;

    println!("=== Fuzz: Container restart during sync ===");

    let session = make_desired_session("fuzz-restart", &alpha, &container, "restart", project_crc32, SyncMode::TwoWaySafe);
    let session_name = session.name.clone();

    // Initial reconcile and sync
    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    wait_for_sync(&mutagen_bin, &session_name, 60).await.unwrap();
    assert_no_deletion!(&alpha, &files, "after initial sync");
    println!("  Initial sync complete");

    // Restart container multiple times
    for i in 0..3 {
        println!("  Container restart cycle {}/3", i + 1);

        // Stop container (not rm - keeps the container)
        println!("    Stopping container...");
        let _ = Command::new("docker")
            .args(["stop", &container])
            .output();

        // Wait for mutagen to notice disconnection
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert_no_deletion!(&alpha, &files, format!("after container stop {}", i));

        // Start container again
        println!("    Starting container...");
        let _ = Command::new("docker")
            .args(["start", &container])
            .output();

        // Recreate the sync dir (container restart might lose ephemeral storage)
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let _ = Command::new("docker")
            .args(["exec", &container, "mkdir", "-p", "/sync/restart"])
            .output();

        // Wait a bit for mutagen to reconnect
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_no_deletion!(&alpha, &files, format!("after container restart {}", i));

        // Run reconcile to re-establish sync
        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        // Note: reconcile might fail if session is halted, that's OK for this test
        println!("    Reconcile result: {:?}", result.is_ok());
        assert_no_deletion!(
            &alpha,
            &files,
            format!("after reconcile during restart {}", i)
        );
    }

    assert_no_deletion!(&alpha, &files, "final check after container restarts");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 8: Terminate without pause (reproducing the main.rs issue)
///
/// Directly terminate sessions without pausing first, simulating what
/// `ebdev mutagen terminate` does. Then immediately reconcile with new sessions.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_terminate_without_pause_then_reconcile() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 50);

    let container = match create_container("fuzz-nopause") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220008;

    println!("=== Fuzz: Terminate without pause, then reconcile ===");

    let backend = RealMutagen::new(mutagen_bin.clone());

    for i in 0..5 {
        println!("  Cycle {}/5", i + 1);

        let beta_subdir = format!("nopause-v{}", i);
        let _ = Command::new("docker")
            .args(["exec", &container, "mkdir", "-p", &format!("/sync/{}", beta_subdir)])
            .output();

        let session = make_desired_session(
            &format!("fuzz-nopause-v{}", i),
            &alpha,
            &container,
            &beta_subdir,
            project_crc32,
            SyncMode::TwoWaySafe,
        );

        // Create session via reconcile
        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        assert!(result.is_ok(), "Reconcile {} failed: {:?}", i, result);
        assert_no_deletion!(&alpha, &files, format!("after reconcile {}", i));

        // Terminate WITHOUT pausing (simulating `ebdev mutagen terminate`)
        let sessions = backend.list_sessions().await.unwrap();
        for s in sessions.iter().filter(|s| s.name.contains("fuzz-nopause")) {
            println!("    Terminating without pause: {}", s.name);
            let _ = backend.terminate_session(&s.identifier).await;
        }

        // Immediately check files
        assert_no_deletion!(&alpha, &files, format!("after terminate-without-pause {}", i));

        // Small delay
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_no_deletion!(&alpha, &files, format!("after delay post-terminate {}", i));
    }

    assert_no_deletion!(&alpha, &files, "final check");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 9: Beta files deleted, then reconcile creates new session to same beta
///
/// This simulates: container running, beta side gets wiped (e.g. volume recreated),
/// then reconcile creates a new session pointing to the empty beta.
/// This is a key scenario: does the new session sync emptiness back to alpha?
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_new_session_to_empty_beta() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 80);

    let container = match create_container("fuzz-newempty") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf0220009;

    println!("=== Fuzz: New session to empty beta ===");

    // Phase 1: Create session and let it sync
    let _ = Command::new("docker")
        .args(["exec", &container, "mkdir", "-p", "/sync/newempty"])
        .output();

    let session = make_desired_session("fuzz-newempty", &alpha, &container, "newempty", project_crc32, SyncMode::TwoWaySafe);
    let session_name = session.name.clone();

    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    wait_for_sync(&mutagen_bin, &session_name, 60).await.unwrap();
    println!("  Phase 1: Initial sync complete");

    // Phase 2: Terminate all sessions
    let result = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Terminate failed: {:?}", result);
    assert_no_deletion!(&alpha, &files, "after terminate");
    println!("  Phase 2: Sessions terminated");

    // Phase 3: Wipe beta
    let _ = Command::new("docker")
        .args(["exec", &container, "sh", "-c", "rm -rf /sync/newempty/*"])
        .output();
    println!("  Phase 3: Beta wiped");

    // Phase 4: Create NEW session to the now-empty beta
    // This is the critical scenario: does mutagen sync the emptiness back?
    println!("  Phase 4: Creating new session to empty beta...");
    let result = reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    println!("    Reconcile result: {:?}", result.is_ok());

    // Give mutagen time to potentially do damage
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    assert_no_deletion!(&alpha, &files, "CRITICAL: after new session to empty beta");
    println!("  Phase 4: Alpha intact after new session to empty beta");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 10: Stress test - all scenarios combined
///
/// Runs a chaotic sequence of operations to maximize the chance of
/// triggering the directory deletion bug.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_combined_stress() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 100);

    let container = match create_container("fuzz-stress") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf022000a;
    let backend = RealMutagen::new(mutagen_bin.clone());

    println!("=== Fuzz: Combined stress test ===");
    println!("  Alpha: {:?}", alpha);
    println!("  Files: {}", files.len());

    for round in 0..5 {
        println!("  --- Round {}/5 ---", round + 1);

        // Step 1: Create sessions
        let beta_sub = format!("stress-{}", round);
        let _ = Command::new("docker")
            .args(["exec", &container, "mkdir", "-p", &format!("/sync/{}", beta_sub)])
            .output();

        let session = make_desired_session(
            &format!("fuzz-stress-r{}", round),
            &alpha,
            &container,
            &beta_sub,
            project_crc32,
            SyncMode::TwoWaySafe,
        );

        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        println!("    Create: {:?}", result.is_ok());
        assert_no_deletion!(&alpha, &files, format!("round {} create", round));

        // Step 2: Modify alpha
        std::fs::write(alpha.join("stress_marker.txt"), format!("round-{}", round)).unwrap();

        // Step 3: Empty beta
        let _ = Command::new("docker")
            .args(["exec", &container, "sh", "-c", &format!("rm -rf /sync/{}/*", beta_sub)])
            .output();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        assert_no_deletion!(&alpha, &files, format!("round {} after beta empty", round));

        // Step 4: Rapid reconcile
        for j in 0..3 {
            let result = reconcile_sessions(
                &mutagen_bin,
                vec![session.clone()],
                project_crc32,
                |_| {},
            )
            .await;
            println!("    Rapid reconcile {}: {:?}", j, result.is_ok());
            assert_no_deletion!(
                &alpha,
                &files,
                format!("round {} rapid reconcile {}", round, j)
            );
        }

        // Step 5: Terminate without pause
        let sessions = backend.list_sessions().await.unwrap();
        for s in sessions.iter().filter(|s| s.name.contains("fuzz-stress")) {
            let _ = backend.terminate_session(&s.identifier).await;
        }
        assert_no_deletion!(
            &alpha,
            &files,
            format!("round {} after terminate", round)
        );

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    assert_no_deletion!(&alpha, &files, "final stress test check");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

// ============================================================================
// "Beta ist von Anfang an leer" - Tests für das reale Szenario
// ============================================================================

/// Scenario 11: Session resume to empty beta
///
/// This is the most likely real-world scenario:
/// 1. First run: session created, syncs alpha → beta (works fine)
/// 2. Session still exists in mutagen (not terminated)
/// 3. Container was recreated → beta is fresh and empty
/// 4. Second run: reconcile_sessions resumes the existing session
/// 5. Mutagen sees "all files gone on beta" and might sync deletions to alpha
///
/// This tests whether the pause-before-resume in reconcile_sessions prevents this.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_resume_session_to_empty_beta() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 80);

    let container = match create_container("fuzz-resume-empty") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf022000b;
    let backend = RealMutagen::new(mutagen_bin.clone());

    println!("=== Fuzz: Resume session to empty beta ===");

    let session = make_desired_session(
        "fuzz-resume-empty",
        &alpha,
        &container,
        "resume-empty",
        project_crc32,
        SyncMode::TwoWaySafe,
    );
    let session_name = session.name.clone();

    // Phase 1: Initial reconcile and sync
    println!("  Phase 1: Initial sync");
    let result =
        reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    wait_for_sync(&mutagen_bin, &session_name, 60)
        .await
        .unwrap();
    assert_no_deletion!(&alpha, &files, "after initial sync");
    println!("    Sync complete, beta has files");

    for round in 0..5 {
        println!("  Round {}/5: Pause session, empty beta, resume via reconcile", round + 1);

        // Phase 2: Pause the session manually (simulating what happens between ebdev runs)
        let sessions = backend.list_sessions().await.unwrap();
        let our_session = sessions.iter().find(|s| s.name == session_name).unwrap();
        backend.pause_session(&our_session.identifier).await.unwrap();
        println!("    Session paused");

        // Phase 3: Empty beta (simulating fresh container volume)
        let _ = Command::new("docker")
            .args([
                "exec",
                &container,
                "sh",
                "-c",
                "rm -rf /sync/resume-empty/*",
            ])
            .output();
        println!("    Beta emptied");

        // Phase 4: Call reconcile_sessions again (like a new ebdev run)
        // This should:
        // 1. Pause all project sessions (already paused, no-op)
        // 2. Find existing session by name, resume it
        // 3. Wait for sync to complete
        println!("    Running reconcile (like new ebdev run)...");
        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
        println!("    Reconcile result: {:?}", result.is_ok());

        // Give mutagen time to potentially cause damage
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        assert_no_deletion!(
            &alpha,
            &files,
            format!("CRITICAL: round {} after resume to empty beta", round)
        );

        // Wait for re-sync to complete before next round
        if result.is_ok() {
            let _ = wait_for_sync(&mutagen_bin, &session_name, 30).await;
        }
    }

    assert_no_deletion!(&alpha, &files, "final check");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 12: Multiple sessions, one beta empty from start
///
/// Simulates the real workflow:
/// - Multiple sync entries in .ebdev.ts
/// - All betas are empty (fresh container)
/// - reconcile_sessions creates all sessions
/// - Checks that alpha directories survive the initial sync
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_multiple_sessions_empty_betas() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();

    // Create multiple alpha directories with different content
    let alphas: Vec<_> = (0..4)
        .map(|i| {
            let alpha = temp_dir.path().join(format!("alpha{}", i));
            std::fs::create_dir_all(&alpha).unwrap();
            let files = create_test_files(&alpha, 40 + i * 20);
            (alpha, files)
        })
        .collect();

    let container = match create_container("fuzz-multi-empty") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    // Create beta subdirs (all empty)
    for i in 0..4 {
        let _ = Command::new("docker")
            .args([
                "exec",
                &container,
                "mkdir",
                "-p",
                &format!("/sync/multi{}", i),
            ])
            .output();
    }

    let project_crc32: u32 = 0xf022000c;

    println!("=== Fuzz: Multiple sessions, all betas empty ===");

    // Create all sessions at once (like a real .ebdev.ts with multiple sync entries)
    let sessions: Vec<_> = (0..4)
        .map(|i| {
            make_desired_session(
                &format!("fuzz-multi{}", i),
                &alphas[i].0,
                &container,
                &format!("multi{}", i),
                project_crc32,
                SyncMode::TwoWaySafe,
            )
        })
        .collect();

    // Run reconcile with all sessions
    let result =
        reconcile_sessions(&mutagen_bin, sessions.clone(), project_crc32, |status| {
            println!(
                "    Status: {:?}",
                status.iter().map(|s| format!("{}={:?}", s.name, s.status)).collect::<Vec<_>>()
            );
        })
        .await;
    assert!(result.is_ok(), "Reconcile failed: {:?}", result);

    // Check all alphas immediately
    for (i, (alpha, files)) in alphas.iter().enumerate() {
        assert_no_deletion!(alpha, files, format!("alpha{} after reconcile", i));
    }
    println!("  All alphas intact after initial reconcile");

    // Wait for all syncs to complete
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    for (i, (alpha, files)) in alphas.iter().enumerate() {
        assert_no_deletion!(alpha, files, format!("alpha{} after sync settle", i));
    }
    println!("  All alphas intact after sync settle");

    // Now do it again - reconcile with existing sessions (simulating second ebdev run)
    println!("  Second reconcile (existing sessions)...");
    let result =
        reconcile_sessions(&mutagen_bin, sessions.clone(), project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Second reconcile failed: {:?}", result);

    for (i, (alpha, files)) in alphas.iter().enumerate() {
        assert_no_deletion!(alpha, files, format!("alpha{} after second reconcile", i));
    }
    println!("  All alphas intact after second reconcile");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 13: Rapid repeated reconcile cycles to empty beta
///
/// This is the most aggressive "empty beta" test:
/// - Create and terminate sessions rapidly, each time to an empty beta
/// - This maximizes the chance of hitting a timing window where mutagen
///   interprets the empty beta as "files were deleted"
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_rapid_create_to_empty_beta() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 60);

    let container = match create_container("fuzz-rapid-empty") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf022000d;

    println!("=== Fuzz: Rapid create-to-empty-beta cycles ===");

    for i in 0..15 {
        println!("  Cycle {}/15", i + 1);

        // Fresh beta subdir each time (always empty)
        let beta_sub = format!("rapid-empty-{}", i);
        let _ = Command::new("docker")
            .args([
                "exec",
                &container,
                "mkdir",
                "-p",
                &format!("/sync/{}", beta_sub),
            ])
            .output();

        let session = make_desired_session(
            &format!("fuzz-rapid-empty-v{}", i),
            &alpha,
            &container,
            &beta_sub,
            project_crc32,
            SyncMode::TwoWaySafe,
        );

        // Create session to empty beta
        let result =
            reconcile_sessions(&mutagen_bin, vec![session], project_crc32, |_| {}).await;
        println!("    Reconcile: {:?}", result.is_ok());

        // Immediately check alpha (don't wait for sync to complete!)
        assert_no_deletion!(&alpha, &files, format!("cycle {} immediately after reconcile", i));

        // Short delay
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Check again after delay
        assert_no_deletion!(&alpha, &files, format!("cycle {} after 500ms delay", i));

        // Terminate the session before next cycle
        let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
        assert_no_deletion!(&alpha, &files, format!("cycle {} after terminate", i));
    }

    assert_no_deletion!(&alpha, &files, "final check");
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 14: Session resume after container recreate (same name, new filesystem)
///
/// Simulates docker-compose down/up:
/// 1. Container "app" with synced data
/// 2. Remove and recreate container "app" (same name, fresh filesystem)
/// 3. Mutagen session still exists, points to container "app"
/// 4. Resume session → mutagen reconnects to empty beta
///
/// This is probably the most realistic reproduction of the bug.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_container_recreate_same_name() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 80);

    let container = match create_container("fuzz-recreate-same") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf022000e;
    let backend = RealMutagen::new(mutagen_bin.clone());

    println!("=== Fuzz: Container recreate same name ===");

    let session = make_desired_session(
        "fuzz-recreate-same",
        &alpha,
        &container,
        "data",
        project_crc32,
        SyncMode::TwoWaySafe,
    );
    let session_name = session.name.clone();

    // Phase 1: Initial sync
    println!("  Phase 1: Initial sync");
    let result =
        reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Initial reconcile failed: {:?}", result);
    wait_for_sync(&mutagen_bin, &session_name, 60)
        .await
        .unwrap();
    assert_no_deletion!(&alpha, &files, "after initial sync");
    println!("    Initial sync complete");

    for round in 0..3 {
        println!("  Round {}/3: Recreate container", round + 1);

        // Phase 2: Recreate the container (same name, fresh filesystem)
        // This is what docker-compose down/up does
        remove_container(&container);
        let new_container = match create_container("fuzz-recreate-same") {
            Ok(c) => c,
            Err(e) => {
                panic!("Failed to recreate container: {}", e);
            }
        };
        assert_eq!(new_container, container, "Container name should match");
        println!("    Container recreated (beta is empty)");

        // Phase 3: Check alpha BEFORE any reconcile (mutagen might auto-reconnect)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        assert_no_deletion!(
            &alpha,
            &files,
            format!("round {} BEFORE reconcile (mutagen auto-reconnect)", round)
        );

        // Phase 4: Run reconcile (like new ebdev run)
        // The existing session should be found, paused, then resumed
        println!("    Running reconcile...");
        let result =
            reconcile_sessions(&mutagen_bin, vec![session.clone()], project_crc32, |_| {})
                .await;
        println!("    Reconcile result: {:?}", result.is_ok());

        // Phase 5: Wait and check
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        assert_no_deletion!(
            &alpha,
            &files,
            format!("CRITICAL: round {} after reconcile with recreated container", round)
        );

        // Wait for re-sync
        if result.is_ok() {
            let _ = wait_for_sync(&mutagen_bin, &session_name, 30).await;
        }
        assert_no_deletion!(&alpha, &files, format!("round {} after re-sync", round));
    }

    assert_no_deletion!(&alpha, &files, "final check");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Scenario 15: Watch the mutagen session state during initial sync to empty beta
///
/// Instead of just checking files, this test monitors the mutagen session status
/// during the initial sync to see exactly what happens when syncing to empty beta.
#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn fuzz_observe_initial_sync_to_empty_beta() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_all_fuzz_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().unwrap();
    let alpha = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    let files = create_test_files(&alpha, 200); // More files for longer sync

    let container = match create_container("fuzz-observe") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xf022000f;
    let backend = RealMutagen::new(mutagen_bin.clone());

    println!("=== Fuzz: Observe initial sync to empty beta ===");
    println!("  Alpha files: {}", files.len());

    let session = make_desired_session(
        "fuzz-observe",
        &alpha,
        &container,
        "observe",
        project_crc32,
        SyncMode::TwoWaySafe,
    );
    let session_name = session.name.clone();

    // Create session (don't use reconcile_sessions to avoid the pause-all)
    let result = backend.create_session_from_desired(&session, false).await;
    assert!(result.is_ok(), "Create failed: {:?}", result);

    // Monitor the session status and alpha directory integrity during initial sync
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut prev_status = String::new();
    let mut status_transitions = Vec::new();

    loop {
        if std::time::Instant::now() > deadline {
            println!("  Timeout reached");
            break;
        }

        let sessions = backend.list_sessions().await.unwrap();
        if let Some(s) = sessions.iter().find(|s| s.name == session_name) {
            if s.status != prev_status {
                let alpha_file_count = count_files_recursive(&alpha);
                println!(
                    "    Status: {} → {} (alpha: {} files, beta: {} files, {} dirs)",
                    if prev_status.is_empty() {
                        "new"
                    } else {
                        &prev_status
                    },
                    s.status,
                    alpha_file_count,
                    s.beta.files,
                    s.beta.directories
                );
                status_transitions.push((
                    prev_status.clone(),
                    s.status.clone(),
                    alpha_file_count,
                ));
                prev_status = s.status.clone();
            }

            // Check for deletion at EVERY status transition
            let missing = verify_files_exist(&alpha, &files);
            if !missing.is_empty() {
                println!("  !!! DELETION DETECTED at status '{}' !!!", s.status);
                println!("  Status transitions so far: {:?}", status_transitions);
                panic!(
                    "DIRECTORY DELETION during initial sync at status '{}'. Missing {}/{} files.",
                    s.status,
                    missing.len(),
                    files.len()
                );
            }

            if s.status == "watching" || s.status == "waiting-for-rescan" {
                println!("  Sync complete ({})", s.status);
                break;
            }

            if s.status.starts_with("halted") {
                println!("  Session halted: {}", s.status);
                break;
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    assert_no_deletion!(&alpha, &files, "after initial sync observation");
    println!("  Status transitions: {:?}", status_transitions);

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_container(&container);
    println!("  PASSED");
}

/// Count files recursively in a directory
fn count_files_recursive(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let ft = entry.file_type().unwrap();
            if ft.is_file() {
                count += 1;
            } else if ft.is_dir() {
                count += count_files_recursive(&entry.path());
            }
        }
    }
    count
}
