//! Integration tests using the real Mutagen backend.
//!
//! These tests run the same scenarios as unit tests but against the real Mutagen CLI.
//! They require:
//! - Docker to be running
//! - Mutagen to be installed (run `ebdev toolchain install` in example/ first)
//!
//! Run with:
//! ```sh
//! cargo test --package ebdev-mutagen-runner --test real_backend_integration --features test-utils -- --ignored --test-threads=1
//! ```
//!
//! Note: Tests must run sequentially (--test-threads=1) to avoid session conflicts.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ebdev_mutagen_config::{DiscoveredProject, MutagenSyncProject, PollingConfig, SyncMode};
use ebdev_mutagen_runner::test_utils::*;
use ebdev_mutagen_runner::RealMutagen;

// Globaler Counter für eindeutige Test-IDs
static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

// ============================================================================
// Test Context - Setup und Cleanup für echte Backend-Tests
// ============================================================================

struct RealTestContext {
    backend: RealMutagen,
    test_prefix: String,
    container_name: String,
}

impl RealTestContext {
    /// Erstellt einen neuen Test-Context, gibt None zurück wenn Voraussetzungen fehlen
    fn new() -> Option<Self> {
        if !docker_available() {
            eprintln!("Docker not available, skipping test");
            return None;
        }

        let mutagen_bin = get_mutagen_bin();
        if !mutagen_bin.exists() {
            eprintln!(
                "Mutagen not installed at {:?}, run 'ebdev toolchain install' in example/ first",
                mutagen_bin
            );
            return None;
        }

        let test_id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let test_prefix = format!("test-{}-{}", std::process::id(), test_id);

        Some(Self {
            backend: RealMutagen::new(mutagen_bin),
            test_prefix,
            container_name: "example-sync-target-1".to_string(),
        })
    }

    /// Erstellt ein Projekt mit eindeutigem Namen für diesen Test
    fn make_project(&self, name: &str, stage: i32) -> DiscoveredProject {
        let unique_name = format!("{}-{}", self.test_prefix, name);
        let shared_dir = workspace_root().join("example").join("shared");

        DiscoveredProject {
            project: MutagenSyncProject {
                name: unique_name,
                target: format!(
                    "docker://ebdev@{}/var/www/test-{}",
                    self.container_name, self.test_prefix
                ),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage,
                ignore: vec![],
                polling: PollingConfig::default(),
            },
            resolved_directory: shared_dir,
            config_path: workspace_root().join("example/.ebdev.toml"),
            root_config_path: workspace_root().join("example/.ebdev.toml"),
        }
    }

    /// Erstellt ein Projekt mit geändertem Target (für Config-Change Tests)
    fn make_project_with_target(&self, name: &str, target_suffix: &str, stage: i32) -> DiscoveredProject {
        let unique_name = format!("{}-{}", self.test_prefix, name);
        let shared_dir = workspace_root().join("example").join("shared");

        DiscoveredProject {
            project: MutagenSyncProject {
                name: unique_name,
                target: format!(
                    "docker://ebdev@{}/var/www/test-{}-{}",
                    self.container_name, self.test_prefix, target_suffix
                ),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage,
                ignore: vec![],
                polling: PollingConfig::default(),
            },
            resolved_directory: shared_dir,
            config_path: workspace_root().join("example/.ebdev.toml"),
            root_config_path: workspace_root().join("example/.ebdev.toml"),
        }
    }

    /// Räumt alle Sessions auf, die zu diesem Test gehören
    async fn cleanup(&self) {
        let _ = cleanup_sessions_with_prefix(&self.backend, &self.test_prefix).await;
    }
}

impl Drop for RealTestContext {
    fn drop(&mut self) {
        // Synchrones Cleanup im Drop - nicht ideal aber funktioniert
        let rt = tokio::runtime::Handle::try_current();
        if let Ok(handle) = rt {
            handle.block_on(async {
                let _ = cleanup_sessions_with_prefix(&self.backend, &self.test_prefix).await;
            });
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn get_mutagen_bin() -> PathBuf {
    workspace_root()
        .join("example")
        .join(".ebdev")
        .join("toolchain")
        .join("mutagen")
        .join("v0.18.1")
        .join("mutagen")
}

fn start_container() -> bool {
    let example_dir = workspace_root().join("example");

    // Ensure any existing container is stopped
    let _ = Command::new("docker")
        .args(["compose", "down", "--remove-orphans"])
        .current_dir(&example_dir)
        .output();

    std::thread::sleep(Duration::from_secs(1));

    // Build and start container
    let output = Command::new("docker")
        .args(["compose", "up", "-d", "--build"])
        .current_dir(&example_dir)
        .output()
        .expect("Failed to run docker compose");

    if !output.status.success() {
        eprintln!(
            "Failed to start container: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return false;
    }

    // Wait for container to be ready
    std::thread::sleep(Duration::from_secs(2));
    true
}

fn stop_container() {
    let example_dir = workspace_root().join("example");
    let _ = Command::new("docker")
        .args(["compose", "down"])
        .current_dir(&example_dir)
        .output();
}

// ============================================================================
// Integration Tests - Szenarien mit echtem Backend
// ============================================================================

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_creates_new_session() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    // Cleanup vor dem Test
    ctx.cleanup().await;

    let project = ctx.make_project("create-test", 0);

    // Führe Szenario aus
    let result = scenario_creates_new_session(&ctx.backend, &project).await;

    // Cleanup
    ctx.cleanup().await;
    stop_container();

    // Verifiziere
    let result = result.expect("Scenario failed");
    assert_eq!(result.created, 1, "Expected 1 session created");
    assert_eq!(result.kept, 0, "Expected 0 sessions kept");
    assert_eq!(result.terminated, 0, "Expected 0 sessions terminated");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_keeps_existing_session() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    let project = ctx.make_project("keep-test", 0);

    // Führe Szenario aus
    let result = scenario_keeps_existing_session(&ctx.backend, &project).await;

    ctx.cleanup().await;
    stop_container();

    // Verifiziere - zweiter Sync sollte Session behalten
    let result = result.expect("Scenario failed");
    assert_eq!(result.kept, 1, "Expected 1 session kept");
    assert_eq!(result.created, 0, "Expected 0 sessions created");
    assert_eq!(result.terminated, 0, "Expected 0 sessions terminated");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_replaces_on_config_change() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    // Gleiches Projekt, unterschiedliches Target
    let old_project = ctx.make_project_with_target("replace-test", "v1", 0);
    let new_project = ctx.make_project_with_target("replace-test", "v2", 0);

    // Führe Szenario aus
    let result = scenario_replaces_on_config_change(&ctx.backend, &old_project, &new_project).await;

    ctx.cleanup().await;
    stop_container();

    // Verifiziere - alte Session terminiert, neue erstellt
    let result = result.expect("Scenario failed");
    assert_eq!(result.terminated, 1, "Expected 1 session terminated");
    assert_eq!(result.created, 1, "Expected 1 session created");
    assert_eq!(result.kept, 0, "Expected 0 sessions kept");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_cleanup_deleted_project() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    let kept_project = ctx.make_project("kept", 0);
    let deleted_project = ctx.make_project("deleted", 0);

    // Führe Szenario aus
    let removed = scenario_cleanup_deleted_project(&ctx.backend, &kept_project, &deleted_project).await;

    ctx.cleanup().await;
    stop_container();

    // Verifiziere - gelöschtes Projekt wurde entfernt
    let removed = removed.expect("Scenario failed");
    assert_eq!(removed, 1, "Expected 1 session removed");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_multiple_sessions_same_stage() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    let p1 = ctx.make_project("multi-1", 0);
    let p2 = ctx.make_project("multi-2", 0);
    let p3 = ctx.make_project("multi-3", 0);
    let projects = vec![&p1, &p2, &p3];

    // Führe Szenario aus
    let result = scenario_multiple_sessions_same_stage(&ctx.backend, &projects).await;

    ctx.cleanup().await;
    stop_container();

    // Verifiziere - alle 3 Sessions erstellt
    let result = result.expect("Scenario failed");
    assert_eq!(result.created, 3, "Expected 3 sessions created");
    assert_eq!(result.kept, 0, "Expected 0 sessions kept");
    assert_eq!(result.terminated, 0, "Expected 0 sessions terminated");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_scenario_stateless_multiple_syncs() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    let project = ctx.make_project("stateless-test", 0);

    // Führe Szenario aus
    let result = scenario_stateless_multiple_syncs(&ctx.backend, &project).await;

    ctx.cleanup().await;
    stop_container();

    // Verifiziere - erster Sync erstellt, danach nur behalten
    let (r1, r2, r3) = result.expect("Scenario failed");

    assert_eq!(r1.created, 1, "First sync: expected 1 created");
    assert_eq!(r1.kept, 0, "First sync: expected 0 kept");

    assert_eq!(r2.created, 0, "Second sync: expected 0 created");
    assert_eq!(r2.kept, 1, "Second sync: expected 1 kept");

    assert_eq!(r3.created, 0, "Third sync: expected 0 created");
    assert_eq!(r3.kept, 1, "Third sync: expected 1 kept");
}

#[tokio::test]
#[ignore = "requires Docker and Mutagen"]
async fn test_real_session_completion_waiting() {
    let ctx = match RealTestContext::new() {
        Some(ctx) => ctx,
        None => return,
    };

    if !start_container() {
        panic!("Failed to start Docker container");
    }

    ctx.cleanup().await;

    let project = ctx.make_project("completion-test", 0);

    // Erstelle Session
    let result = scenario_creates_new_session(&ctx.backend, &project).await;
    assert!(result.is_ok(), "Failed to create session");

    // Warte auf Completion
    let session_name = project.session_name();
    let completed = wait_for_session_complete(&ctx.backend, &session_name, 30_000).await;

    ctx.cleanup().await;
    stop_container();

    // Session sollte "watching" oder ähnlich sein
    assert!(
        completed.expect("Wait failed"),
        "Session did not reach complete status within timeout"
    );
}
