use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::time::sleep;

use ebdev_mutagen_config::{DiscoveredProject, SyncMode};

#[derive(Debug, Error)]
pub enum MutagenRunnerError {
    #[error("Failed to execute mutagen: {0}")]
    Execution(#[from] std::io::Error),

    #[error("Mutagen command failed: {0}")]
    CommandFailed(String),

    #[error("Sync session not found: {0}")]
    SessionNotFound(String),
}

/// Runs staged mutagen sync
///
/// - Groups projects by stage
/// - Executes stages sequentially (lowest stage first)
/// - All stages except the last use --no-watch and are terminated after sync completes
/// - The last stage runs with watch mode and is NOT terminated
pub async fn run_staged_sync(
    mutagen_bin: &Path,
    projects: Vec<DiscoveredProject>,
) -> Result<(), MutagenRunnerError> {
    if projects.is_empty() {
        println!("No mutagen sync projects found.");
        return Ok(());
    }

    // Group projects by stage
    let mut stages: BTreeMap<i32, Vec<&DiscoveredProject>> = BTreeMap::new();
    for project in &projects {
        stages
            .entry(project.project.stage)
            .or_default()
            .push(project);
    }

    let stage_keys: Vec<i32> = stages.keys().copied().collect();
    let last_stage = *stage_keys.last().unwrap();

    println!(
        "Found {} project(s) in {} stage(s)",
        projects.len(),
        stages.len()
    );

    for stage in stage_keys {
        let stage_projects = stages.get(&stage).unwrap();
        let is_last_stage = stage == last_stage;

        println!();
        println!(
            "=== Stage {} ({} project(s)) {}===",
            stage,
            stage_projects.len(),
            if is_last_stage { "[FINAL - WATCH MODE] " } else { "" }
        );

        // Create sync sessions for this stage
        let mut session_names = Vec::new();
        for project in stage_projects {
            let session_name = create_sync_session(
                mutagen_bin,
                project,
                !is_last_stage, // no_watch for all except last stage
            )
            .await?;
            session_names.push(session_name);
        }

        if is_last_stage {
            // Last stage: keep running, don't terminate
            println!();
            println!("Final stage started in watch mode. Syncs are running...");
            println!("Press Ctrl+C to stop.");

            // Wait indefinitely (user will Ctrl+C)
            loop {
                sleep(Duration::from_secs(60)).await;
            }
        } else {
            // Non-last stage: wait for sync completion, then terminate
            println!("Waiting for stage {} to complete sync...", stage);

            for name in &session_names {
                wait_for_sync_complete(mutagen_bin, name).await?;
            }

            println!("Stage {} sync complete. Terminating sessions...", stage);

            for name in &session_names {
                terminate_session(mutagen_bin, name).await?;
            }
        }
    }

    Ok(())
}

async fn create_sync_session(
    mutagen_bin: &Path,
    project: &DiscoveredProject,
    no_watch: bool,
) -> Result<String, MutagenRunnerError> {
    let name = &project.project.name;

    // First, try to terminate any existing session with this name
    let _ = terminate_session(mutagen_bin, name).await;

    let alpha = project.resolved_directory.to_string_lossy().to_string();
    let beta = &project.project.target;

    let mut args = vec![
        "sync".to_string(),
        "create".to_string(),
        alpha,
        beta.clone(),
        format!("--name={}", name),
    ];

    // Add sync mode
    let mode_str = match project.project.mode {
        SyncMode::TwoWay => "two-way-safe",
        SyncMode::OneWayCreate => "one-way-safe",
        SyncMode::OneWayReplica => "one-way-replica",
    };
    args.push(format!("--sync-mode={}", mode_str));

    // Add ignore patterns
    for pattern in &project.project.ignore {
        args.push(format!("--ignore={}", pattern));
    }

    // Add watch mode
    if no_watch {
        // no-watch: do initial sync only, don't watch for changes
        args.push("--watch-mode=no-watch".to_string());
    } else if project.project.polling.enabled {
        // force-poll with interval
        args.push("--watch-mode=force-poll".to_string());
        args.push(format!(
            "--watch-polling-interval={}",
            project.project.polling.interval
        ));
    }
    // else: use default portable watch mode

    println!("  Creating sync: {} -> {}", name, beta);

    let output = Command::new(mutagen_bin)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MutagenRunnerError::CommandFailed(format!(
            "Failed to create sync '{}': {}",
            name, stderr
        )));
    }

    println!("    Created: {}", name);
    Ok(name.clone())
}

async fn wait_for_sync_complete(
    mutagen_bin: &Path,
    session_name: &str,
) -> Result<(), MutagenRunnerError> {
    println!("    Waiting for '{}' to complete...", session_name);

    loop {
        let output = Command::new(mutagen_bin)
            .args(["sync", "list", "--label-selector", &format!("name={}", session_name)])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Check for completion states
        // When using --no-watch, the session will show "Watching for changes" briefly
        // then complete. We look for states indicating sync is done.
        if stdout.contains("Watching for changes")
            || stdout.contains("Waiting for rescan")
            || stdout.is_empty()
            || !stdout.contains("Scanning")
                && !stdout.contains("Reconciling")
                && !stdout.contains("Staging")
                && !stdout.contains("Transitioning")
        {
            // Check if session still exists (for --no-watch sessions that auto-terminate)
            if stdout.is_empty() || !stdout.contains(session_name) {
                // Session might have auto-terminated after --no-watch completed
                break;
            }

            // Session exists and is in a "done" state
            if stdout.contains("Watching for changes") {
                break;
            }
        }

        sleep(Duration::from_millis(500)).await;
    }

    println!("    Completed: {}", session_name);
    Ok(())
}

async fn terminate_session(
    mutagen_bin: &Path,
    session_name: &str,
) -> Result<(), MutagenRunnerError> {
    let output = Command::new(mutagen_bin)
        .args(["sync", "terminate", "--label-selector", &format!("name={}", session_name)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    // Don't fail if session doesn't exist
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("no sessions") && !stderr.contains("not found") {
            return Err(MutagenRunnerError::CommandFailed(format!(
                "Failed to terminate '{}': {}",
                session_name, stderr
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_grouping() {
        // Simple test to verify BTreeMap ordering
        let mut stages: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        stages.entry(2).or_default().push(1);
        stages.entry(0).or_default().push(2);
        stages.entry(1).or_default().push(3);

        let keys: Vec<i32> = stages.keys().copied().collect();
        assert_eq!(keys, vec![0, 1, 2]);
    }
}
