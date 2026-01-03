use std::collections::{BTreeMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
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
// MutagenBackend Trait - abstrahiert Mutagen-Interaktion für Tests
// ============================================================================

/// Trait für Mutagen-Operationen.
/// Ermöglicht Mocking für Tests.
#[async_trait]
pub trait MutagenBackend: Send + Sync {
    /// Listet alle aktuellen Mutagen-Sessions auf
    async fn list_sessions(&self) -> Vec<MutagenSession>;

    /// Erstellt eine neue Sync-Session
    async fn create_session(
        &self,
        project: &DiscoveredProject,
        no_watch: bool,
    ) -> Result<String, MutagenRunnerError>;

    /// Terminiert eine Session anhand ihrer ID
    async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError>;
}

// ============================================================================
// RealMutagen - Echte Mutagen CLI Implementierung
// ============================================================================

/// Echte Mutagen-Implementierung über CLI
pub struct RealMutagen {
    pub bin_path: PathBuf,
}

impl RealMutagen {
    pub fn new(bin_path: PathBuf) -> Self {
        Self { bin_path }
    }
}

#[async_trait]
impl MutagenBackend for RealMutagen {
    async fn list_sessions(&self) -> Vec<MutagenSession> {
        let output = Command::new(&self.bin_path)
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

    async fn create_session(
        &self,
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

        let mode_str = match project.project.mode {
            SyncMode::TwoWay => "two-way-safe",
            SyncMode::OneWayCreate => "one-way-safe",
            SyncMode::OneWayReplica => "one-way-replica",
        };
        args.push(format!("--sync-mode={}", mode_str));

        for pattern in &project.project.ignore {
            args.push(format!("--ignore={}", pattern));
        }

        if no_watch {
            args.push("--watch-mode=no-watch".to_string());
        } else if project.project.polling.enabled {
            args.push("--watch-mode=force-poll".to_string());
            args.push(format!("--watch-polling-interval={}", project.project.polling.interval));
        }

        let output = Command::new(&self.bin_path)
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

    async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
        let output = Command::new(&self.bin_path)
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
}

// ============================================================================
// SyncUI Trait - abstrahiert UI für Headless und TUI
// ============================================================================

/// Trait für UI-Interaktionen während des Sync-Prozesses.
/// Ermöglicht einheitliche Logik für Headless und TUI.
trait SyncUI {
    /// Wird aufgerufen wenn der Sync startet
    fn on_start(&mut self, stage_count: usize, init_only: bool);

    /// Wird aufgerufen wenn Sessions für gelöschte Projekte entfernt wurden
    fn on_cleanup(&mut self, removed: usize);

    /// Wird aufgerufen wenn eine Stage beginnt
    fn on_stage_start(&mut self, stage: i32, projects: &[&DiscoveredProject], is_final: bool);

    /// Wird aufgerufen nach dem Sync einer Stage
    fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize);

    /// Wird aufgerufen während auf Sync-Completion gewartet wird
    fn on_waiting(&mut self, stage: i32);

    /// Wird aufgerufen wenn eine Stage abgeschlossen ist
    fn on_stage_complete(&mut self, stage: i32);

    /// Wird aufgerufen wenn Watch-Mode beginnt
    fn on_watch_mode(&mut self);

    /// Wird aufgerufen wenn Config-Reload startet
    fn on_reload_start(&mut self);

    /// Wird aufgerufen nach Config-Reload
    fn on_reload_result(&mut self, terminated: usize, kept: usize, created: usize);

    /// Wird aufgerufen bei Fehlern
    fn on_error(&mut self, msg: &str);

    /// Wird aufgerufen am Ende (nur bei init_only)
    fn on_complete(&mut self);

    /// Prüft ob der Benutzer abbrechen möchte
    fn check_quit(&mut self) -> Result<bool, MutagenRunnerError>;

    /// Wird in Loops aufgerufen (TUI: draw + events, Headless: sleep)
    fn tick(&mut self) -> Result<(), MutagenRunnerError>;

    /// Update Session-Status (nur für TUI relevant)
    fn update_sessions(&mut self, _sessions: &[MutagenSession]) {}

    /// Prüft ob alle Sessions einer Stage complete sind (für non-final stages)
    fn all_sessions_complete(&self) -> bool { true }

    /// Setzt Sessions für eine Stage (nur für TUI relevant)
    fn set_stage_sessions(&mut self, _stage_idx: usize, _projects: &[&DiscoveredProject], _is_final: bool) {}

    /// Markiert Sessions als erstellt (nur für TUI relevant)
    fn mark_sessions_created(&mut self, _session_names: &[String]) {}

    /// Aktualisiert Session-Liste nach Reload (nur für TUI relevant)
    fn update_stage_sessions(&mut self, _projects: &[DiscoveredProject], _stage: i32) {}
}

// ============================================================================
// HeadlessUI - Einfache println!-basierte Ausgabe
// ============================================================================

struct HeadlessUI;

impl SyncUI for HeadlessUI {
    fn on_start(&mut self, stage_count: usize, init_only: bool) {
        println!("Running {} stage(s){}",
            stage_count,
            if init_only { " (init mode)" } else { "" }
        );
    }

    fn on_cleanup(&mut self, removed: usize) {
        if removed > 0 {
            println!("Removed {} session(s) for deleted projects", removed);
        }
    }

    fn on_stage_start(&mut self, stage: i32, projects: &[&DiscoveredProject], is_final: bool) {
        println!("\n=== Stage {} ({} project(s)){} ===",
            stage,
            projects.len(),
            if is_final { " [WATCH MODE]" } else { "" }
        );
    }

    fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize) {
        if terminated > 0 || kept > 0 || created > 0 {
            println!("  Synced: -{} ={} +{}", terminated, kept, created);
        }
    }

    fn on_waiting(&mut self, _stage: i32) {
        println!("  Waiting for sync to complete...");
    }

    fn on_stage_complete(&mut self, _stage: i32) {
        println!("  Sync complete, terminating stage sessions...");
    }

    fn on_watch_mode(&mut self) {
        println!("\nWatching for changes. Press Ctrl+C to stop.");
    }

    fn on_reload_start(&mut self) {
        println!("\nConfig change detected, reloading...");
    }

    fn on_reload_result(&mut self, terminated: usize, kept: usize, created: usize) {
        println!("  Synced: -{} ={} +{}", terminated, kept, created);
    }

    fn on_error(&mut self, msg: &str) {
        eprintln!("  Error: {}", msg);
    }

    fn on_complete(&mut self) {
        println!("\nAll init stages completed successfully.");
    }

    fn check_quit(&mut self) -> Result<bool, MutagenRunnerError> {
        Ok(false) // Headless kann nicht interaktiv abbrechen
    }

    fn tick(&mut self) -> Result<(), MutagenRunnerError> {
        // Headless macht nichts im tick
        Ok(())
    }
}

// ============================================================================
// TuiUI - Terminal UI mit ratatui
// ============================================================================

struct TuiUI {
    terminal: tui::Tui,
    app: App,
    rx: mpsc::Receiver<TuiMessage>,
    _poller_handle: tokio::task::JoinHandle<()>,
}

impl TuiUI {
    fn new(mutagen_bin: &Path, stage_count: usize) -> Result<Self, MutagenRunnerError> {
        let terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
        let app = App::new(stage_count as i32);
        let (tx, rx) = mpsc::channel::<TuiMessage>(100);
        let poller_handle = tui::spawn_status_poller(mutagen_bin.to_path_buf(), tx);

        Ok(Self {
            terminal,
            app,
            rx,
            _poller_handle: poller_handle,
        })
    }

    fn cleanup(self) -> Result<(), MutagenRunnerError> {
        self._poller_handle.abort();
        tui::restore().map_err(MutagenRunnerError::Execution)
    }

    fn process_status_updates(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if let TuiMessage::UpdateStatus(sessions) = msg {
                self.app.update_session_status(&sessions);
            }
        }
    }
}

impl SyncUI for TuiUI {
    fn on_start(&mut self, _stage_count: usize, _init_only: bool) {
        // TUI zeigt das implizit über die Stage-Anzeige
    }

    fn on_cleanup(&mut self, removed: usize) {
        if removed > 0 {
            self.app.set_message(format!("Removed {} session(s) for deleted projects", removed));
        }
    }

    fn on_stage_start(&mut self, _stage: i32, _projects: &[&DiscoveredProject], _is_final: bool) {
        // Stage-Setup wird separat über set_stage_sessions gemacht
    }

    fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize) {
        if terminated > 0 || kept > 0 || created > 0 {
            self.app.set_message(format!("Synced: -{} ={} +{}", terminated, kept, created));
        }
    }

    fn on_waiting(&mut self, stage: i32) {
        self.app.set_message(format!("Waiting for stage {} to complete...", stage));
    }

    fn on_stage_complete(&mut self, stage: i32) {
        self.app.set_message(format!("Stage {} complete, terminating...", stage));
    }

    fn on_watch_mode(&mut self) {
        self.app.set_message("Watching for changes. Press 'q' to quit.".to_string());
    }

    fn on_reload_start(&mut self) {
        self.app.set_message("Reloading...".to_string());
    }

    fn on_reload_result(&mut self, terminated: usize, kept: usize, created: usize) {
        self.app.set_message(format!("Reloaded: -{} ={} +{}", terminated, kept, created));
    }

    fn on_error(&mut self, msg: &str) {
        self.app.set_message(format!("Error: {}", msg));
    }

    fn on_complete(&mut self) {
        // TUI zeigt nichts extra an - wird nach restore() geprintet
    }

    fn check_quit(&mut self) -> Result<bool, MutagenRunnerError> {
        self.terminal.draw(|f| tui::draw(f, &self.app)).map_err(MutagenRunnerError::Execution)?;
        tui::handle_events().map_err(MutagenRunnerError::Execution)
    }

    fn tick(&mut self) -> Result<(), MutagenRunnerError> {
        self.terminal.draw(|f| tui::draw(f, &self.app)).map_err(MutagenRunnerError::Execution)?;
        self.process_status_updates();
        Ok(())
    }

    fn update_sessions(&mut self, sessions: &[MutagenSession]) {
        self.app.update_session_status(sessions);
    }

    fn all_sessions_complete(&self) -> bool {
        self.app.all_sessions_complete()
    }

    fn set_stage_sessions(&mut self, stage_idx: usize, projects: &[&DiscoveredProject], is_final: bool) {
        let sessions: Vec<SessionState> = projects.iter()
            .map(|p| SessionState {
                name: p.session_name(),
                alpha: p.resolved_directory.to_string_lossy().to_string(),
                beta: p.project.target.clone(),
                status: None,
                created: false,
            })
            .collect();
        self.app.set_stage(stage_idx as i32, sessions, is_final);
    }

    fn mark_sessions_created(&mut self, session_names: &[String]) {
        for name in session_names {
            self.app.mark_session_created(name);
        }
    }

    fn update_stage_sessions(&mut self, projects: &[DiscoveredProject], stage: i32) {
        let new_sessions: Vec<SessionState> = projects.iter()
            .filter(|p| p.project.stage == stage)
            .map(|p| SessionState {
                name: p.session_name(),
                alpha: p.resolved_directory.to_string_lossy().to_string(),
                beta: p.project.target.clone(),
                status: None,
                created: true,
            })
            .collect();
        self.app.stage_sessions = new_sessions;
    }
}

// ============================================================================
// Public API
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
    let backend = Arc::new(RealMutagen::new(mutagen_bin.to_path_buf()));
    run_staged_sync_with_backend(backend, base_path, projects, init_only).await
}

/// Runs staged mutagen sync mit einem beliebigen MutagenBackend.
/// Ermöglicht Mocking für Tests.
pub async fn run_staged_sync_with_backend<M: MutagenBackend + 'static>(
    backend: Arc<M>,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    if projects.is_empty() {
        println!("No mutagen sync projects found.");
        return Ok(());
    }

    // Berechne stages vorab für TUI-Initialisierung
    let stages = group_by_stage(&projects);
    let stage_keys: Vec<i32> = stages.keys().copied().collect();
    let last_stage = *stage_keys.last().unwrap();
    let stages_to_run: Vec<i32> = if init_only {
        stage_keys.into_iter().filter(|&s| s != last_stage).collect()
    } else {
        stage_keys
    };

    if stages_to_run.is_empty() {
        println!("No stages to run.");
        return Ok(());
    }

    if std::io::stdout().is_terminal() {
        // TUI mode braucht den bin_path für den Status-Poller
        // Für Tests ohne echtes Mutagen überspringen wir TUI
        if let Some(real) = (backend.as_ref() as &dyn std::any::Any).downcast_ref::<RealMutagen>() {
            let mut ui = TuiUI::new(&real.bin_path, stages_to_run.len())?;
            let result = run_staged_sync_impl(
                backend,
                base_path,
                projects,
                init_only,
                &mut ui,
            ).await;
            ui.cleanup()?;
            if result.is_ok() && init_only {
                println!("All init stages completed successfully.");
            }
            return result;
        }
    }

    let mut ui = HeadlessUI;
    run_staged_sync_impl(backend, base_path, projects, init_only, &mut ui).await
}

// ============================================================================
// Core Sync Logic - einzige Implementierung
// ============================================================================

async fn run_staged_sync_impl<M: MutagenBackend, U: SyncUI>(
    backend: Arc<M>,
    base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
    ui: &mut U,
) -> Result<(), MutagenRunnerError> {
    let stages = group_by_stage(&projects);
    let stage_keys: Vec<i32> = stages.keys().copied().collect();
    let last_stage = *stage_keys.last().unwrap();

    let stages_to_run: Vec<i32> = if init_only {
        stage_keys.into_iter().filter(|&s| s != last_stage).collect()
    } else {
        stage_keys
    };

    ui.on_start(stages_to_run.len(), init_only);

    // Cleanup sessions für gelöschte Projekte
    let removed = cleanup_removed_projects(backend.as_ref(), &projects).await?;
    ui.on_cleanup(removed);

    // Verarbeite jede Stage
    for (stage_idx, stage) in stages_to_run.iter().enumerate() {
        let stage_projects: Vec<_> = stages.get(stage).unwrap().iter().cloned().collect();
        let is_final = !init_only && *stage == last_stage;

        ui.on_stage_start(*stage, &stage_projects, is_final);
        ui.set_stage_sessions(stage_idx, &stage_projects, is_final);

        // Check quit vor dem Sync
        if ui.check_quit()? {
            return Err(MutagenRunnerError::UserAborted);
        }

        // Sync Sessions für diese Stage
        let no_watch = !is_final;
        let session_names: Vec<String> = stage_projects.iter()
            .map(|p| p.session_name())
            .collect();

        let (term, kept, created) = sync_stage_sessions(backend.as_ref(), &stage_projects, no_watch).await?;
        ui.mark_sessions_created(&session_names);
        ui.on_sync_result(term, kept, created);

        if is_final {
            // Final stage: Watch-Mode mit Hot-Reload
            ui.on_watch_mode();

            let config_paths = get_config_paths(&projects);
            let (_watcher, config_rx) = start_config_watcher(config_paths)?;
            let mut pending_reload = false;

            loop {
                ui.tick()?;

                if ui.check_quit()? {
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

                    ui.on_reload_start();
                    ui.tick()?;

                    match discover_projects(base_path) {
                        Ok(new_projects) => {
                            match sync_sessions(backend.as_ref(), &new_projects, false).await {
                                Ok((term, kept, created)) => {
                                    ui.update_stage_sessions(&new_projects, last_stage);
                                    ui.on_reload_result(term, kept, created);
                                }
                                Err(e) => ui.on_error(&e.to_string()),
                            }
                        }
                        Err(e) => ui.on_error(&format!("Failed to reload: {}", e)),
                    }
                }

                sleep(Duration::from_millis(50)).await;
            }
        } else {
            // Non-final stage: Warte auf Completion, dann terminiere
            ui.on_waiting(*stage);

            loop {
                ui.tick()?;

                if ui.check_quit()? {
                    terminate_sessions_by_name(backend.as_ref(), &session_names).await?;
                    return Err(MutagenRunnerError::UserAborted);
                }

                if ui.all_sessions_complete() {
                    // Für Headless: polling-basierte Completion-Check
                    let sessions = backend.list_sessions().await;
                    let all_complete = session_names.iter().all(|name| {
                        sessions.iter()
                            .find(|s| &s.name == name)
                            .map(|s| s.is_complete())
                            .unwrap_or(true)
                    });

                    if all_complete {
                        break;
                    }
                }

                sleep(Duration::from_millis(50)).await;
            }

            ui.on_stage_complete(*stage);
            ui.tick()?;
            terminate_sessions_by_name(backend.as_ref(), &session_names).await?;

            sleep(Duration::from_millis(500)).await;
        }
    }

    if init_only {
        ui.on_complete();
    }

    Ok(())
}

// ============================================================================
// Core Session Management
// ============================================================================

async fn cleanup_removed_projects(
    backend: &dyn MutagenBackend,
    projects: &[DiscoveredProject],
) -> Result<usize, MutagenRunnerError> {
    if projects.is_empty() {
        return Ok(0);
    }

    let root_crc32 = projects[0].root_crc32();
    let root_suffix = format!("{:08x}", root_crc32);

    let current_sessions = backend.list_sessions().await;

    let valid_prefixes: HashSet<String> = projects.iter()
        .map(|p| format!("{}-", p.project.name))
        .collect();

    let mut terminated = 0;
    for session in &current_sessions {
        if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
            if suffix == root_suffix {
                let project_still_exists = valid_prefixes.iter()
                    .any(|prefix| session.name.starts_with(prefix));

                if !project_still_exists {
                    let _ = backend.terminate_session(&session.identifier).await;
                    terminated += 1;
                }
            }
        }
    }

    Ok(terminated)
}

async fn sync_stage_sessions(
    backend: &dyn MutagenBackend,
    stage_projects: &[&DiscoveredProject],
    no_watch: bool,
) -> Result<(usize, usize, usize), MutagenRunnerError> {
    if stage_projects.is_empty() {
        return Ok((0, 0, 0));
    }

    let root_crc32 = stage_projects[0].root_crc32();
    let root_suffix = format!("{:08x}", root_crc32);

    let current_sessions = backend.list_sessions().await;

    let mut terminated = 0;
    let mut kept = 0;
    let mut created = 0;

    for project in stage_projects {
        let expected_name = project.session_name();
        let project_prefix = format!("{}-", project.project.name);

        let mut found_matching = false;
        for session in &current_sessions {
            if session.name.starts_with(&project_prefix) {
                if let Some(suffix) = DiscoveredProject::extract_root_crc32_suffix(&session.name) {
                    if suffix == root_suffix {
                        if session.name == expected_name {
                            found_matching = true;
                            kept += 1;
                        } else {
                            let _ = backend.terminate_session(&session.identifier).await;
                            terminated += 1;
                        }
                    }
                }
            }
        }

        if !found_matching {
            backend.create_session(project, no_watch).await?;
            created += 1;
        }
    }

    Ok((terminated, kept, created))
}

async fn sync_sessions(
    backend: &dyn MutagenBackend,
    projects: &[DiscoveredProject],
    no_watch: bool,
) -> Result<(usize, usize, usize), MutagenRunnerError> {
    if projects.is_empty() {
        return Ok((0, 0, 0));
    }

    let removed = cleanup_removed_projects(backend, projects).await?;
    let project_refs: Vec<&DiscoveredProject> = projects.iter().collect();
    let (term, kept, created) = sync_stage_sessions(backend, &project_refs, no_watch).await?;

    Ok((removed + term, kept, created))
}

async fn terminate_sessions_by_name(
    backend: &dyn MutagenBackend,
    session_names: &[String],
) -> Result<(), MutagenRunnerError> {
    let sessions = backend.list_sessions().await;

    for session in &sessions {
        if session_names.contains(&session.name) {
            let _ = backend.terminate_session(&session.identifier).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mock UI für Tests
    #[derive(Default)]
    struct MockUI {
        events: Vec<String>,
        quit_after: Option<usize>,
        tick_count: usize,
    }

    impl SyncUI for MockUI {
        fn on_start(&mut self, stage_count: usize, init_only: bool) {
            self.events.push(format!("start:{}:{}", stage_count, init_only));
        }
        fn on_cleanup(&mut self, removed: usize) {
            self.events.push(format!("cleanup:{}", removed));
        }
        fn on_stage_start(&mut self, stage: i32, projects: &[&DiscoveredProject], is_final: bool) {
            self.events.push(format!("stage_start:{}:{}:{}", stage, projects.len(), is_final));
        }
        fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize) {
            self.events.push(format!("sync_result:{}:{}:{}", terminated, kept, created));
        }
        fn on_waiting(&mut self, stage: i32) {
            self.events.push(format!("waiting:{}", stage));
        }
        fn on_stage_complete(&mut self, stage: i32) {
            self.events.push(format!("stage_complete:{}", stage));
        }
        fn on_watch_mode(&mut self) {
            self.events.push("watch_mode".to_string());
        }
        fn on_reload_start(&mut self) {
            self.events.push("reload_start".to_string());
        }
        fn on_reload_result(&mut self, terminated: usize, kept: usize, created: usize) {
            self.events.push(format!("reload_result:{}:{}:{}", terminated, kept, created));
        }
        fn on_error(&mut self, msg: &str) {
            self.events.push(format!("error:{}", msg));
        }
        fn on_complete(&mut self) {
            self.events.push("complete".to_string());
        }
        fn check_quit(&mut self) -> Result<bool, MutagenRunnerError> {
            if let Some(quit_after) = self.quit_after {
                if self.tick_count >= quit_after {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        fn tick(&mut self) -> Result<(), MutagenRunnerError> {
            self.tick_count += 1;
            Ok(())
        }
    }

    /// Mock Mutagen Backend für Tests
    #[derive(Default)]
    pub struct MockMutagen {
        sessions: Mutex<Vec<MutagenSession>>,
        create_calls: Mutex<Vec<(String, bool)>>,
        terminate_calls: Mutex<Vec<String>>,
    }

    impl MockMutagen {
        pub fn new() -> Self {
            Self::default()
        }

        /// Fügt eine Session zum Mock hinzu
        pub fn add_session(&self, session: MutagenSession) {
            self.sessions.lock().unwrap().push(session);
        }

        /// Prüft welche Sessions erstellt wurden
        pub fn created_sessions(&self) -> Vec<(String, bool)> {
            self.create_calls.lock().unwrap().clone()
        }

        /// Prüft welche Sessions terminiert wurden
        pub fn terminated_sessions(&self) -> Vec<String> {
            self.terminate_calls.lock().unwrap().clone()
        }
    }

    fn mock_endpoint(path: &str) -> status::EndpointStatus {
        status::EndpointStatus {
            protocol: "local".to_string(),
            path: path.to_string(),
            host: None,
            user: None,
            connected: true,
            scanned: true,
            directories: 0,
            files: 0,
            total_file_size: 0,
        }
    }

    fn mock_session(name: &str) -> MutagenSession {
        MutagenSession {
            identifier: format!("mock-{}", name),
            name: name.to_string(),
            status: "watching".to_string(),
            successful_cycles: 0,
            alpha: mock_endpoint("/test"),
            beta: mock_endpoint("/target"),
        }
    }

    #[async_trait]
    impl MutagenBackend for MockMutagen {
        async fn list_sessions(&self) -> Vec<MutagenSession> {
            self.sessions.lock().unwrap().clone()
        }

        async fn create_session(
            &self,
            project: &DiscoveredProject,
            no_watch: bool,
        ) -> Result<String, MutagenRunnerError> {
            let session_name = project.session_name();
            self.create_calls.lock().unwrap().push((session_name.clone(), no_watch));

            // Füge die Session auch zur Liste hinzu
            let session = mock_session(&session_name);
            self.sessions.lock().unwrap().push(session);

            Ok(session_name)
        }

        async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
            self.terminate_calls.lock().unwrap().push(session_id.to_string());
            // Entferne die Session aus der Liste
            self.sessions.lock().unwrap().retain(|s| s.identifier != session_id);
            Ok(())
        }
    }

    // ========================================================================
    // Helper für Test-Projekte
    // ========================================================================

    fn make_project(name: &str, target: &str, stage: i32, root: &str) -> DiscoveredProject {
        use ebdev_mutagen_config::{MutagenSyncProject, PollingConfig};

        DiscoveredProject {
            project: MutagenSyncProject {
                name: name.to_string(),
                target: target.to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage,
                ignore: vec![],
                polling: PollingConfig::default(),
            },
            resolved_directory: PathBuf::from(format!("{}/{}", root, name)),
            config_path: PathBuf::from(format!("{}/.ebdev.toml", root)),
            root_config_path: PathBuf::from(format!("{}/.ebdev.toml", root)),
        }
    }

    fn make_session_for_project(project: &DiscoveredProject) -> MutagenSession {
        let name = project.session_name();
        MutagenSession {
            identifier: format!("id-{}", name),
            name,
            status: "watching".to_string(),
            successful_cycles: 1,
            alpha: mock_endpoint(&project.resolved_directory.to_string_lossy()),
            beta: mock_endpoint(&project.project.target),
        }
    }

    // ========================================================================
    // Tests: group_by_stage
    // ========================================================================

    #[test]
    fn test_group_by_stage_empty() {
        let projects: Vec<DiscoveredProject> = vec![];
        let stages = group_by_stage(&projects);
        assert!(stages.is_empty());
    }

    #[test]
    fn test_group_by_stage_single_stage() {
        let p1 = make_project("frontend", "docker://app", 0, "/root");
        let p2 = make_project("backend", "docker://app", 0, "/root");
        let projects = vec![p1, p2];

        let stages = group_by_stage(&projects);

        assert_eq!(stages.len(), 1);
        assert_eq!(stages.get(&0).unwrap().len(), 2);
    }

    #[test]
    fn test_group_by_stage_multiple_stages() {
        let p1 = make_project("shared", "docker://app", 0, "/root");
        let p2 = make_project("frontend", "docker://app", 1, "/root");
        let p3 = make_project("backend", "docker://app", 1, "/root");
        let p4 = make_project("config", "docker://app", 2, "/root");
        let projects = vec![p1, p2, p3, p4];

        let stages = group_by_stage(&projects);

        assert_eq!(stages.len(), 3);
        assert_eq!(stages.get(&0).unwrap().len(), 1);
        assert_eq!(stages.get(&1).unwrap().len(), 2);
        assert_eq!(stages.get(&2).unwrap().len(), 1);
    }

    // ========================================================================
    // Tests: sync_stage_sessions - Neue Sessions erstellen
    // ========================================================================

    #[tokio::test]
    async fn test_sync_stage_sessions_creates_new_session() {
        let backend = MockMutagen::new();
        let project = make_project("test", "docker://test", 0, "/root");

        let projects = vec![&project];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, true).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 1);
        assert_eq!(backend.created_sessions().len(), 1);
        assert_eq!(backend.created_sessions()[0].0, project.session_name());
    }

    #[tokio::test]
    async fn test_sync_stage_sessions_creates_multiple_sessions() {
        let backend = MockMutagen::new();
        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");

        let projects = vec![&p1, &p2];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 2);
        assert_eq!(backend.created_sessions().len(), 2);
    }

    // ========================================================================
    // Tests: sync_stage_sessions - Stateless Verhalten (Sessions behalten)
    // ========================================================================

    #[tokio::test]
    async fn test_sync_stage_sessions_keeps_matching_session() {
        let backend = MockMutagen::new();
        let project = make_project("test", "docker://test", 0, "/root");

        // Füge existierende Session hinzu die genau passt
        backend.add_session(make_session_for_project(&project));

        let projects = vec![&project];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, true).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 1);
        assert_eq!(created, 0);
        assert!(backend.created_sessions().is_empty());
        assert!(backend.terminated_sessions().is_empty());
    }

    #[tokio::test]
    async fn test_sync_stage_sessions_keeps_multiple_matching_sessions() {
        let backend = MockMutagen::new();
        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");

        // Beide Sessions existieren bereits
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));

        let projects = vec![&p1, &p2];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 2);
        assert_eq!(created, 0);
    }

    // ========================================================================
    // Tests: sync_stage_sessions - Config-Änderung (Session ersetzen)
    // ========================================================================

    #[tokio::test]
    async fn test_sync_stage_sessions_replaces_when_config_changes() {
        let backend = MockMutagen::new();

        // Altes Projekt mit altem Target
        let old_project = make_project("test", "docker://old-target", 0, "/root");
        backend.add_session(make_session_for_project(&old_project));

        // Neues Projekt mit neuem Target (gleicher Name, aber anderer CRC)
        let new_project = make_project("test", "docker://new-target", 0, "/root");

        let projects = vec![&new_project];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, true).await.unwrap();

        // Alte Session wird terminiert, neue wird erstellt
        assert_eq!(terminated, 1);
        assert_eq!(kept, 0);
        assert_eq!(created, 1);
        assert_eq!(backend.terminated_sessions().len(), 1);
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_sync_stage_sessions_replaces_when_directory_changes() {
        let backend = MockMutagen::new();

        // Altes Projekt
        let mut old_project = make_project("test", "docker://test", 0, "/root");
        old_project.project.directory = Some(PathBuf::from("./old"));
        backend.add_session(make_session_for_project(&old_project));

        // Neues Projekt mit anderem Verzeichnis
        let mut new_project = make_project("test", "docker://test", 0, "/root");
        new_project.project.directory = Some(PathBuf::from("./new"));

        let projects = vec![&new_project];
        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, true).await.unwrap();

        assert_eq!(terminated, 1);
        assert_eq!(kept, 0);
        assert_eq!(created, 1);
    }

    // ========================================================================
    // Tests: cleanup_removed_projects
    // ========================================================================

    #[tokio::test]
    async fn test_cleanup_removes_sessions_for_deleted_projects() {
        let backend = MockMutagen::new();

        // Zwei Projekte existierten
        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));

        // Nur frontend existiert noch
        let remaining = vec![p1];
        let removed = cleanup_removed_projects(&backend, &remaining).await.unwrap();

        assert_eq!(removed, 1);
        assert_eq!(backend.terminated_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_cleanup_does_not_remove_sessions_from_other_roots() {
        let backend = MockMutagen::new();

        // Projekte aus verschiedenen Roots
        let p1 = make_project("shared", "docker://app", 0, "/root1");
        let p2 = make_project("shared", "docker://app", 0, "/root2");
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));

        // Cleanup nur für root1
        let projects = vec![p1];
        let removed = cleanup_removed_projects(&backend, &projects).await.unwrap();

        // Sollte nichts entfernen, da das Projekt noch existiert
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_cleanup_with_no_projects() {
        let backend = MockMutagen::new();
        backend.add_session(mock_session("some-session"));

        let projects: Vec<DiscoveredProject> = vec![];
        let removed = cleanup_removed_projects(&backend, &projects).await.unwrap();

        // Bei leerer Projektliste wird nichts entfernt
        assert_eq!(removed, 0);
    }

    // ========================================================================
    // Tests: terminate_sessions_by_name
    // ========================================================================

    #[tokio::test]
    async fn test_terminate_sessions_by_name() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));

        let names = vec![p1.session_name()];
        terminate_sessions_by_name(&backend, &names).await.unwrap();

        assert_eq!(backend.terminated_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_terminate_sessions_by_name_multiple() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        let p3 = make_project("config", "docker://app", 2, "/root");
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));
        backend.add_session(make_session_for_project(&p3));

        let names = vec![p1.session_name(), p2.session_name()];
        terminate_sessions_by_name(&backend, &names).await.unwrap();

        assert_eq!(backend.terminated_sessions().len(), 2);
        // p3 sollte noch existieren
        assert_eq!(backend.list_sessions().await.len(), 1);
    }

    // ========================================================================
    // Tests: sync_sessions (Full Sync: Cleanup + Stage Sync)
    // ========================================================================

    #[tokio::test]
    async fn test_sync_sessions_creates_all_new() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        let projects = vec![p1, p2];

        let (terminated, kept, created) = sync_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 2);
    }

    #[tokio::test]
    async fn test_sync_sessions_keeps_existing() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        backend.add_session(make_session_for_project(&p1));
        backend.add_session(make_session_for_project(&p2));

        let projects = vec![p1, p2];
        let (terminated, kept, created) = sync_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 2);
        assert_eq!(created, 0);
    }

    #[tokio::test]
    async fn test_sync_sessions_mixed_scenario() {
        let backend = MockMutagen::new();

        // Existierende Sessions: frontend (bleibt), old-service (wird entfernt)
        let frontend = make_project("frontend", "docker://app", 1, "/root");
        let old_service = make_project("old-service", "docker://app", 1, "/root");
        backend.add_session(make_session_for_project(&frontend));
        backend.add_session(make_session_for_project(&old_service));

        // Neue Konfiguration: frontend (bleibt), backend (neu)
        let backend_proj = make_project("backend", "docker://app", 1, "/root");
        let projects = vec![frontend, backend_proj];

        let (terminated, kept, created) = sync_sessions(&backend, &projects, false).await.unwrap();

        // old-service wird entfernt (cleanup), frontend bleibt, backend neu
        assert_eq!(terminated, 1); // old-service
        assert_eq!(kept, 1);       // frontend
        assert_eq!(created, 1);    // backend
    }

    // ========================================================================
    // Tests: Stateless Verhalten über mehrere Aufrufe
    // ========================================================================

    #[tokio::test]
    async fn test_stateless_multiple_syncs() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        let projects = vec![p1.clone(), p2.clone()];

        // Erster Sync: Alles neu
        let (t1, k1, c1) = sync_sessions(&backend, &projects, false).await.unwrap();
        assert_eq!((t1, k1, c1), (0, 0, 2));

        // Zweiter Sync: Alles behalten (stateless!)
        let (t2, k2, c2) = sync_sessions(&backend, &projects, false).await.unwrap();
        assert_eq!((t2, k2, c2), (0, 2, 0));

        // Dritter Sync: Immer noch alles behalten
        let (t3, k3, c3) = sync_sessions(&backend, &projects, false).await.unwrap();
        assert_eq!((t3, k3, c3), (0, 2, 0));
    }

    #[tokio::test]
    async fn test_stateless_config_change_triggers_replace() {
        let backend = MockMutagen::new();

        let project_v1 = make_project("service", "docker://v1", 0, "/root");
        let projects_v1 = vec![project_v1];

        // Erster Sync
        let (_, _, c1) = sync_sessions(&backend, &projects_v1, false).await.unwrap();
        assert_eq!(c1, 1);

        // Config ändert sich (neues Target)
        let project_v2 = make_project("service", "docker://v2", 0, "/root");
        let projects_v2 = vec![project_v2];

        // Zweiter Sync: Alte Session wird ersetzt
        let (t2, k2, c2) = sync_sessions(&backend, &projects_v2, false).await.unwrap();
        assert_eq!(t2, 1); // v1 terminiert
        assert_eq!(k2, 0);
        assert_eq!(c2, 1); // v2 erstellt
    }

    // ========================================================================
    // Tests: no_watch Parameter
    // ========================================================================

    #[tokio::test]
    async fn test_sync_passes_no_watch_flag() {
        let backend = MockMutagen::new();
        let project = make_project("test", "docker://test", 0, "/root");

        // Sync mit no_watch=true
        let projects = vec![&project];
        sync_stage_sessions(&backend, &projects, true).await.unwrap();

        let calls = backend.created_sessions();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1); // no_watch == true

        // Neues Backend für zweiten Test
        let backend2 = MockMutagen::new();
        let project2 = make_project("test2", "docker://test2", 0, "/root");
        let projects2 = vec![&project2];

        // Sync mit no_watch=false
        sync_stage_sessions(&backend2, &projects2, false).await.unwrap();

        let calls2 = backend2.created_sessions();
        assert_eq!(calls2.len(), 1);
        assert!(!calls2[0].1); // no_watch == false
    }

    // ========================================================================
    // Tests: Edge Cases
    // ========================================================================

    #[tokio::test]
    async fn test_sync_empty_projects() {
        let backend = MockMutagen::new();
        backend.add_session(mock_session("orphan-session"));

        let projects: Vec<DiscoveredProject> = vec![];
        let (terminated, kept, created) = sync_sessions(&backend, &projects, false).await.unwrap();

        // Leere Projektliste ändert nichts
        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 0);
    }

    #[tokio::test]
    async fn test_sync_stage_empty_projects() {
        let backend = MockMutagen::new();
        let projects: Vec<&DiscoveredProject> = vec![];

        let (terminated, kept, created) = sync_stage_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 0);
    }

    #[tokio::test]
    async fn test_sync_ignores_sessions_from_other_roots() {
        let backend = MockMutagen::new();

        // Session von einem anderen Root
        let other_root_project = make_project("service", "docker://app", 0, "/other-root");
        backend.add_session(make_session_for_project(&other_root_project));

        // Sync für /root
        let project = make_project("service", "docker://app", 0, "/root");
        let projects = vec![project];

        let (terminated, kept, created) = sync_sessions(&backend, &projects, false).await.unwrap();

        // Session von other-root sollte nicht betroffen sein
        assert_eq!(terminated, 0);
        assert_eq!(kept, 0);
        assert_eq!(created, 1);

        // Beide Sessions existieren jetzt
        assert_eq!(backend.list_sessions().await.len(), 2);
    }
}
