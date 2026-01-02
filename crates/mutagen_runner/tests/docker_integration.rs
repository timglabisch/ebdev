//! Integration tests for mutagen sync with Docker containers.
//!
//! These tests require Docker to be running and will create/destroy containers.
//!
//! Run with:
//! ```sh
//! cargo test --package ebdev-mutagen-runner --test docker_integration -- --ignored --test-threads=1
//! ```
//!
//! Note: Tests must run sequentially (--test-threads=1) because they share a Docker container.
//!
//! Prerequisites:
//! - Docker must be running
//! - Mutagen must be installed (run `ebdev toolchain install` in example/ directory first)
//!
//! The test uses the example directory structure from the workspace root.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Check if Docker is available
fn docker_available() -> bool {
    Command::new("docker")
        .args(["info"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the workspace root directory
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Start the test Docker container
fn start_container() -> bool {
    let example_dir = workspace_root().join("example");

    // First, ensure any existing container is stopped and removed
    let _ = Command::new("docker")
        .args(["compose", "down", "--remove-orphans"])
        .current_dir(&example_dir)
        .output();

    // Also force remove the container if it exists
    let _ = Command::new("docker")
        .args(["rm", "-f", "example-sync-target-1"])
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

/// Stop and remove the test Docker container
fn stop_container() {
    let example_dir = workspace_root().join("example");

    let _ = Command::new("docker")
        .args(["compose", "down"])
        .current_dir(&example_dir)
        .output();
}

/// Terminate all mutagen sessions with a given name prefix
fn terminate_mutagen_sessions(mutagen_bin: &PathBuf) {
    let _ = Command::new(mutagen_bin)
        .args(["sync", "terminate", "--all"])
        .output();
}

/// Create a test file in a directory
fn create_test_file(dir: &PathBuf, filename: &str, content: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(filename), content)
}

/// Check if a file exists in the Docker container
fn file_exists_in_container(container: &str, path: &str) -> bool {
    Command::new("docker")
        .args(["exec", container, "test", "-f", path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read file content from Docker container
fn read_file_from_container(container: &str, path: &str) -> Option<String> {
    Command::new("docker")
        .args(["exec", container, "cat", path])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Get the mutagen binary path
fn get_mutagen_bin() -> PathBuf {
    workspace_root()
        .join("example")
        .join(".ebdev")
        .join("toolchain")
        .join("mutagen")
        .join("v0.18.1")
        .join("mutagen")
}

#[test]
#[ignore = "requires Docker"]
fn test_staged_sync_with_docker() {
    if !docker_available() {
        eprintln!("Docker not available, skipping test");
        return;
    }

    let mutagen_bin = get_mutagen_bin();
    if !mutagen_bin.exists() {
        eprintln!(
            "Mutagen not installed at {:?}, run 'ebdev toolchain install' in example/ first",
            mutagen_bin
        );
        return;
    }

    // Clean up any existing sessions
    terminate_mutagen_sessions(&mutagen_bin);

    // Start container
    if !start_container() {
        panic!("Failed to start Docker container");
    }

    // Create a unique test file
    let test_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let test_filename = format!("integration_test_{}.txt", test_id);
    let test_content = format!("Test content {}", test_id);

    let shared_dir = workspace_root().join("example").join("shared");

    // Create test file in shared directory
    create_test_file(&shared_dir, &test_filename, &test_content)
        .expect("Failed to create test file");

    // Run staged sync using the library
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        let example_dir = workspace_root().join("example");
        let projects = ebdev_mutagen_config::discover_projects(&example_dir)
            .expect("Failed to discover projects");

        // Filter to only stage 0 for this test (shared directory)
        let stage0_projects: Vec<_> = projects
            .into_iter()
            .filter(|p| p.project.stage == 0)
            .collect();

        // Create sync session with no-watch mode
        for project in &stage0_projects {
            let alpha = project.resolved_directory.to_string_lossy().to_string();
            let beta = &project.project.target;

            let output = tokio::process::Command::new(&mutagen_bin)
                .args([
                    "sync",
                    "create",
                    &alpha,
                    beta,
                    &format!("--name=test-{}", project.project.name),
                    "--watch-mode=no-watch",
                ])
                .output()
                .await
                .expect("Failed to create sync");

            if !output.status.success() {
                return Err(format!(
                    "Failed to create sync: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        // Wait for sync to complete
        tokio::time::sleep(Duration::from_secs(5)).await;

        Ok(())
    });

    // Clean up test file
    let _ = std::fs::remove_file(shared_dir.join(&test_filename));

    // Clean up mutagen sessions
    terminate_mutagen_sessions(&mutagen_bin);

    // Stop container
    stop_container();

    // Check result
    result.expect("Sync failed");
}

#[test]
#[ignore = "requires Docker"]
fn test_file_sync_to_container() {
    if !docker_available() {
        eprintln!("Docker not available, skipping test");
        return;
    }

    let mutagen_bin = get_mutagen_bin();
    if !mutagen_bin.exists() {
        eprintln!("Mutagen not installed, skipping test");
        return;
    }

    // Clean up any existing sessions
    terminate_mutagen_sessions(&mutagen_bin);

    // Start container
    if !start_container() {
        panic!("Failed to start Docker container");
    }

    let shared_dir = workspace_root().join("example").join("shared");

    // We test with the existing utils.js file
    let test_filename = "utils.js";
    let expected_content = std::fs::read_to_string(shared_dir.join(test_filename))
        .expect("Failed to read test file");

    // Create a sync session (default two-way mode)
    eprintln!("Creating sync session...");
    eprintln!("  Source: {}", shared_dir.display());
    eprintln!("  Target: docker://ebdev@example-sync-target-1/var/www/shared");

    let output = Command::new(&mutagen_bin)
        .args([
            "sync",
            "create",
            shared_dir.to_str().unwrap(),
            "docker://ebdev@example-sync-target-1/var/www/shared",
            "--name=integration-test-sync",
        ])
        .output()
        .expect("Failed to create sync");

    eprintln!(
        "Sync create stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    eprintln!(
        "Sync create stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output.status.success(),
        "Failed to create sync: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for sync to complete - poll until session shows "Watching for changes"
    for i in 0..30 {
        std::thread::sleep(Duration::from_secs(1));

        let status = Command::new(&mutagen_bin)
            .args(["sync", "list"])
            .output()
            .expect("Failed to list sync");

        let stdout = String::from_utf8_lossy(&status.stdout);

        if stdout.contains("Watching for changes") {
            eprintln!("Sync completed after {} seconds", i + 1);
            break;
        }
    }

    // Give a bit more time for files to be written
    std::thread::sleep(Duration::from_secs(2));

    // Check if file exists in container
    let container_file_path = format!("/var/www/shared/{}", test_filename);
    let synced = file_exists_in_container("example-sync-target-1", &container_file_path);

    // Read content to verify
    let synced_content = read_file_from_container("example-sync-target-1", &container_file_path);

    // Debug output
    if !synced {
        eprintln!("File not found in container: {}", container_file_path);
        let ls_output = Command::new("docker")
            .args([
                "exec",
                "example-sync-target-1",
                "ls",
                "-la",
                "/var/www/shared/",
            ])
            .output()
            .expect("Failed to list container dir");
        eprintln!(
            "Container /var/www/shared contents: {}",
            String::from_utf8_lossy(&ls_output.stdout)
        );
    }

    // Clean up
    terminate_mutagen_sessions(&mutagen_bin);
    stop_container();

    // Assert
    assert!(synced, "File was not synced to container");
    assert_eq!(
        synced_content,
        Some(expected_content),
        "Synced content doesn't match"
    );
}

#[test]
#[ignore = "requires Docker"]
fn test_stage_ordering() {
    if !docker_available() {
        eprintln!("Docker not available, skipping test");
        return;
    }

    let example_dir = workspace_root().join("example");
    let projects =
        ebdev_mutagen_config::discover_projects(&example_dir).expect("Failed to discover projects");

    // Sort by stage
    let mut sorted = projects.clone();
    sorted.sort_by_key(|p| p.project.stage);

    // Verify stages are in order
    let stages: Vec<i32> = sorted.iter().map(|p| p.project.stage).collect();

    // Should be [0, 1, 1, 2] based on example config
    assert!(
        stages.windows(2).all(|w| w[0] <= w[1]),
        "Stages are not in order: {:?}",
        stages
    );

    // Verify we have the expected number of projects
    assert_eq!(projects.len(), 4, "Expected 4 projects in example config");

    // Verify stage distribution
    let stage_counts: std::collections::HashMap<i32, usize> =
        sorted.iter().fold(std::collections::HashMap::new(), |mut acc, p| {
            *acc.entry(p.project.stage).or_insert(0) += 1;
            acc
        });

    assert_eq!(stage_counts.get(&0), Some(&1), "Expected 1 project in stage 0");
    assert_eq!(stage_counts.get(&1), Some(&2), "Expected 2 projects in stage 1");
    assert_eq!(stage_counts.get(&2), Some(&1), "Expected 1 project in stage 2");
}
