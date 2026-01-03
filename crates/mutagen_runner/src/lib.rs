use std::collections::{BTreeMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::sleep;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use ebdev_mutagen_config::{discover_projects, get_config_paths, DiscoveredProject, SyncMode};

pub mod status;
pub mod tui;

use status::MutagenSession;
use tui::{App, SessionState, TuiMessage};

#[derive(Debug, Error)]
pub enum MutagenRunnerError {
    #[error("Failed to execute mutagen: {0}")]
    Execution(#[from] std::io::Error),

    #[error("Mutagen command failed: {0}")]
    CommandFailed(String),

    #[error("User aborted")]
    UserAborted,

    #[error("Config error: {0}")]
    Config(#[from] ebdev_mutagen_config::MutagenConfigError),

    #[error("Watch error: {0}")]
    WatchError(String),
}

// ============================================================================
// Public API - nur 2 Funktionen statt 4
// ============================================================================

/// Runs staged mutagen sync.
/// - init_only=false: Runs all stages, final stage stays in watch mode with hot-reload
/// - init_only=true: Runs all stages except final, terminates after completion
pub async fn run_staged_sync(
    mutagen_bin: &Path,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    if projects.is_empty() {
        println!("No mutagen sync projects found.");
        return Ok(());
    }

    if std::io::stdout().is_terminal() {
        run_staged_sync_tui(mutagen_bin, base_path, projects, init_only).await
    } else {
        run_staged_sync_headless(mutagen_bin, base_path, projects, init_only).await
    }
}

// ============================================================================
// Core Logic - zentrale Session-Verwaltung
// ============================================================================

/// Cleanup sessions: terminate ALL sessions belonging to this root.
/// This ensures a clean slate at startup before creating new sessions.
/// Returns number of terminated sessions.
async fn cleanup_sessions(
    mutagen_bin: &Path,
    projects: &[DiscoveredProject],
) -> Result<usize, MutagenRunnerError> {
    if projects.is_empty() {
        return Ok(0);
    }

    let root_crc32 = projects[0].root_crc32();
    let root_suffix = format!("{:08x}", root_crc32);

    // Get current sessions
    let current_sessions = poll_status(mutagen_bin).await;

    // Terminate ALL sessions that belong to this root
    let mut terminated = 0;
    for session in &current_sessions {
        if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
            if suffix == root_suffix {
                let _ = terminate_session_by_id(mutagen_bin, &session.identifier).await;
                terminated += 1;
            }
        }
    }

    Ok(terminated)
}

/// Synchronize sessions to match desired state (for hot-reload).
/// - Terminates sessions that no longer exist in projects (by root_crc32)
/// - Keeps sessions that match exactly (same session_name)
/// - Creates sessions that are missing
/// Returns (terminated, kept, created) counts
async fn sync_sessions(
    mutagen_bin: &Path,
    projects: &[DiscoveredProject],
    no_watch: bool,
) -> Result<(usize, usize, usize), MutagenRunnerError> {
    if projects.is_empty() {
        return Ok((0, 0, 0));
    }

    let root_crc32 = projects[0].root_crc32();
    let root_suffix = format!("{:08x}", root_crc32);

    // Get current sessions
    let current_sessions = poll_status(mutagen_bin).await;

    // Build set of expected session names
    let expected_names: HashSet<String> = projects.iter()
        .map(|p| p.session_name())
        .collect();

    // Find sessions to terminate (belong to this root but not in expected)
    let mut terminated = 0;
    let mut kept_names: HashSet<String> = HashSet::new();

    for session in &current_sessions {
        if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
            if suffix == root_suffix {
                if expected_names.contains(&session.name) {
                    kept_names.insert(session.name.clone());
                } else {
                    // Session no longer needed - terminate by ID
                    let _ = terminate_session_by_id(mutagen_bin, &session.identifier).await;
                    terminated += 1;
                }
            }
        }
    }

    // Create sessions that don't exist yet
    let mut created = 0;
    for project in projects {
        let session_name = project.session_name();
        if !kept_names.contains(&session_name) {
            create_sync_session(mutagen_bin, project, no_watch).await?;
            created += 1;
        }
    }

    Ok((terminated, kept_names.len(), created))
}

/// Wait for all sessions in the list to complete their initial sync
async fn wait_for_sessions_complete(
    mutagen_bin: &Path,
    session_names: &[String],
) -> Result<(), MutagenRunnerError> {
    loop {
        let sessions = poll_status(mutagen_bin).await;

        let all_complete = session_names.iter().all(|name| {
            sessions.iter()
                .find(|s| &s.name == name)
                .map(|s| s.is_complete())
                .unwrap_or(true) // If not found, consider complete (auto-terminated)
        });

        if all_complete {
            break;
        }

        sleep(Duration::from_millis(500)).await;
    }
    Ok(())
}

/// Terminate sessions by their names
async fn terminate_sessions_by_name(
    mutagen_bin: &Path,
    session_names: &[String],
) -> Result<(), MutagenRunnerError> {
    let sessions = poll_status(mutagen_bin).await;

    for session in &sessions {
        if session_names.contains(&session.name) {
            let _ = terminate_session_by_id(mutagen_bin, &session.identifier).await;
        }
    }
    Ok(())
}

// ============================================================================
// Headless Implementation
// ============================================================================

async fn run_staged_sync_headless(
    mutagen_bin: &Path,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    // Group projects by stage
    let stages = group_by_stage(&projects);
    let stage_keys: Vec<i32> = stages.keys().copied().collect();
    let last_stage = *stage_keys.last().unwrap();

    // Determine which stages to run
    let stages_to_run: Vec<i32> = if init_only {
        stage_keys.into_iter().filter(|&s| s != last_stage).collect()
    } else {
        stage_keys
    };

    if stages_to_run.is_empty() {
        println!("No stages to run.");
        return Ok(());
    }

    println!("Running {} stage(s){}",
        stages_to_run.len(),
        if init_only { " (init mode)" } else { "" }
    );

    // Cleanup sessions that no longer exist in config
    let terminated = cleanup_sessions(mutagen_bin, &projects).await?;
    if terminated > 0 {
        println!("Cleaned up {} outdated session(s)", terminated);
    }

    // Process each stage
    for stage in stages_to_run.iter() {
        let stage_projects: Vec<_> = stages.get(stage).unwrap().iter().cloned().collect();
        let is_final = !init_only && *stage == last_stage;

        println!("\n=== Stage {} ({} project(s)){} ===",
            stage,
            stage_projects.len(),
            if is_final { " [WATCH MODE]" } else { "" }
        );

        // Create sessions for this stage
        let no_watch = !is_final;
        let session_names: Vec<String> = stage_projects.iter()
            .map(|p| p.session_name())
            .collect();

        for project in &stage_projects {
            create_sync_session(mutagen_bin, project, no_watch).await?;
            println!("  Created: {}", project.session_name());
        }

        if is_final {
            // Final stage: watch mode with hot-reload
            println!("\nWatching for changes. Press Ctrl+C to stop.");

            let config_paths = get_config_paths(&projects);
            let (_watcher, config_rx) = start_config_watcher(config_paths)?;

            loop {
                // Check for config changes
                if config_rx.try_recv().is_ok() {
                    // Debounce
                    sleep(Duration::from_millis(500)).await;
                    while config_rx.try_recv().is_ok() {}

                    println!("\nConfig change detected, reloading...");

                    match discover_projects(base_path) {
                        Ok(new_projects) => {
                            let (term, kept, created) = sync_sessions(
                                mutagen_bin,
                                &new_projects,
                                false,
                            ).await?;
                            println!("  Terminated: {}, Kept: {}, Created: {}", term, kept, created);
                        }
                        Err(e) => eprintln!("  Failed to reload: {}", e),
                    }
                }

                sleep(Duration::from_secs(1)).await;
            }
        } else {
            // Non-final stage: wait for completion, then terminate
            println!("  Waiting for sync to complete...");
            wait_for_sessions_complete(mutagen_bin, &session_names).await?;

            println!("  Sync complete, terminating stage sessions...");
            terminate_sessions_by_name(mutagen_bin, &session_names).await?;
        }
    }

    if init_only {
        println!("\nAll init stages completed successfully.");
    }

    Ok(())
}

// ============================================================================
// TUI Implementation
// ============================================================================

async fn run_staged_sync_tui(
    mutagen_bin: &Path,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    // Group projects by stage
    let stages = group_by_stage(&projects);
    let stage_keys: Vec<i32> = stages.keys().copied().collect();
    let last_stage = *stage_keys.last().unwrap();

    // Determine which stages to run
    let stages_to_run: Vec<i32> = if init_only {
        stage_keys.into_iter().filter(|&s| s != last_stage).collect()
    } else {
        stage_keys
    };

    if stages_to_run.is_empty() {
        println!("No stages to run.");
        return Ok(());
    }

    // Initialize TUI
    let mut terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
    let mut app = App::new(stages_to_run.len() as i32);

    // Start status poller
    let (tx, mut rx) = mpsc::channel::<TuiMessage>(100);
    let poller_handle = tui::spawn_status_poller(mutagen_bin.to_path_buf(), tx.clone());

    // Cleanup sessions that no longer exist in config
    let terminated = cleanup_sessions(mutagen_bin, &projects).await?;
    if terminated > 0 {
        app.set_message(format!("Cleaned up {} outdated session(s)", terminated));
    }

    let result = run_stages_tui_loop(
        &mut terminal,
        &mut app,
        &mut rx,
        mutagen_bin,
        base_path,
        &projects,
        &stages,
        &stages_to_run,
        last_stage,
        init_only,
    ).await;

    // Cleanup
    poller_handle.abort();
    tui::restore().map_err(MutagenRunnerError::Execution)?;

    if result.is_ok() && init_only {
        println!("All init stages completed successfully.");
    }

    result
}

async fn run_stages_tui_loop(
    terminal: &mut tui::Tui,
    app: &mut App,
    rx: &mut mpsc::Receiver<TuiMessage>,
    mutagen_bin: &Path,
    base_path: &Path,
    all_projects: &[DiscoveredProject],
    stages: &BTreeMap<i32, Vec<&DiscoveredProject>>,
    stages_to_run: &[i32],
    last_stage: i32,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    for (stage_idx, stage) in stages_to_run.iter().enumerate() {
        let stage_projects: Vec<_> = stages.get(stage).unwrap().to_vec();
        let is_final = !init_only && *stage == last_stage;

        // Setup TUI for this stage
        let sessions: Vec<SessionState> = stage_projects.iter()
            .map(|p| SessionState {
                name: p.session_name(),
                alpha: p.resolved_directory.to_string_lossy().to_string(),
                beta: p.project.target.clone(),
                status: None,
                created: false,
            })
            .collect();
        app.set_stage(stage_idx as i32, sessions, is_final);

        // Create sessions for this stage
        let no_watch = !is_final;
        let mut session_names = Vec::new();

        for project in &stage_projects {
            // Check for quit
            terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;
            if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                terminate_sessions_by_name(mutagen_bin, &session_names).await?;
                return Err(MutagenRunnerError::UserAborted);
            }

            create_sync_session(mutagen_bin, project, no_watch).await?;
            let name = project.session_name();
            app.mark_session_created(&name);
            session_names.push(name);

            sleep(Duration::from_millis(200)).await;
        }

        if is_final {
            // Final stage: watch mode with hot-reload
            app.set_message("Watching for changes. Press 'q' to quit.".to_string());

            let config_paths = get_config_paths(all_projects);
            let (_watcher, config_rx) = start_config_watcher(config_paths)?;
            let mut pending_reload = false;

            loop {
                terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                    break;
                }

                // Check for config changes
                if config_rx.try_recv().is_ok() {
                    pending_reload = true;
                }

                // Process reload
                if pending_reload {
                    while config_rx.try_recv().is_ok() {}
                    pending_reload = false;

                    app.set_message("Reloading...".to_string());
                    terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                    if let Ok(new_projects) = discover_projects(base_path) {
                        match sync_sessions(mutagen_bin, &new_projects, false).await {
                            Ok((term, kept, created)) => {
                                // Update TUI with new sessions
                                let new_sessions: Vec<SessionState> = new_projects.iter()
                                    .filter(|p| p.project.stage == last_stage)
                                    .map(|p| SessionState {
                                        name: p.session_name(),
                                        alpha: p.resolved_directory.to_string_lossy().to_string(),
                                        beta: p.project.target.clone(),
                                        status: None,
                                        created: true,
                                    })
                                    .collect();
                                app.stage_sessions = new_sessions;
                                app.set_message(format!("Reloaded: -{} ={} +{}", term, kept, created));
                            }
                            Err(e) => app.set_message(format!("Error: {}", e)),
                        }
                    }
                }

                // Process status updates
                while let Ok(msg) = rx.try_recv() {
                    if let TuiMessage::UpdateStatus(sessions) = msg {
                        app.update_session_status(&sessions);
                    }
                }

                sleep(Duration::from_millis(50)).await;
            }
        } else {
            // Non-final stage: wait for completion
            app.set_message(format!("Waiting for stage {} to complete...", stage));

            loop {
                terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                    terminate_sessions_by_name(mutagen_bin, &session_names).await?;
                    return Err(MutagenRunnerError::UserAborted);
                }

                while let Ok(msg) = rx.try_recv() {
                    if let TuiMessage::UpdateStatus(sessions) = msg {
                        app.update_session_status(&sessions);
                    }
                }

                if app.all_sessions_complete() {
                    break;
                }

                sleep(Duration::from_millis(50)).await;
            }

            // Terminate stage sessions
            app.set_message(format!("Stage {} complete, terminating...", stage));
            terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;
            terminate_sessions_by_name(mutagen_bin, &session_names).await?;

            sleep(Duration::from_millis(500)).await;
        }
    }

    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

fn group_by_stage(projects: &[DiscoveredProject]) -> BTreeMap<i32, Vec<&DiscoveredProject>> {
    let mut stages: BTreeMap<i32, Vec<&DiscoveredProject>> = BTreeMap::new();
    for project in projects {
        stages.entry(project.project.stage).or_default().push(project);
    }
    stages
}

fn start_config_watcher(
    config_paths: Vec<PathBuf>,
) -> Result<(RecommendedWatcher, std::sync::mpsc::Receiver<PathBuf>), MutagenRunnerError> {
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if event.kind.is_modify() || event.kind.is_create() {
                for path in event.paths {
                    let _ = tx.send(path);
                }
            }
        }
    })
    .map_err(|e| MutagenRunnerError::WatchError(e.to_string()))?;

    for path in &config_paths {
        if let Some(parent) = path.parent() {
            watcher
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(|e| MutagenRunnerError::WatchError(e.to_string()))?;
        }
    }

    Ok((watcher, rx))
}

async fn create_sync_session(
    mutagen_bin: &Path,
    project: &DiscoveredProject,
    no_watch: bool,
) -> Result<String, MutagenRunnerError> {
    let session_name = project.session_name();
    let alpha = project.resolved_directory.to_string_lossy().to_string();
    let beta = &project.project.target;

    let mut args = vec![
        "sync".to_string(),
        "create".to_string(),
        alpha,
        beta.clone(),
        format!("--name={}", session_name),
    ];

    // Sync mode
    let mode_str = match project.project.mode {
        SyncMode::TwoWay => "two-way-safe",
        SyncMode::OneWayCreate => "one-way-safe",
        SyncMode::OneWayReplica => "one-way-replica",
    };
    args.push(format!("--sync-mode={}", mode_str));

    // Ignore patterns
    for pattern in &project.project.ignore {
        args.push(format!("--ignore={}", pattern));
    }

    // Watch mode
    if no_watch {
        args.push("--watch-mode=no-watch".to_string());
    } else if project.project.polling.enabled {
        args.push("--watch-mode=force-poll".to_string());
        args.push(format!("--watch-polling-interval={}", project.project.polling.interval));
    }

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
            session_name, stderr
        )));
    }

    Ok(session_name)
}

async fn poll_status(mutagen_bin: &Path) -> Vec<MutagenSession> {
    let output = Command::new(mutagen_bin)
        .args(["sync", "list", "--template", "{{json .}}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str(&stdout).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

async fn terminate_session_by_id(
    mutagen_bin: &Path,
    session_id: &str,
) -> Result<(), MutagenRunnerError> {
    let output = Command::new(mutagen_bin)
        .args(["sync", "terminate", session_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("no sessions") && !stderr.contains("not found") {
            return Err(MutagenRunnerError::CommandFailed(format!(
                "Failed to terminate session {}: {}",
                session_id, stderr
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
        // Basic test - real tests would need mock projects
        let stages: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        assert!(stages.is_empty());
    }
}
