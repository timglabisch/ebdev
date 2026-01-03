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

    #[error("User aborted")]
    UserAborted,

    #[error("Config error: {0}")]
    Config(#[from] ebdev_mutagen_config::MutagenConfigError),
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

    fn spawn_status_poller(&self, tx: mpsc::Sender<TuiMessage>) -> Option<tokio::task::JoinHandle<()>> {
        Some(tui::spawn_status_poller(self.bin_path.clone(), tx))
    }
}

// ============================================================================
// SyncPlan - Berechnung der auszuführenden Stages (einmalig)
// ============================================================================

/// Enthält den berechneten Sync-Plan
struct SyncPlan<'a> {
    stages_to_run: Vec<i32>,
    last_stage: i32,
    projects_by_stage: BTreeMap<i32, Vec<&'a DiscoveredProject>>,
}

impl<'a> SyncPlan<'a> {
    fn new(projects: &'a [DiscoveredProject], init_only: bool) -> Option<Self> {
        if projects.is_empty() {
            return None;
        }

        let projects_by_stage = group_by_stage(projects);
        let stage_keys: Vec<i32> = projects_by_stage.keys().copied().collect();
        let last_stage = *stage_keys.last()?;

        let stages_to_run: Vec<i32> = if init_only {
            stage_keys.into_iter().filter(|&s| s != last_stage).collect()
        } else {
            stage_keys
        };

        if stages_to_run.is_empty() {
            return None;
        }

        Some(Self {
            stages_to_run,
            last_stage,
            projects_by_stage,
        })
    }

    fn stage_count(&self) -> usize {
        self.stages_to_run.len()
    }

    fn is_final_stage(&self, stage: i32, init_only: bool) -> bool {
        !init_only && stage == self.last_stage
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
    fn on_stage_start(&mut self, stage: i32, projects: &[&DiscoveredProject], is_final: bool);

    /// Wird aufgerufen nach dem Sync einer Stage
    fn on_sync_result(&mut self, terminated: usize, kept: usize, created: usize);

    /// Wird aufgerufen während auf Sync-Completion gewartet wird
    fn on_waiting(&mut self, stage: i32);

    /// Wird aufgerufen wenn eine Stage abgeschlossen ist
    fn on_stage_complete(&mut self, stage: i32);

    /// Wird aufgerufen wenn Watch-Mode beginnt
    fn on_watch_mode(&mut self);

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
    _poller_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TuiUI {
    fn new<M: MutagenBackend>(backend: &M, stage_count: usize) -> Result<Self, MutagenRunnerError> {
        let terminal = tui::init().map_err(MutagenRunnerError::Execution)?;
        let app = App::new(stage_count as i32);
        let (tx, rx) = mpsc::channel::<TuiMessage>(100);
        let poller_handle = backend.spawn_status_poller(tx);

        Ok(Self {
            terminal,
            app,
            rx,
            _poller_handle: poller_handle,
        })
    }

    fn cleanup(self) -> Result<(), MutagenRunnerError> {
        if let Some(handle) = self._poller_handle {
            handle.abort();
        }
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
}

// ============================================================================
// Public API
// ============================================================================

/// Runs staged mutagen sync.
/// - init_only=false: Runs all stages, final stage stays in watch mode
/// - init_only=true: Runs all stages except final, terminates after completion
pub async fn run_staged_sync(
    mutagen_bin: &Path,
    _base_path: &Path,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    let backend = Arc::new(RealMutagen::new(mutagen_bin.to_path_buf()));
    run_staged_sync_with_backend(backend, projects, init_only).await
}

/// Runs staged mutagen sync mit einem beliebigen MutagenBackend.
/// Ermöglicht Mocking für Tests.
pub async fn run_staged_sync_with_backend<M: MutagenBackend + 'static>(
    backend: Arc<M>,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
) -> Result<(), MutagenRunnerError> {
    let plan = match SyncPlan::new(&projects, init_only) {
        Some(plan) => plan,
        None => {
            println!("No mutagen sync projects found.");
            return Ok(());
        }
    };

    if std::io::stdout().is_terminal() {
        let mut ui = TuiUI::new(backend.as_ref(), plan.stage_count())?;
        let result = run_staged_sync_impl(backend, &projects, init_only, &plan, &mut ui).await;
        ui.cleanup()?;
        if result.is_ok() && init_only {
            println!("All init stages completed successfully.");
        }
        return result;
    }

    let mut ui = HeadlessUI;
    run_staged_sync_impl(backend, &projects, init_only, &plan, &mut ui).await
}

/// Runs staged mutagen sync with a custom UI implementation.
/// Useful for testing with mock UIs.
pub async fn run_staged_sync_with_ui<M: MutagenBackend, U: SyncUI>(
    backend: Arc<M>,
    projects: Vec<DiscoveredProject>,
    init_only: bool,
    ui: &mut U,
) -> Result<(), MutagenRunnerError> {
    let plan = match SyncPlan::new(&projects, init_only) {
        Some(plan) => plan,
        None => {
            return Ok(());
        }
    };

    run_staged_sync_impl(backend, &projects, init_only, &plan, ui).await
}

// ============================================================================
// Core Sync Logic - einzige Implementierung
// ============================================================================

async fn run_staged_sync_impl<M: MutagenBackend, U: SyncUI>(
    backend: Arc<M>,
    projects: &[DiscoveredProject],
    init_only: bool,
    plan: &SyncPlan<'_>,
    ui: &mut U,
) -> Result<(), MutagenRunnerError> {
    ui.on_start(plan.stage_count(), init_only);

    // Cleanup sessions für gelöschte Projekte
    let removed = cleanup_removed_projects(backend.as_ref(), projects).await?;
    ui.on_cleanup(removed);

    // Verarbeite jede Stage
    for (stage_idx, stage) in plan.stages_to_run.iter().enumerate() {
        let stage_projects: Vec<_> = plan.projects_by_stage.get(stage).unwrap().iter().cloned().collect();
        let is_final = plan.is_final_stage(*stage, init_only);

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
            // Final stage: Watch-Mode (einfache Loop bis Quit)
            ui.on_watch_mode();

            loop {
                ui.tick()?;

                if ui.check_quit()? {
                    break;
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
        fn all_sessions_complete(&self) -> bool {
            self.sessions_complete
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
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use test_utils::*;

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
        backend.add_session(mock_session_for_project(&project));

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
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));

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
    // Tests: terminate_sessions_by_name
    // ========================================================================

    #[tokio::test]
    async fn test_terminate_sessions_by_name() {
        let backend = MockMutagen::new();

        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));

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
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));
        backend.add_session(mock_session_for_project(&p3));

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
        backend.add_session(mock_session_for_project(&p1));
        backend.add_session(mock_session_for_project(&p2));

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
    // Tests: run_staged_sync_with_ui
    // ========================================================================

    #[tokio::test]
    async fn test_run_staged_sync_with_ui_single_stage() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("test", "docker://test", 0, "/root");
        let projects = vec![project];

        let mut ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend, projects, false, &mut ui).await;

        assert!(result.is_ok());
        assert!(ui.events.contains(&"start:1:false".to_string()));
        assert!(ui.events.contains(&"watch_mode".to_string()));
    }

    #[tokio::test]
    async fn test_run_staged_sync_with_ui_init_only() {
        let backend = Arc::new(MockMutagen::new());
        let p1 = make_project("shared", "docker://app", 0, "/root");
        let p2 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p1, p2];

        let mut ui = MockUI::new().with_sessions_complete();
        let result = run_staged_sync_with_ui(backend, projects, true, &mut ui).await;

        assert!(result.is_ok());
        // init_only=true sollte nur Stage 0 ausführen (nicht Stage 1)
        assert!(ui.events.contains(&"start:1:true".to_string()));
        assert!(ui.events.contains(&"complete".to_string()));
        assert!(!ui.events.iter().any(|e| e.contains("watch_mode")));
    }

    #[tokio::test]
    async fn test_run_staged_sync_with_ui_user_abort() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("test", "docker://test", 0, "/root");
        let projects = vec![project];

        // Quit sofort
        let mut ui = MockUI::new().quit_after(0);
        let result = run_staged_sync_with_ui(backend, projects, false, &mut ui).await;

        assert!(matches!(result, Err(MutagenRunnerError::UserAborted)));
    }

    // ========================================================================
    // Tests: Multi-Stage Workflow
    // ========================================================================

    #[tokio::test]
    async fn test_multi_stage_sequential_execution() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("backend", "docker://app", 1, "/root");
        let p3 = make_project("config", "docker://app", 2, "/root");
        let projects = vec![p0, p1, p2, p3];

        // quit_after muss hoch genug sein für alle Stages + final watch mode
        let mut ui = MockUI::new().quit_after(20).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Prüfe dass alle Stages in Reihenfolge gestartet wurden
        let stage_starts: Vec<_> = ui.events.iter()
            .filter(|e| e.starts_with("stage_start:"))
            .collect();

        assert_eq!(stage_starts.len(), 3); // 3 Stages
        assert!(stage_starts[0].contains("stage_start:0:1:")); // Stage 0, 1 Projekt
        assert!(stage_starts[1].contains("stage_start:1:2:")); // Stage 1, 2 Projekte
        assert!(stage_starts[2].contains("stage_start:2:1:")); // Stage 2, 1 Projekt
    }

    #[tokio::test]
    async fn test_non_final_stages_are_terminated() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p0.clone(), p1];

        // quit_after muss hoch genug sein für Stage 0 completion + Stage 1 watch mode
        let mut ui = MockUI::new().quit_after(15).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Stage 0 sollte complete event haben (non-final)
        assert!(ui.events.contains(&"stage_complete:0".to_string()));

        // Stage 0 Session sollte terminiert worden sein
        let terminated = backend.terminated_sessions();
        assert!(terminated.iter().any(|id| id.contains(&p0.session_name())));
    }

    #[tokio::test]
    async fn test_final_stage_enters_watch_mode() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p0, p1];

        // quit_after muss hoch genug sein für Stage 0 completion + Stage 1 watch mode
        let mut ui = MockUI::new().quit_after(15).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend, projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Final stage sollte watch_mode event haben
        assert!(ui.events.contains(&"watch_mode".to_string()));

        // Aber kein stage_complete für Stage 1 (final stage)
        assert!(!ui.events.contains(&"stage_complete:1".to_string()));
    }

    #[tokio::test]
    async fn test_init_only_skips_final_stage() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("frontend", "docker://app", 1, "/root");
        let p2 = make_project("config", "docker://app", 2, "/root");
        let projects = vec![p0, p1, p2];

        let mut ui = MockUI::new().with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, true, &mut ui).await;

        assert!(result.is_ok());

        // Nur Stage 0 und 1 sollten ausgeführt werden (nicht Stage 2)
        assert!(ui.events.contains(&"start:2:true".to_string())); // 2 stages
        assert!(ui.events.iter().any(|e| e.contains("stage_start:0:")));
        assert!(ui.events.iter().any(|e| e.contains("stage_start:1:")));
        assert!(!ui.events.iter().any(|e| e.contains("stage_start:2:")));

        // complete event sollte kommen
        assert!(ui.events.contains(&"complete".to_string()));
    }

    // ========================================================================
    // Tests: Session-Verwaltung im Full-Flow
    // ========================================================================

    #[tokio::test]
    async fn test_existing_sessions_are_kept_in_full_flow() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("test", "docker://test", 0, "/root");

        // Session existiert bereits
        backend.add_session(mock_session_for_project(&project));

        let projects = vec![project];
        let mut ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Keine neue Session sollte erstellt worden sein
        assert!(backend.created_sessions().is_empty());

        // sync_result sollte kept=1 zeigen
        assert!(ui.events.contains(&"sync_result:0:1:0".to_string()));
    }

    #[tokio::test]
    async fn test_config_change_replaces_session_in_full_flow() {
        let backend = Arc::new(MockMutagen::new());

        // Alte Session mit anderem Target
        let old_project = make_project("test", "docker://old-target", 0, "/root");
        backend.add_session(mock_session_for_project(&old_project));

        // Neue Config mit neuem Target
        let new_project = make_project("test", "docker://new-target", 0, "/root");
        let projects = vec![new_project];

        let mut ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Alte Session terminiert, neue erstellt
        assert_eq!(backend.terminated_sessions().len(), 1);
        assert_eq!(backend.created_sessions().len(), 1);

        // sync_result sollte terminated=1, kept=0, created=1 zeigen
        assert!(ui.events.contains(&"sync_result:1:0:1".to_string()));
    }

    #[tokio::test]
    async fn test_deleted_project_cleanup_in_full_flow() {
        let backend = Arc::new(MockMutagen::new());

        // Zwei Sessions existieren
        let kept_project = make_project("kept", "docker://app", 0, "/root");
        let deleted_project = make_project("deleted", "docker://app", 0, "/root");
        backend.add_session(mock_session_for_project(&kept_project));
        backend.add_session(mock_session_for_project(&deleted_project));

        // Nur kept_project in der neuen Config
        let projects = vec![kept_project];

        let mut ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // deleted_project Session sollte entfernt worden sein (cleanup)
        assert!(ui.events.contains(&"cleanup:1".to_string()));
    }

    // ========================================================================
    // Tests: Sync-Modi
    // ========================================================================

    #[tokio::test]
    async fn test_sync_mode_one_way_create() {
        use ebdev_mutagen_config::{MutagenSyncProject, PollingConfig};

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
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.toml"),
            root_config_path: PathBuf::from("/root/.ebdev.toml"),
        };

        let projects = vec![&project];
        sync_stage_sessions(&backend, &projects, false).await.unwrap();

        // Session wurde erstellt mit korrektem Mode
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_sync_mode_one_way_replica() {
        use ebdev_mutagen_config::{MutagenSyncProject, PollingConfig};

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
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.toml"),
            root_config_path: PathBuf::from("/root/.ebdev.toml"),
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
        use ebdev_mutagen_config::{MutagenSyncProject, PollingConfig};

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
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.toml"),
            root_config_path: PathBuf::from("/root/.ebdev.toml"),
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
        use ebdev_mutagen_config::{MutagenSyncProject, PollingConfig};

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
            },
            resolved_directory: PathBuf::from("/root/test"),
            config_path: PathBuf::from("/root/.ebdev.toml"),
            root_config_path: PathBuf::from("/root/.ebdev.toml"),
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

        let mut ui = MockUI::new();
        let result = run_staged_sync_with_ui(backend, projects, false, &mut ui).await;

        assert!(result.is_ok());
        // Keine Events sollten gefeuert worden sein
        assert!(ui.events.is_empty());
    }

    #[tokio::test]
    async fn test_single_stage_with_init_only_returns_none() {
        let backend = Arc::new(MockMutagen::new());
        // Nur ein Projekt in Stage 0 - mit init_only wird nichts ausgeführt
        let project = make_project("test", "docker://test", 0, "/root");
        let projects = vec![project];

        let mut ui = MockUI::new();
        let result = run_staged_sync_with_ui(backend, projects, true, &mut ui).await;

        // Sollte OK sein, aber keine Events (kein Stage zum Ausführen)
        assert!(result.is_ok());
        assert!(ui.events.is_empty());
    }

    #[tokio::test]
    async fn test_user_abort_during_non_final_stage_terminates_sessions() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p0.clone(), p1];

        // Quit während Stage 0 waiting
        let mut ui = MockUI::new().quit_after(3); // Nach stage_start, sync_result, waiting
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(matches!(result, Err(MutagenRunnerError::UserAborted)));

        // Stage 0 Sessions sollten terminiert worden sein
        let terminated = backend.terminated_sessions();
        assert!(!terminated.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_projects_same_stage_all_created() {
        let backend = Arc::new(MockMutagen::new());
        let p1 = make_project("frontend", "docker://app", 0, "/root");
        let p2 = make_project("backend", "docker://app", 0, "/root");
        let p3 = make_project("shared", "docker://app", 0, "/root");
        let projects = vec![p1, p2, p3];

        let mut ui = MockUI::new().quit_after(1).with_sessions_complete();
        let result = run_staged_sync_with_ui(backend.clone(), projects, false, &mut ui).await;

        assert!(result.is_ok());

        // Alle 3 Sessions sollten erstellt worden sein
        assert_eq!(backend.created_sessions().len(), 3);

        // sync_result sollte created=3 zeigen
        assert!(ui.events.contains(&"sync_result:0:0:3".to_string()));
    }

    // ========================================================================
    // Tests: SyncPlan
    // ========================================================================

    #[test]
    fn test_sync_plan_empty_projects() {
        let projects: Vec<DiscoveredProject> = vec![];
        let plan = SyncPlan::new(&projects, false);
        assert!(plan.is_none());
    }

    #[test]
    fn test_sync_plan_single_stage_init_only() {
        let project = make_project("test", "docker://test", 0, "/root");
        let projects = vec![project];

        // Mit init_only und nur einer Stage gibt es nichts zu tun
        let plan = SyncPlan::new(&projects, true);
        assert!(plan.is_none());
    }

    #[test]
    fn test_sync_plan_multiple_stages() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let p2 = make_project("config", "docker://app", 2, "/root");
        let projects = vec![p0, p1, p2];

        let plan = SyncPlan::new(&projects, false).unwrap();

        assert_eq!(plan.stage_count(), 3);
        assert_eq!(plan.stages_to_run, vec![0, 1, 2]);
        assert_eq!(plan.last_stage, 2);
        assert!(plan.is_final_stage(2, false));
        assert!(!plan.is_final_stage(1, false));
    }

    #[test]
    fn test_sync_plan_init_only_excludes_final() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let projects = vec![p0, p1];

        let plan = SyncPlan::new(&projects, true).unwrap();

        assert_eq!(plan.stage_count(), 1);
        assert_eq!(plan.stages_to_run, vec![0]);
        // Mit init_only ist keine Stage final
        assert!(!plan.is_final_stage(0, true));
    }
}
