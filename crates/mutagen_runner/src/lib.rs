use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::sleep;

use ebdev_mutagen_config::{DiscoveredProject, SyncMode};

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

    #[error("Sync session not found: {0}")]
    SessionNotFound(String),

    #[error("User aborted")]
    UserAborted,
}

/// Runs staged mutagen sync.
/// Automatically uses TUI if running in a terminal, otherwise uses headless mode.
pub async fn run_staged_sync(
    mutagen_bin: &Path,
    projects: Vec<DiscoveredProject>,
) -> Result<(), MutagenRunnerError> {
    if std::io::stdout().is_terminal() {
        run_staged_sync_tui(mutagen_bin, projects).await
    } else {
        run_staged_sync_headless(mutagen_bin, projects).await
    }
}

/// Runs staged mutagen sync with TUI visualization
pub async fn run_staged_sync_tui(
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
    let total_stages = stage_keys.len() as i32;

    // Initialize TUI
    let mut terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
    let mut app = App::new(total_stages);

    // Channel for communication
    let (tx, mut rx) = mpsc::channel::<TuiMessage>(100);

    // Start status poller
    let mutagen_bin_clone = mutagen_bin.to_path_buf();
    let poller_handle = tui::spawn_status_poller(mutagen_bin_clone, tx.clone());

    let result = run_stages_with_tui(
        &mut terminal,
        &mut app,
        &mut rx,
        mutagen_bin,
        &stages,
        &stage_keys,
        last_stage,
    )
    .await;

    // Cleanup
    poller_handle.abort();
    tui::restore().map_err(MutagenRunnerError::Execution)?;

    result
}

/// Runs staged mutagen sync without TUI (for tests and non-interactive environments)
pub async fn run_staged_sync_headless(
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
                wait_for_sync_complete_headless(mutagen_bin, name).await?;
            }

            println!("Stage {} sync complete. Terminating sessions...", stage);

            for name in &session_names {
                terminate_session(mutagen_bin, name).await?;
            }
        }
    }

    Ok(())
}

async fn run_stages_with_tui(
    terminal: &mut tui::Tui,
    app: &mut App,
    rx: &mut mpsc::Receiver<TuiMessage>,
    mutagen_bin: &Path,
    stages: &BTreeMap<i32, Vec<&DiscoveredProject>>,
    stage_keys: &[i32],
    last_stage: i32,
) -> Result<(), MutagenRunnerError> {
    for (stage_idx, stage) in stage_keys.iter().enumerate() {
        let stage_projects = stages.get(stage).unwrap();
        let is_last_stage = *stage == last_stage;

        // Prepare session states
        let sessions: Vec<SessionState> = stage_projects
            .iter()
            .map(|p| SessionState {
                name: p.project.name.clone(),
                alpha: p.resolved_directory.to_string_lossy().to_string(),
                beta: p.project.target.clone(),
                status: None,
                created: false,
            })
            .collect();

        app.set_stage(stage_idx as i32, sessions, is_last_stage);

        // Create sync sessions for this stage
        for project in stage_projects {
            // Check for quit before creating each session
            terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;
            if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                app.should_quit = true;
                terminate_all_sessions(mutagen_bin, stage_projects).await;
                return Err(MutagenRunnerError::UserAborted);
            }

            create_sync_session(mutagen_bin, project, !is_last_stage).await?;
            app.mark_session_created(&project.project.name);

            // Small delay to let mutagen start
            sleep(Duration::from_millis(200)).await;
        }

        if is_last_stage {
            // Final stage: keep running until user quits
            app.set_message("Final stage running. Press 'q' to quit.".to_string());

            loop {
                terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                // Handle events
                if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                    app.should_quit = true;
                    break;
                }

                // Process status updates
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        TuiMessage::UpdateStatus(sessions) => {
                            app.update_session_status(&sessions);
                        }
                        TuiMessage::Quit => {
                            app.should_quit = true;
                        }
                        _ => {}
                    }
                }

                if app.should_quit {
                    break;
                }

                sleep(Duration::from_millis(50)).await;
            }
        } else {
            // Non-last stage: wait for sync completion, then terminate
            app.set_message(format!("Waiting for stage {} to complete...", stage + 1));

            loop {
                terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                // Handle events
                if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                    app.should_quit = true;
                    terminate_all_sessions(mutagen_bin, stage_projects).await;
                    return Err(MutagenRunnerError::UserAborted);
                }

                // Process status updates
                while let Ok(msg) = rx.try_recv() {
                    if let TuiMessage::UpdateStatus(sessions) = msg {
                        app.update_session_status(&sessions);
                    }
                }

                // Check if all sessions complete
                if app.all_sessions_complete() {
                    break;
                }

                sleep(Duration::from_millis(50)).await;
            }

            // Terminate sessions for this stage
            app.set_message(format!("Stage {} complete. Terminating...", stage + 1));
            terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

            for project in stage_projects {
                terminate_session(mutagen_bin, &project.project.name).await?;
            }

            // Small delay before next stage
            sleep(Duration::from_millis(500)).await;
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
        args.push("--watch-mode=no-watch".to_string());
    } else if project.project.polling.enabled {
        args.push("--watch-mode=force-poll".to_string());
        args.push(format!(
            "--watch-polling-interval={}",
            project.project.polling.interval
        ));
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
            name, stderr
        )));
    }

    Ok(name.clone())
}

async fn wait_for_sync_complete_headless(
    mutagen_bin: &Path,
    session_name: &str,
) -> Result<(), MutagenRunnerError> {
    println!("    Waiting for '{}' to complete...", session_name);

    loop {
        let sessions = poll_status(mutagen_bin).await;

        // Find our session
        if let Some(session) = sessions.iter().find(|s| s.name == session_name) {
            if session.is_complete() {
                break;
            }
        } else {
            // Session not found - might have auto-terminated
            break;
        }

        sleep(Duration::from_millis(500)).await;
    }

    println!("    Completed: {}", session_name);
    Ok(())
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

async fn terminate_all_sessions(mutagen_bin: &Path, projects: &[&DiscoveredProject]) {
    for project in projects {
        let _ = terminate_session(mutagen_bin, &project.project.name).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_grouping() {
        let mut stages: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
        stages.entry(2).or_default().push(1);
        stages.entry(0).or_default().push(2);
        stages.entry(1).or_default().push(3);

        let keys: Vec<i32> = stages.keys().copied().collect();
        assert_eq!(keys, vec![0, 1, 2]);
    }
}
