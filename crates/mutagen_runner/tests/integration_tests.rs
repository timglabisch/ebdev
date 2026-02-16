//! Integration tests against real mutagen backend
//!
//! These tests require:
//! - mutagen binary installed and in PATH
//! - Docker running with ability to create containers
//!
//! Run with: cargo test -p ebdev-mutagen-runner --test integration_tests -- --ignored

use ebdev_mutagen_runner::{
    reconcile_sessions, state::DesiredSession, MutagenBackend, RealMutagen, SessionStatusInfo,
    SyncMode,
};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const TEST_IMAGE: &str = "alpine:latest";
const TEST_CONTAINER_PREFIX: &str = "ebdev-mutagen-test";

/// Helper to find mutagen binary
fn find_mutagen() -> Option<PathBuf> {
    // Check common locations
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

    // Try PATH
    which::which("mutagen").ok()
}

/// Helper to create a test docker container
fn create_test_container(name: &str) -> Result<String, String> {
    let container_name = format!("{}-{}", TEST_CONTAINER_PREFIX, name);

    // Remove if exists
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .output();

    // Create container
    let output = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &container_name,
            TEST_IMAGE,
            "sleep",
            "3600",
        ])
        .output()
        .map_err(|e| format!("Failed to create container: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Docker run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Create test directory in container
    let output = Command::new("docker")
        .args(["exec", &container_name, "mkdir", "-p", "/sync"])
        .output()
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "mkdir failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(container_name)
}

/// Helper to remove test container
fn remove_test_container(name: &str) {
    let _ = Command::new("docker").args(["rm", "-f", name]).output();
}

/// Helper to terminate all test sessions
async fn cleanup_test_sessions(mutagen_bin: &PathBuf) {
    let backend = RealMutagen::new(mutagen_bin.clone());
    let sessions = backend.list_sessions().await.unwrap();

    for session in sessions {
        if session.name.contains("test-") {
            let _ = backend.terminate_session(&session.identifier).await;
        }
    }
}

// ============================================================================
// Real Backend Tests
// ============================================================================

#[tokio::test]
#[ignore = "requires mutagen binary"]
async fn test_real_backend_list_sessions() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    let backend = RealMutagen::new(mutagen_bin);
    let sessions = backend.list_sessions().await.unwrap();

    // Should not panic, may be empty
    println!("Found {} sessions", sessions.len());
}

#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn test_real_backend_create_and_terminate() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    // Create temp directory for alpha
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    std::fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();

    // Create test container
    let container = match create_test_container("create-terminate") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let backend = RealMutagen::new(mutagen_bin.clone());

    // Create session
    let session = DesiredSession::new(
        "test-create-12345678".to_string(),
        "test-create".to_string(),
        temp_dir.path().to_path_buf(),
        format!("docker://{}//sync", container),
        SyncMode::TwoWay,
        vec![".git".to_string()],
    );

    let result = backend.create_session_from_desired(&session, true).await;
    assert!(result.is_ok(), "Failed to create session: {:?}", result);

    // Verify session exists
    let sessions = backend.list_sessions().await.unwrap();
    let found = sessions.iter().find(|s| s.name == "test-create-12345678");
    assert!(found.is_some(), "Session not found after creation");

    // Terminate session
    let session_id = found.unwrap().identifier.clone();
    let result = backend.terminate_session(&session_id).await;
    assert!(result.is_ok(), "Failed to terminate session: {:?}", result);

    // Verify session is gone
    let sessions = backend.list_sessions().await.unwrap();
    let found = sessions.iter().find(|s| s.name == "test-create-12345678");
    assert!(found.is_none(), "Session still exists after termination");

    // Cleanup
    remove_test_container(&container);
}

#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn test_real_reconcile_full_cycle() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    // Cleanup any leftover test sessions
    cleanup_test_sessions(&mutagen_bin).await;

    // Create temp directories
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let alpha_path = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha_path).unwrap();
    std::fs::write(alpha_path.join("test.txt"), "hello").unwrap();

    // Create test container
    let container = match create_test_container("reconcile") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xdeadbeef;
    let statuses: Arc<Mutex<Vec<Vec<SessionStatusInfo>>>> = Arc::new(Mutex::new(vec![]));
    let statuses_clone = statuses.clone();

    let sessions = vec![DesiredSession::new(
        format!("test-reconcile-{:08x}", project_crc32),
        "test-reconcile".to_string(),
        alpha_path,
        format!("docker://{}//sync", container),
        SyncMode::TwoWay,
        vec![],
    )];

    // Run reconcile
    let result = reconcile_sessions(&mutagen_bin, sessions, project_crc32, move |status| {
        statuses_clone.lock().unwrap().push(status);
    })
    .await;

    assert!(result.is_ok(), "Reconcile failed: {:?}", result);

    // Verify status updates were received
    let received = statuses.lock().unwrap();
    assert!(!received.is_empty(), "No status updates received");
    println!("Received {} status updates", received.len());

    // Last status should show watching
    if let Some(last) = received.last() {
        println!("Final status: {:?}", last);
    }

    // Cleanup: terminate sessions
    let cleanup_result = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    assert!(cleanup_result.is_ok(), "Cleanup failed: {:?}", cleanup_result);

    // Verify sessions are terminated
    let backend = RealMutagen::new(mutagen_bin);
    let sessions = backend.list_sessions().await.unwrap();
    let found = sessions
        .iter()
        .find(|s| s.name.ends_with(&format!("{:08x}", project_crc32)));
    assert!(found.is_none(), "Session still exists after cleanup");

    remove_test_container(&container);
}

#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn test_real_reconcile_recreates_changed_session() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_test_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let alpha_path = temp_dir.path().join("alpha");
    std::fs::create_dir_all(&alpha_path).unwrap();

    let container = match create_test_container("recreate") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let project_crc32: u32 = 0xcafe1234;

    // First reconcile with target /sync1
    let sessions_v1 = vec![DesiredSession::new(
        format!("test-recreate-{:08x}", project_crc32),
        "test-recreate".to_string(),
        alpha_path.clone(),
        format!("docker://{}//sync1", container),
        SyncMode::TwoWay,
        vec![],
    )];

    let result = reconcile_sessions(&mutagen_bin, sessions_v1.clone(), project_crc32, |status| {
        println!("Reconcile status: {:?}", status);
    }).await;
    assert!(result.is_ok(), "First reconcile failed: {:?}", result);

    // Verify session exists (with retry for timing issues)
    let backend = RealMutagen::new(mutagen_bin.clone());
    let expected_name = format!("test-recreate-{:08x}", project_crc32);
    println!("Looking for session: {}", expected_name);

    let mut v1_id = String::new();
    for i in 0..10 {
        let sessions = backend.list_sessions().await.unwrap();
        println!("Attempt {}: Found {} sessions", i + 1, sessions.len());
        for s in &sessions {
            println!("  - {} ({})", s.name, s.identifier);
        }
        if let Some(session) = sessions.iter().find(|s| s.name == expected_name) {
            v1_id = session.identifier.clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(!v1_id.is_empty(), "V1 session '{}' not found after retries", expected_name);

    // Second reconcile with changed target /sync2 (different config = new session name)
    let sessions_v2 = vec![DesiredSession::new(
        format!("test-recreate-v2-{:08x}", project_crc32), // Different name = config changed
        "test-recreate".to_string(),
        alpha_path,
        format!("docker://{}//sync2", container),
        SyncMode::TwoWay,
        vec![],
    )];

    let result = reconcile_sessions(&mutagen_bin, sessions_v2, project_crc32, |_| {}).await;
    assert!(result.is_ok(), "Second reconcile failed: {:?}", result);

    // Verify old session was terminated and new one created
    let sessions = backend.list_sessions().await.unwrap();
    let old_exists = sessions.iter().any(|s| s.identifier == v1_id);
    assert!(!old_exists, "Old session should be terminated");

    let new_exists = sessions.iter().any(|s| s.name.contains("test-recreate-v2"));
    assert!(new_exists, "New session should exist");

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_test_container(&container);
}

// ============================================================================
// Sync Mode Tests
// ============================================================================

#[tokio::test]
#[ignore = "requires mutagen binary and docker"]
async fn test_real_sync_modes() {
    let mutagen_bin = match find_mutagen() {
        Some(p) => p,
        None => {
            eprintln!("Skipping: mutagen not found");
            return;
        }
    };

    cleanup_test_sessions(&mutagen_bin).await;

    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let container = match create_test_container("syncmodes") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping: {}", e);
            return;
        }
    };

    let backend = RealMutagen::new(mutagen_bin.clone());
    let project_crc32: u32 = 0xf00d1234;

    // Test each sync mode
    for (mode, mode_name) in [
        (SyncMode::TwoWay, "twoway"),
        (SyncMode::OneWayCreate, "oneway-create"),
        (SyncMode::OneWayReplica, "oneway-replica"),
    ] {
        let alpha_path = temp_dir.path().join(mode_name);
        std::fs::create_dir_all(&alpha_path).unwrap();

        let session = DesiredSession::new(
            format!("test-{}-{:08x}", mode_name, project_crc32),
            format!("test-{}", mode_name),
            alpha_path,
            format!("docker://{}//sync-{}", container, mode_name),
            mode,
            vec![],
        );

        let result = backend.create_session_from_desired(&session, true).await;
        assert!(
            result.is_ok(),
            "Failed to create {} session: {:?}",
            mode_name,
            result
        );
    }

    // Verify all sessions created
    let sessions = backend.list_sessions().await.unwrap();
    assert!(sessions.iter().any(|s| s.name.contains("twoway")));
    assert!(sessions.iter().any(|s| s.name.contains("oneway-create")));
    assert!(sessions.iter().any(|s| s.name.contains("oneway-replica")));

    // Cleanup
    let _ = reconcile_sessions(&mutagen_bin, vec![], project_crc32, |_| {}).await;
    remove_test_container(&container);
}
