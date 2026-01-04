use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use async_trait::async_trait;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::mpsc;

use ebdev_mutagen_config::{DiscoveredProject, SyncMode};

pub mod status;
pub mod tui;

// Operator Pattern Module
pub mod state;
pub mod reconcile;
pub mod controller;

use status::MutagenSession;
use tui::TuiMessage;

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
}

/// Options for controlling which stages to run
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Run init stages (0..N-1)
    pub run_init_stages: bool,
    /// Run the final sync stage
    pub run_final_stage: bool,
    /// Stay in watch mode after sync completes (only with run_final_stage)
    pub keep_open: bool,
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

    /// Erstellt eine neue Sync-Session aus einer DesiredSession
    async fn create_session_from_desired(
        &self,
        session: &state::DesiredSession,
        no_watch: bool,
    ) -> Result<String, MutagenRunnerError>;

    /// Terminiert eine Session anhand ihrer ID
    async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError>;

    /// Terminiert alle Sessions
    async fn terminate_all(&self) -> Result<(), MutagenRunnerError>;

    /// Spawnt einen Status-Poller für die TUI (optional)
    /// Gibt None zurück wenn nicht unterstützt (z.B. bei Mocks)
    fn spawn_status_poller(&self, tx: mpsc::Sender<TuiMessage>) -> Option<tokio::task::JoinHandle<()>> {
        let _ = tx;
        None
    }
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

    async fn create_session_from_desired(
        &self,
        session: &state::DesiredSession,
        no_watch: bool,
    ) -> Result<String, MutagenRunnerError> {
        let alpha = session.alpha.to_string_lossy().to_string();

        let mut args = vec![
            "sync".to_string(),
            "create".to_string(),
            alpha,
            session.beta.clone(),
            format!("--name={}", session.name),
        ];

        let mode_str = match session.mode {
            SyncMode::TwoWay => "two-way-safe",
            SyncMode::OneWayCreate => "one-way-safe",
            SyncMode::OneWayReplica => "one-way-replica",
        };
        args.push(format!("--sync-mode={}", mode_str));

        for pattern in &session.ignore {
            args.push(format!("--ignore={}", pattern));
        }

        if no_watch {
            args.push("--watch-mode=no-watch".to_string());
        } else if session.polling.enabled {
            args.push("--watch-mode=force-poll".to_string());
            args.push(format!("--watch-polling-interval={}", session.polling.interval));
        }

        // Add permissions configuration
        args.push(format!("--default-file-mode={:o}", session.permissions.default_file_mode));
        args.push(format!("--default-directory-mode={:o}", session.permissions.default_directory_mode));

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
                session.name, stderr
            )));
        }

        Ok(session.name.clone())
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

    async fn terminate_all(&self) -> Result<(), MutagenRunnerError> {
        let output = Command::new(&self.bin_path)
            .args(["sync", "terminate", "--all"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no sessions") {
                return Err(MutagenRunnerError::CommandFailed(format!(
                    "Failed to terminate all sessions: {}", stderr
                )));
            }
        }

        Ok(())
    }

    fn spawn_status_poller(&self, tx: mpsc::Sender<TuiMessage>) -> Option<tokio::task::JoinHandle<()>> {
        Some(tui::spawn_status_poller(self.bin_path.clone(), tx))
    }
}


// ============================================================================
// SyncUI Trait - abstrahiert UI für Headless und TUI
// ============================================================================

/// Trait für UI-Interaktionen während des Sync-Prozesses.
/// Ermöglicht einheitliche Logik für Headless und TUI.
pub trait SyncUI {
    /// Wird aufgerufen wenn der Sync startet
    fn on_start(&mut self, stage_count: usize, init_only: bool);

    /// Wird aufgerufen wenn Sessions für gelöschte Projekte entfernt wurden
    fn on_cleanup(&mut self, removed: usize);

    /// Wird aufgerufen wenn eine Stage beginnt
    fn on_stage_start(&mut self, stage: i32, session_count: usize, is_final: bool);

    /// Wird aufgerufen nach dem Sync einer Stage
    fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize);

    /// Wird aufgerufen während auf Sync-Completion gewartet wird
    fn on_waiting(&mut self, stage: i32);

    /// Wird aufgerufen wenn eine Stage abgeschlossen ist
    fn on_stage_complete(&mut self, stage: i32, is_final: bool);

    /// Wird aufgerufen wenn Watch-Mode beginnt
    fn on_watch_mode(&mut self);

    /// Wird aufgerufen bei Fehlern
    fn on_error(&mut self, _msg: &str) {}

    /// Wird aufgerufen am Ende (nur bei init_only)
    fn on_complete(&mut self);

    /// Prüft ob der Benutzer abbrechen möchte
    fn check_quit(&mut self) -> Result<bool, MutagenRunnerError>;

    /// Wird in Loops aufgerufen (TUI: draw + events, Headless: sleep)
    fn tick(&mut self) -> Result<(), MutagenRunnerError>;

    /// Setzt Sessions für eine Stage (für TUI relevant)
    fn set_stage_sessions(&mut self, _stage_idx: usize, _sessions: &[&state::DesiredSession], _is_final: bool) {}

    /// Markiert Sessions als erstellt (nur für TUI relevant)
    fn mark_sessions_created(&mut self, _session_names: &[String]) {}
}

// ============================================================================
// HeadlessUI - Einfache println!-basierte Ausgabe
// ============================================================================

/// Headless UI implementation using println!
pub struct HeadlessUI;

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

    fn on_stage_start(&mut self, stage: i32, session_count: usize, is_final: bool) {
        println!("\n=== Stage {} ({} session(s)){} ===",
            stage,
            session_count,
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

    fn on_stage_complete(&mut self, _stage: i32, is_final: bool) {
        if is_final {
            println!("  Sync complete.");
        } else {
            println!("  Sync complete, terminating stage sessions...");
        }
    }

    fn on_watch_mode(&mut self) {
        println!("\nWatching for changes. Press Ctrl+C to stop (or 'q'/ESC in TUI mode).");
    }

    fn on_error(&mut self, msg: &str) {
        eprintln!("  Error: {}", msg);
    }

    fn on_complete(&mut self) {
        // Nothing to print - "Sync completed successfully." is printed in the caller
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
// Public API - All functions now use the Operator Pattern Controller
// ============================================================================

/// Terminates all mutagen sessions.
pub async fn terminate_all_sessions(mutagen_bin: &Path) -> Result<(), MutagenRunnerError> {
    let backend = RealMutagen::new(mutagen_bin.to_path_buf());
    backend.terminate_all().await
}

/// Runs staged mutagen sync using the Operator Pattern Controller.
///
/// # Arguments
/// * `backend` - The Mutagen backend (real or mock)
/// * `projects` - List of discovered projects
/// * `options` - Sync options (init_stages, final_stage, keep_open)
/// * `ui` - The UI implementation
pub async fn run_sync<B: MutagenBackend + 'static, U: SyncUI>(
    backend: Arc<B>,
    projects: Vec<DiscoveredProject>,
    options: SyncOptions,
    mut ui: U,
) -> Result<(), MutagenRunnerError> {
    let desired = state::DesiredState::from_projects(&projects);

    if desired.sessions.is_empty() {
        return Ok(());
    }

    let init_only = options.run_init_stages && !options.run_final_stage;

    // Calculate actual stages to run based on options
    let stages_to_run_count = match (options.run_init_stages, options.run_final_stage) {
        (true, true) => desired.stages.len(),
        (true, false) => desired.stages.len().saturating_sub(1),
        (false, true) => 1.min(desired.stages.len()),
        (false, false) => 0,
    };
    ui.on_start(stages_to_run_count, init_only);

    let controller = controller::SyncController::new(backend, desired, options, ui);
    controller.run().await
}

/// Convenience function: runs sync with HeadlessUI
pub async fn run_sync_headless<B: MutagenBackend + 'static>(
    backend: Arc<B>,
    projects: Vec<DiscoveredProject>,
    options: SyncOptions,
) -> Result<(), MutagenRunnerError> {
    run_sync(backend, projects, options, HeadlessUI).await
}

// ============================================================================
// TuiUI - Ratatui-based TUI implementation of SyncUI
// ============================================================================

/// TUI implementation using Ratatui
pub struct TuiUI {
    terminal: tui::Tui,
    app: tui::App,
}

impl TuiUI {
    pub fn new(total_stages: i32) -> Result<Self, MutagenRunnerError> {
        let terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
        Ok(Self {
            terminal,
            app: tui::App::new(total_stages),
        })
    }
}

impl Drop for TuiUI {
    fn drop(&mut self) {
        let _ = tui::restore();
    }
}

impl SyncUI for TuiUI {
    fn on_start(&mut self, _stage_count: usize, _init_only: bool) {
        // App already initialized with total_stages
    }

    fn on_cleanup(&mut self, _removed: usize) {
        // Could show a message, but not critical
    }

    fn on_stage_start(&mut self, stage: i32, _session_count: usize, is_final: bool) {
        self.app.current_stage = stage;
        self.app.is_final_stage = is_final;
    }

    fn on_sync_result(&mut self, _terminated: usize, _kept: usize, _created: usize) {
        // Status is shown in the table
    }

    fn on_waiting(&mut self, _stage: i32) {
        // Status is shown in the table
    }

    fn on_stage_complete(&mut self, _stage: i32, is_final: bool) {
        if is_final {
            self.app.set_message("Sync complete.".to_string());
        }
    }

    fn on_watch_mode(&mut self) {
        self.app.set_message("Watching for changes. Press 'q' or ESC to quit.".to_string());
    }

    fn on_error(&mut self, msg: &str) {
        self.app.set_message(format!("Error: {}", msg));
    }

    fn on_complete(&mut self) {
        self.app.set_message("Complete.".to_string());
    }

    fn check_quit(&mut self) -> Result<bool, MutagenRunnerError> {
        tui::handle_events().map_err(MutagenRunnerError::Execution)
    }

    fn tick(&mut self) -> Result<(), MutagenRunnerError> {
        self.terminal.draw(|frame| tui::draw(frame, &self.app))
            .map_err(MutagenRunnerError::Execution)?;
        Ok(())
    }

    fn set_stage_sessions(&mut self, _stage_idx: usize, sessions: &[&state::DesiredSession], is_final: bool) {
        let session_states: Vec<tui::SessionState> = sessions.iter().map(|s| {
            tui::SessionState {
                name: s.name.clone(),
                alpha: s.alpha.to_string_lossy().to_string(),
                beta: s.beta.clone(),
                status: None,
                created: false,
            }
        }).collect();
        self.app.set_stage(self.app.current_stage, session_states, is_final);
    }

    fn mark_sessions_created(&mut self, session_names: &[String]) {
        for name in session_names {
            self.app.mark_session_created(name);
        }
    }
}

/// Convenience function: runs sync with TuiUI
pub async fn run_sync_tui<B: MutagenBackend + 'static>(
    backend: Arc<B>,
    projects: Vec<DiscoveredProject>,
    options: SyncOptions,
) -> Result<(), MutagenRunnerError> {
    let stage_count = state::DesiredState::from_projects(&projects).stages.len() as i32;
    let ui = TuiUI::new(stage_count)?;
    run_sync(backend, projects, options, ui).await
}



// ============================================================================
// Core Session Management (public für Tests)
// ============================================================================

/// Entfernt Sessions für Projekte die nicht mehr in der Config existieren
pub async fn cleanup_removed_projects(
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

/// Synchronisiert Sessions für eine Stage
pub async fn sync_stage_sessions(
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

/// Vollständiger Sync: Cleanup + Stage Sync
pub async fn sync_sessions(
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

// ============================================================================
// Test Utilities - exportiert für Integrationstests
// ============================================================================

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use std::sync::Mutex;

    /// Mock UI für Tests - zeichnet alle Events auf
    #[derive(Default)]
    pub struct MockUI {
        pub events: Vec<String>,
        pub quit_after: Option<usize>,
        pub tick_count: usize,
        pub sessions_complete: bool,
    }

    impl MockUI {
        pub fn new() -> Self {
            Self::default()
        }

        /// Konfiguriert die UI so dass sie nach N ticks quit zurückgibt
        pub fn quit_after(mut self, ticks: usize) -> Self {
            self.quit_after = Some(ticks);
            self
        }

        /// Markiert alle Sessions als complete
        pub fn with_sessions_complete(mut self) -> Self {
            self.sessions_complete = true;
            self
        }
    }

    impl SyncUI for MockUI {
        fn on_start(&mut self, stage_count: usize, init_only: bool) {
            self.events.push(format!("start:{}:{}", stage_count, init_only));
        }
        fn on_cleanup(&mut self, removed: usize) {
            self.events.push(format!("cleanup:{}", removed));
        }
        fn on_stage_start(&mut self, stage: i32, session_count: usize, is_final: bool) {
            self.events.push(format!("stage_start:{}:{}:{}", stage, session_count, is_final));
        }
        fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize) {
            self.events.push(format!("sync_result:{}:{}:{}", terminated, kept, created));
        }
        fn on_waiting(&mut self, stage: i32) {
            self.events.push(format!("waiting:{}", stage));
        }
        fn on_stage_complete(&mut self, stage: i32, is_final: bool) {
            self.events.push(format!("stage_complete:{}:{}", stage, is_final));
        }
        fn on_watch_mode(&mut self) {
            self.events.push("watch_mode".to_string());
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
        create_pending_sessions: bool,
    }

    impl MockMutagen {
        pub fn new() -> Self {
            Self::default()
        }

        /// Erstellt einen MockMutagen der pending (nicht-complete) Sessions erstellt
        pub fn with_pending_sessions() -> Self {
            Self {
                create_pending_sessions: true,
                ..Self::default()
            }
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

    /// Erstellt einen Mock-Endpoint für Tests
    pub fn mock_endpoint(path: &str) -> status::EndpointStatus {
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

    /// Erstellt eine Mock-Session für Tests
    pub fn mock_session(name: &str) -> MutagenSession {
        MutagenSession {
            identifier: format!("mock-{}", name),
            name: name.to_string(),
            status: "watching".to_string(),
            successful_cycles: 0,
            alpha: mock_endpoint("/test"),
            beta: mock_endpoint("/target"),
        }
    }

    /// Erstellt eine Mock-Session für ein Projekt
    pub fn mock_session_for_project(project: &DiscoveredProject) -> MutagenSession {
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

    /// Erstellt ein Test-Projekt
    pub fn make_project(name: &str, target: &str, stage: i32, root: &str) -> DiscoveredProject {
        use ebdev_mutagen_config::{MutagenSyncProject, PermissionsConfig, PollingConfig};

        DiscoveredProject {
            project: MutagenSyncProject {
                name: name.to_string(),
                target: target.to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage,
                ignore: vec![],
                polling: PollingConfig::default(),
                permissions: PermissionsConfig::default(),
            },
            resolved_directory: PathBuf::from(format!("{}/{}", root, name)),
            config_path: PathBuf::from(format!("{}/.ebdev.ts", root)),
            root_config_path: PathBuf::from(format!("{}/.ebdev.ts", root)),
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
            let mut session = mock_session(&session_name);
            if self.create_pending_sessions {
                session.status = "connecting".to_string();
            }
            self.sessions.lock().unwrap().push(session);

            Ok(session_name)
        }

        async fn create_session_from_desired(
            &self,
            desired: &state::DesiredSession,
            no_watch: bool,
        ) -> Result<String, MutagenRunnerError> {
            let session_name = desired.name.clone();
            self.create_calls.lock().unwrap().push((session_name.clone(), no_watch));

            // Füge die Session auch zur Liste hinzu
            let mut session = mock_session(&session_name);
            if self.create_pending_sessions {
                session.status = "connecting".to_string();
            }
            self.sessions.lock().unwrap().push(session);

            Ok(session_name)
        }

        async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
            self.terminate_calls.lock().unwrap().push(session_id.to_string());
            // Entferne die Session aus der Liste
            self.sessions.lock().unwrap().retain(|s| s.identifier != session_id);
            Ok(())
        }

        async fn terminate_all(&self) -> Result<(), MutagenRunnerError> {
            self.sessions.lock().unwrap().clear();
            Ok(())
        }
    }

    // ========================================================================
    // Generische Test-Szenarien - können mit Mock oder Real Backend verwendet werden
    // ========================================================================

    /// Ergebnis eines Test-Szenarios
    #[derive(Debug)]
    pub struct ScenarioResult {
        pub terminated: usize,
        pub kept: usize,
        pub created: usize,
    }

    /// Szenario: Neue Session wird erstellt wenn keine existiert
    pub async fn scenario_creates_new_session<B: MutagenBackend>(
        backend: &B,
        project: &DiscoveredProject,
    ) -> Result<ScenarioResult, MutagenRunnerError> {
        let projects = vec![project];
        let (terminated, kept, created) = sync_stage_sessions(backend, &projects, true).await?;
        Ok(ScenarioResult { terminated, kept, created })
    }

    /// Szenario: Bestehende Session wird behalten (stateless)
    pub async fn scenario_keeps_existing_session<B: MutagenBackend>(
        backend: &B,
        project: &DiscoveredProject,
    ) -> Result<ScenarioResult, MutagenRunnerError> {
        let projects = vec![project];

        // Erste Sync - erstellt Session
        sync_stage_sessions(backend, &projects, true).await?;

        // Zweite Sync - sollte Session behalten
        let (terminated, kept, created) = sync_stage_sessions(backend, &projects, true).await?;
        Ok(ScenarioResult { terminated, kept, created })
    }

    /// Szenario: Session wird ersetzt wenn Config sich ändert
    pub async fn scenario_replaces_on_config_change<B: MutagenBackend>(
        backend: &B,
        old_project: &DiscoveredProject,
        new_project: &DiscoveredProject,
    ) -> Result<ScenarioResult, MutagenRunnerError> {
        // Erstelle Session mit alter Config
        let old_projects = vec![old_project];
        sync_stage_sessions(backend, &old_projects, true).await?;

        // Sync mit neuer Config - sollte ersetzen
        let new_projects = vec![new_project];
        let (terminated, kept, created) = sync_stage_sessions(backend, &new_projects, true).await?;
        Ok(ScenarioResult { terminated, kept, created })
    }

    /// Szenario: Session für gelöschtes Projekt wird entfernt
    pub async fn scenario_cleanup_deleted_project<B: MutagenBackend>(
        backend: &B,
        kept_project: &DiscoveredProject,
        deleted_project: &DiscoveredProject,
    ) -> Result<usize, MutagenRunnerError> {
        // Erstelle beide Sessions
        let all_projects = vec![kept_project, deleted_project];
        sync_stage_sessions(backend, &all_projects, true).await?;

        // Cleanup mit nur einem Projekt
        let remaining = vec![kept_project.clone()];
        let removed = cleanup_removed_projects(backend, &remaining).await?;
        Ok(removed)
    }

    /// Szenario: Mehrere Sessions in einer Stage
    pub async fn scenario_multiple_sessions_same_stage<B: MutagenBackend>(
        backend: &B,
        projects: &[&DiscoveredProject],
    ) -> Result<ScenarioResult, MutagenRunnerError> {
        let (terminated, kept, created) = sync_stage_sessions(backend, projects, true).await?;
        Ok(ScenarioResult { terminated, kept, created })
    }

    /// Szenario: Stateless über mehrere Syncs
    pub async fn scenario_stateless_multiple_syncs<B: MutagenBackend>(
        backend: &B,
        project: &DiscoveredProject,
    ) -> Result<(ScenarioResult, ScenarioResult, ScenarioResult), MutagenRunnerError> {
        let projects = vec![project];

        // Erster Sync
        let (t1, k1, c1) = sync_stage_sessions(backend, &projects, true).await?;
        let r1 = ScenarioResult { terminated: t1, kept: k1, created: c1 };

        // Zweiter Sync
        let (t2, k2, c2) = sync_stage_sessions(backend, &projects, true).await?;
        let r2 = ScenarioResult { terminated: t2, kept: k2, created: c2 };

        // Dritter Sync
        let (t3, k3, c3) = sync_stage_sessions(backend, &projects, true).await?;
        let r3 = ScenarioResult { terminated: t3, kept: k3, created: c3 };

        Ok((r1, r2, r3))
    }

    /// Hilfsfunktion: Terminiert alle Sessions die mit einem Präfix beginnen
    pub async fn cleanup_sessions_with_prefix<B: MutagenBackend>(
        backend: &B,
        prefix: &str,
    ) -> Result<usize, MutagenRunnerError> {
        let sessions = backend.list_sessions().await;
        let mut terminated = 0;

        for session in sessions {
            if session.name.starts_with(prefix) {
                backend.terminate_session(&session.identifier).await?;
                terminated += 1;
            }
        }

        Ok(terminated)
    }

    /// Hilfsfunktion: Wartet bis eine Session "watching" oder "waiting-for-rescan" Status hat
    pub async fn wait_for_session_complete<B: MutagenBackend>(
        backend: &B,
        session_name: &str,
        timeout_ms: u64,
    ) -> Result<bool, MutagenRunnerError> {
        use std::time::{Duration, Instant};
        use tokio::time::sleep;

        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        loop {
            let sessions = backend.list_sessions().await;
            if let Some(session) = sessions.iter().find(|s| s.name == session_name) {
                if session.is_complete() {
                    return Ok(true);
                }
            }

            if start.elapsed() > timeout {
                return Ok(false);
            }

            sleep(Duration::from_millis(500)).await;
        }
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use test_utils::*;

    // ========================================================================
    // Tests: sync_stage_sessions - Config-Änderung (Session ersetzen)
    // Note: Basic create/keep tests are covered by controller and reconcile tests
    // ========================================================================

    #[tokio::test]
    async fn test_sync_stage_sessions_replaces_when_config_changes() {
        let backend = MockMutagen::new();

        // Altes Projekt mit altem Target
        let old_project = make_project("test", "docker://old-target", 0, "/root");
        backend.add_session(mock_session_for_project(&old_project));

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
        backend.add_session(mock_session_for_project(&old_project));

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
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));

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
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));

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
    // Tests: sync_sessions (Full Sync: Cleanup + Stage Sync)
    // Note: Basic create/keep tests are covered by controller tests
    // ========================================================================

    #[tokio::test]
    async fn test_sync_sessions_mixed_scenario() {
        let backend = MockMutagen::new();

        // Existierende Sessions: frontend (bleibt), old-service (wird entfernt)
        let frontend = make_project("frontend", "docker://app", 1, "/root");
        let old_service = make_project("old-service", "docker://app", 1, "/root");
        backend.add_session(mock_session_for_project(&frontend));
        backend.add_session(mock_session_for_project(&old_service));

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
    async fn test_sync_ignores_sessions_from_other_roots() {
        let backend = MockMutagen::new();

        // Session von einem anderen Root
        let other_root_project = make_project("service", "docker://app", 0, "/other-root");
        backend.add_session(mock_session_for_project(&other_root_project));

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


    // ========================================================================
    // Tests: Sync-Modi
    // ========================================================================

    #[tokio::test]
    async fn test_sync_mode_one_way_create() {
        use ebdev_mutagen_config::{MutagenSyncProject, PermissionsConfig, PollingConfig};

        let backend = MockMutagen::new();
        let project = DiscoveredProject {
            project: MutagenSyncProject {
                name: "test".to_string(),
                target: "docker://test".to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::OneWayCreate,
                stage: 0,
                ignore: vec![],
                polling: PollingConfig::default(),
                permissions: PermissionsConfig::default(),
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.ts"),
            root_config_path: PathBuf::from("/root/.ebdev.ts"),
        };

        let projects = vec![&project];
        sync_stage_sessions(&backend, &projects, false).await.unwrap();

        // Session wurde erstellt mit korrektem Mode
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_sync_mode_one_way_replica() {
        use ebdev_mutagen_config::{MutagenSyncProject, PermissionsConfig, PollingConfig};

        let backend = MockMutagen::new();
        let project = DiscoveredProject {
            project: MutagenSyncProject {
                name: "test".to_string(),
                target: "docker://test".to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::OneWayReplica,
                stage: 0,
                ignore: vec![],
                polling: PollingConfig::default(),
                permissions: PermissionsConfig::default(),
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.ts"),
            root_config_path: PathBuf::from("/root/.ebdev.ts"),
        };

        let projects = vec![&project];
        sync_stage_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(backend.created_sessions().len(), 1);
    }

    // ========================================================================
    // Tests: Polling-Konfiguration
    // ========================================================================

    #[tokio::test]
    async fn test_polling_config_enabled() {
        use ebdev_mutagen_config::{MutagenSyncProject, PermissionsConfig, PollingConfig};

        let backend = MockMutagen::new();
        let project = DiscoveredProject {
            project: MutagenSyncProject {
                name: "test".to_string(),
                target: "docker://test".to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage: 0,
                ignore: vec![],
                polling: PollingConfig {
                    enabled: true,
                    interval: 5,
                },
                permissions: PermissionsConfig::default(),
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.ts"),
            root_config_path: PathBuf::from("/root/.ebdev.ts"),
        };

        let projects = vec![&project];
        // Mit no_watch=false wird polling verwendet
        sync_stage_sessions(&backend, &projects, false).await.unwrap();

        assert_eq!(backend.created_sessions().len(), 1);
    }

    // ========================================================================
    // Tests: Ignore-Patterns
    // ========================================================================

    #[tokio::test]
    async fn test_ignore_patterns_passed_to_session() {
        use ebdev_mutagen_config::{MutagenSyncProject, PermissionsConfig, PollingConfig};

        let backend = MockMutagen::new();
        let project = DiscoveredProject {
            project: MutagenSyncProject {
                name: "test".to_string(),
                target: "docker://test".to_string(),
                directory: Some(PathBuf::from(".")),
                mode: SyncMode::TwoWay,
                stage: 0,
                ignore: vec![".git".to_string(), "node_modules".to_string(), "*.log".to_string()],
                polling: PollingConfig::default(),
                permissions: PermissionsConfig::default(),
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.ts"),
            root_config_path: PathBuf::from("/root/.ebdev.ts"),
        };

        let projects = vec![&project];
        sync_stage_sessions(&backend, &projects, true).await.unwrap();

        // Session wurde erstellt (ignore patterns werden an RealMutagen weitergegeben)
        assert_eq!(backend.created_sessions().len(), 1);
    }

    // ========================================================================
    // Tests: Edge Cases
    // ========================================================================

    #[tokio::test]
    async fn test_empty_projects_returns_early() {
        let backend = Arc::new(MockMutagen::new());
        let projects: Vec<DiscoveredProject> = vec![];
        let options = SyncOptions { run_init_stages: true, run_final_stage: true, keep_open: false };

        let ui = MockUI::new();
        let result = run_sync(backend, projects, options, ui).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_single_stage_with_init_only_returns_early() {
        let backend = Arc::new(MockMutagen::new());
        // Nur ein Projekt in Stage 0 - mit init_only wird nichts ausgeführt
        let project = make_project("test", "docker://test", 0, "/root");
        let projects = vec![project];
        let options = SyncOptions { run_init_stages: true, run_final_stage: false, keep_open: false };

        let ui = MockUI::new().with_sessions_complete();
        let result = run_sync(backend, projects, options, ui).await;

        // Sollte OK sein (kein Stage zum Ausführen)
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_user_abort_during_stage() {
        // Verwende pending sessions damit die Waiting-Loop nicht sofort beendet wird
        let backend = Arc::new(MockMutagen::with_pending_sessions());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p0, p1];
        let options = SyncOptions { run_init_stages: true, run_final_stage: true, keep_open: false };

        // Quit während waiting
        let ui = MockUI::new().quit_after(3);
        let result = run_sync(backend.clone(), projects, options, ui).await;

        assert!(matches!(result, Err(MutagenRunnerError::UserAborted)));
    }

    #[tokio::test]
    async fn test_multiple_projects_same_stage_all_created() {
        let backend = Arc::new(MockMutagen::new());
        let p1 = make_project("frontend", "docker://app", 0, "/root");
        let p2 = make_project("backend", "docker://app", 0, "/root");
        let p3 = make_project("shared", "docker://app", 0, "/root");
        let projects = vec![p1, p2, p3];
        let options = SyncOptions { run_init_stages: true, run_final_stage: true, keep_open: false };

        let ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_sync(backend.clone(), projects, options, ui).await;

        assert!(result.is_ok());

        // Alle 3 Sessions sollten erstellt worden sein
        assert_eq!(backend.created_sessions().len(), 3);
    }

}
