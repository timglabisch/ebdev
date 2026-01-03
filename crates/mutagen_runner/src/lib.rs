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

    #[error("Sync session not found: {0}")]
    SessionNotFound(String),

    #[error("User aborted")]
    UserAborted,

    #[error("Config error: {0}")]
    Config(#[from] ebdev_mutagen_config::MutagenConfigError),

    #[error("Watch error: {0}")]
    WatchError(String),
}

/// Cleanup all sessions that belong to this root installation (by root_crc32 suffix)
pub async fn cleanup_sessions_by_root(
    mutagen_bin: &Path,
    root_crc32: u32,
) -> Result<Vec<String>, MutagenRunnerError> {
    let root_suffix = format!("{:08x}", root_crc32);
    let sessions = poll_status(mutagen_bin).await;
    let mut terminated = Vec::new();

    for session in sessions {
        if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
            if suffix == root_suffix {
                terminate_session_by_name(mutagen_bin, &session.name).await?;
                terminated.push(session.name);
            }
        }
    }

    Ok(terminated)
}

/// Start watching config files and return a channel that receives change events
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

    // Watch all config file directories (watching files directly can be unreliable)
    for path in &config_paths {
        if let Some(parent) = path.parent() {
            watcher
                .watch(parent, RecursiveMode::NonRecursive)
                .map_err(|e| MutagenRunnerError::WatchError(e.to_string()))?;
        }
    }

    Ok((watcher, rx))
}

/// Synchronize sessions: terminate outdated ones, keep matching ones, create new ones
/// Returns (terminated, kept, created) session names
pub async fn sync_sessions(
    mutagen_bin: &Path,
    projects: &[DiscoveredProject],
    no_watch: bool,
) -> Result<(Vec<String>, Vec<String>, Vec<String>), MutagenRunnerError> {
    if projects.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    let root_crc32 = projects[0].root_crc32();
    let root_suffix = format!("{:08x}", root_crc32);

    // Get current sessions
    let current_sessions = poll_status(mutagen_bin).await;

    // Build set of expected session names
    let expected_names: HashSet<String> = projects.iter()
        .map(|p| p.session_name())
        .collect();

    let mut terminated = Vec::new();
    let mut kept = Vec::new();

    // Check existing sessions
    for session in &current_sessions {
        if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
            if suffix == root_suffix {
                if expected_names.contains(&session.name) {
                    // Session still valid, keep it
                    kept.push(session.name.clone());
                } else {
                    // Session outdated (config changed or project removed), terminate it
                    terminate_session_by_name(mutagen_bin, &session.name).await?;
                    terminated.push(session.name.clone());
                }
            }
        }
    }

    // Create sessions that don't exist yet
    let mut created = Vec::new();
    for project in projects {
        let session_name = project.session_name();
        if !kept.contains(&session_name) {
            create_sync_session(mutagen_bin, project, no_watch).await?;
            created.push(session_name);
        }
    }

    Ok((terminated, kept, created))
}

/// Runs staged mutagen sync.
/// Automatically uses TUI if running in a terminal, otherwise uses headless mode.
/// Supports hot-reload: watches config files and updates sessions when they change.
pub async fn run_staged_sync(
    mutagen_bin: &Path,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
) -> Result<(), MutagenRunnerError> {
    if std::io::stdout().is_terminal() {
        run_staged_sync_tui(mutagen_bin, base_path, projects).await
    } else {
        run_staged_sync_headless(mutagen_bin, base_path, projects).await
    }
}

/// Runs only init stages (all except the final watch stage).
/// Automatically uses TUI if running in a terminal, otherwise uses headless mode.
/// Before starting, cleans up all existing sessions belonging to this root installation.
pub async fn run_staged_sync_init(
    mutagen_bin: &Path,
    projects: Vec<DiscoveredProject>,
) -> Result<(), MutagenRunnerError> {
    if projects.is_empty() {
        println!("No mutagen sync projects found.");
        return Ok(());
    }

    // Cleanup all sessions belonging to this root installation before starting
    let root_crc32 = projects[0].root_crc32();
    let terminated = cleanup_sessions_by_root(mutagen_bin, root_crc32).await?;
    if !terminated.is_empty() {
        println!("Cleaned up {} existing session(s)", terminated.len());
    }

    if std::io::stdout().is_terminal() {
        run_staged_sync_init_tui(mutagen_bin, projects).await
    } else {
        run_staged_sync_init_headless(mutagen_bin, projects).await
    }
}

/// Runs staged mutagen sync with TUI visualization
pub async fn run_staged_sync_tui(
    mutagen_bin: &Path,
    base_path: &Path,
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
        base_path,
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
    base_path: &Path,
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
            // Last stage: keep running with config watching
            println!();
            println!("Final stage started in watch mode. Syncs are running...");
            println!("Watching for config changes. Press Ctrl+C to stop.");

            // Start config watcher
            let config_paths = get_config_paths(&projects);
            let (_watcher, config_rx) = start_config_watcher(config_paths)?;

            // Watch loop with hot-reload support
            loop {
                // Check for config changes (non-blocking)
                if config_rx.try_recv().is_ok() {
                    // Debounce: collect all events within 500ms
                    sleep(Duration::from_millis(500)).await;
                    while config_rx.try_recv().is_ok() {}

                    println!("\nConfig change detected, reloading...");

                    // Reload projects
                    match discover_projects(base_path) {
                        Ok(new_projects) => {
                            // Filter to only final stage projects
                            let final_stage_projects: Vec<_> = new_projects.iter()
                                .filter(|p| p.project.stage == last_stage)
                                .cloned()
                                .collect();

                            let (terminated, kept, created) = sync_sessions(
                                mutagen_bin,
                                &final_stage_projects,
                                false, // watch mode
                            ).await?;

                            if !terminated.is_empty() {
                                println!("  Terminated {} outdated session(s)", terminated.len());
                            }
                            if !kept.is_empty() {
                                println!("  Kept {} unchanged session(s)", kept.len());
                            }
                            if !created.is_empty() {
                                println!("  Created {} new session(s)", created.len());
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to reload config: {}", e);
                        }
                    }
                }

                sleep(Duration::from_secs(1)).await;
            }
        } else {
            // Non-last stage: wait for sync completion, then terminate
            println!("Waiting for stage {} to complete sync...", stage);

            for name in &session_names {
                wait_for_sync_complete_headless(mutagen_bin, name).await?;
            }

            println!("Stage {} sync complete. Terminating sessions...", stage);

            for name in &session_names {
                terminate_session_by_name(mutagen_bin, name).await?;
            }
        }
    }

    Ok(())
}

/// Runs only init stages (all except the final watch stage) without TUI
pub async fn run_staged_sync_init_headless(
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

    // Filter out the last stage
    let init_stage_keys: Vec<i32> = stage_keys.into_iter().filter(|&s| s != last_stage).collect();

    if init_stage_keys.is_empty() {
        println!("No init stages to run (only final stage exists).");
        return Ok(());
    }

    println!(
        "Running {} init stage(s) (excluding final watch stage)",
        init_stage_keys.len()
    );

    for stage in init_stage_keys {
        let stage_projects = stages.get(&stage).unwrap();

        println!();
        println!(
            "=== Stage {} ({} project(s)) ===",
            stage,
            stage_projects.len()
        );

        // Create sync sessions for this stage (all with no_watch since we're doing init)
        let mut session_names = Vec::new();
        for project in stage_projects {
            let session_name = create_sync_session(mutagen_bin, project, true).await?;
            session_names.push(session_name);
        }

        // Wait for sync completion, then terminate
        println!("Waiting for stage {} to complete sync...", stage);

        for name in &session_names {
            wait_for_sync_complete_headless(mutagen_bin, name).await?;
        }

        println!("Stage {} sync complete. Terminating sessions...", stage);

        for name in &session_names {
            terminate_session_by_name(mutagen_bin, name).await?;
        }
    }

    println!();
    println!("All init stages completed successfully.");

    Ok(())
}

/// Runs only init stages (all except the final watch stage) with TUI
pub async fn run_staged_sync_init_tui(
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

    // Filter out the last stage
    let init_stage_keys: Vec<i32> = stage_keys.into_iter().filter(|&s| s != last_stage).collect();

    if init_stage_keys.is_empty() {
        println!("No init stages to run (only final stage exists).");
        return Ok(());
    }

    let total_stages = init_stage_keys.len() as i32;

    // Initialize TUI
    let mut terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
    let mut app = App::new(total_stages);

    // Channel for communication
    let (tx, mut rx) = mpsc::channel::<TuiMessage>(100);

    // Start status poller
    let mutagen_bin_clone = mutagen_bin.to_path_buf();
    let poller_handle = tui::spawn_status_poller(mutagen_bin_clone, tx.clone());

    let result = run_init_stages_with_tui(
        &mut terminal,
        &mut app,
        &mut rx,
        mutagen_bin,
        &stages,
        &init_stage_keys,
    )
    .await;

    // Cleanup
    poller_handle.abort();
    tui::restore().map_err(MutagenRunnerError::Execution)?;

    if result.is_ok() {
        println!("All init stages completed successfully.");
    }

    result
}

async fn run_init_stages_with_tui(
    terminal: &mut tui::Tui,
    app: &mut App,
    rx: &mut mpsc::Receiver<TuiMessage>,
    mutagen_bin: &Path,
    stages: &BTreeMap<i32, Vec<&DiscoveredProject>>,
    stage_keys: &[i32],
) -> Result<(), MutagenRunnerError> {
    for (stage_idx, stage) in stage_keys.iter().enumerate() {
        let stage_projects = stages.get(stage).unwrap();

        // Prepare session states
        let sessions: Vec<SessionState> = stage_projects
            .iter()
            .map(|p| SessionState {
                name: p.session_name(),
                alpha: p.resolved_directory.to_string_lossy().to_string(),
                beta: p.project.target.clone(),
                status: None,
                created: false,
            })
            .collect();

        // false = not final stage (all init stages are non-final)
        app.set_stage(stage_idx as i32, sessions, false);

        // Create sync sessions for this stage
        for project in stage_projects {
            // Check for quit before creating each session
            terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;
            if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                app.should_quit = true;
                terminate_all_sessions(mutagen_bin, stage_projects).await;
                return Err(MutagenRunnerError::UserAborted);
            }

            create_sync_session(mutagen_bin, project, true).await?;
            app.mark_session_created(&project.session_name());

            // Small delay to let mutagen start
            sleep(Duration::from_millis(200)).await;
        }

        // Wait for sync completion, then terminate
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
            terminate_session(mutagen_bin, project).await?;
        }

        // Small delay before next stage
        sleep(Duration::from_millis(500)).await;
    }

    Ok(())
}

async fn run_stages_with_tui(
    terminal: &mut tui::Tui,
    app: &mut App,
    rx: &mut mpsc::Receiver<TuiMessage>,
    mutagen_bin: &Path,
    base_path: &Path,
    stages: &BTreeMap<i32, Vec<&DiscoveredProject>>,
    stage_keys: &[i32],
    last_stage: i32,
) -> Result<(), MutagenRunnerError> {
    // Collect all projects for config watching
    let all_projects: Vec<DiscoveredProject> = stages.values()
        .flat_map(|v| v.iter().map(|p| (*p).clone()))
        .collect();

    for (stage_idx, stage) in stage_keys.iter().enumerate() {
        let stage_projects = stages.get(stage).unwrap();
        let is_last_stage = *stage == last_stage;

        // Prepare session states
        let sessions: Vec<SessionState> = stage_projects
            .iter()
            .map(|p| SessionState {
                name: p.session_name(),
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
            app.mark_session_created(&project.session_name());

            // Small delay to let mutagen start
            sleep(Duration::from_millis(200)).await;
        }

        if is_last_stage {
            // Final stage: keep running with config watching
            app.set_message("Final stage running. Watching configs. Press 'q' to quit.".to_string());

            // Start config watcher
            let config_paths = get_config_paths(&all_projects);
            let (_watcher, config_rx) = start_config_watcher(config_paths)?;
            let mut pending_config_change = false;

            loop {
                terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                // Handle keyboard events
                if tui::handle_events().map_err(MutagenRunnerError::Execution)? {
                    app.should_quit = true;
                    break;
                }

                // Check for config changes (non-blocking)
                if config_rx.try_recv().is_ok() {
                    pending_config_change = true;
                }

                // Process pending config change (with debounce)
                if pending_config_change {
                    // Consume remaining events
                    while config_rx.try_recv().is_ok() {}
                    pending_config_change = false;

                    app.set_message("Config changed, reloading sessions...".to_string());
                    terminal.draw(|f| tui::draw(f, app)).map_err(MutagenRunnerError::Execution)?;

                    // Reload projects
                    if let Ok(new_projects) = discover_projects(base_path) {
                        // Filter to only final stage projects
                        let final_stage_projects: Vec<_> = new_projects.iter()
                            .filter(|p| p.project.stage == last_stage)
                            .cloned()
                            .collect();

                        match sync_sessions(mutagen_bin, &final_stage_projects, false).await {
                            Ok((terminated, kept, created)) => {
                                // Update app sessions
                                let new_sessions: Vec<SessionState> = final_stage_projects
                                    .iter()
                                    .map(|p| SessionState {
                                        name: p.session_name(),
                                        alpha: p.resolved_directory.to_string_lossy().to_string(),
                                        beta: p.project.target.clone(),
                                        status: None,
                                        created: true,
                                    })
                                    .collect();
                                app.stage_sessions = new_sessions;

                                let msg = format!(
                                    "Reloaded: {} terminated, {} kept, {} created",
                                    terminated.len(), kept.len(), created.len()
                                );
                                app.set_message(msg);
                            }
                            Err(e) => {
                                app.set_message(format!("Reload error: {}", e));
                            }
                        }
                    }
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
                        TuiMessage::ConfigChanged => {
                            pending_config_change = true;
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
                terminate_session(mutagen_bin, project).await?;
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
            session_name, stderr
        )));
    }

    Ok(session_name)
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

/// Terminate a session by its full name (with CRC32 suffixes)
async fn terminate_session_by_name(
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

/// Terminate a session for a project (uses session_name())
async fn terminate_session(
    mutagen_bin: &Path,
    project: &DiscoveredProject,
) -> Result<(), MutagenRunnerError> {
    terminate_session_by_name(mutagen_bin, &project.session_name()).await
}

async fn terminate_all_sessions(mutagen_bin: &Path, projects: &[&DiscoveredProject]) {
    for project in projects {
        let _ = terminate_session(mutagen_bin, project).await;
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
