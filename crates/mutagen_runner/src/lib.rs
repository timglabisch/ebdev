use std::path::{Path, PathBuf};
use std::process::Stdio;
use async_trait::async_trait;
use thiserror::Error;
use tokio::process::Command;

pub mod config;

// Re-export config types for convenience
pub use config::{PermissionsConfig, PollingConfig, SyncMode};

pub mod status;
pub mod state;

use status::MutagenSession;

#[derive(Debug, Error)]
pub enum MutagenRunnerError {
    #[error("Failed to execute mutagen: {0}")]
    Execution(#[from] std::io::Error),

    #[error("Mutagen command failed: {0}")]
    CommandFailed(String),
}

// ============================================================================
// MutagenBackend Trait - abstrahiert Mutagen-Interaktion für Tests
// ============================================================================

/// Trait für Mutagen-Operationen.
/// Ermöglicht Mocking für Tests.
#[async_trait]
pub trait MutagenBackend: Send + Sync {
    /// Listet alle aktuellen Mutagen-Sessions auf
    async fn list_sessions(&self) -> Result<Vec<MutagenSession>, MutagenRunnerError>;

    /// Erstellt eine neue Sync-Session aus einer DesiredSession
    async fn create_session_from_desired(
        &self,
        session: &state::DesiredSession,
        no_watch: bool,
    ) -> Result<String, MutagenRunnerError>;

    /// Terminiert eine Session anhand ihrer ID
    async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError>;

    /// Pausiert eine Session (stoppt Sync sofort)
    async fn pause_session(&self, session_id: &str) -> Result<(), MutagenRunnerError>;

    /// Setzt eine pausierte Session fort
    async fn resume_session(&self, session_id: &str) -> Result<(), MutagenRunnerError>;
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

/// Builds the command line arguments for creating a mutagen sync session.
/// Extracted for testability.
pub fn build_create_args(session: &state::DesiredSession, no_watch: bool) -> Vec<String> {
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

    // Always exclude the .ebdev toolchain directory
    args.push("--ignore=.ebdev".to_string());

    for pattern in &session.ignore {
        args.push(format!("--ignore={}", pattern));
    }

    if no_watch {
        args.push("--watch-mode=no-watch".to_string());
    } else if session.polling.enabled {
        args.push("--watch-mode=force-poll".to_string());
        args.push(format!("--watch-polling-interval={}", session.polling.interval));
    }

    args.push(format!("--default-file-mode={:o}", session.permissions.default_file_mode));
    args.push(format!("--default-directory-mode={:o}", session.permissions.default_directory_mode));

    args
}

#[async_trait]
impl MutagenBackend for RealMutagen {
    async fn list_sessions(&self) -> Result<Vec<MutagenSession>, MutagenRunnerError> {
        let output = Command::new(&self.bin_path)
            .args(["sync", "list", "--template", "{{json .}}"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // "no sessions" is not an error, just means empty list
            if stderr.contains("no sessions") || stderr.contains("unable to locate") {
                return Ok(Vec::new());
            }
            return Err(MutagenRunnerError::CommandFailed(format!(
                "Failed to list sessions: {}", stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(serde_json::from_str(&stdout).unwrap_or_default())
    }

    async fn create_session_from_desired(
        &self,
        session: &state::DesiredSession,
        no_watch: bool,
    ) -> Result<String, MutagenRunnerError> {
        let args = build_create_args(session, no_watch);

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

    async fn pause_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
        let output = Command::new(&self.bin_path)
            .args(["sync", "pause", session_id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no sessions") && !stderr.contains("not found") {
                return Err(MutagenRunnerError::CommandFailed(format!(
                    "Failed to pause session {}: {}",
                    session_id, stderr
                )));
            }
        }

        Ok(())
    }

    async fn resume_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
        let output = Command::new(&self.bin_path)
            .args(["sync", "resume", session_id])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no sessions") && !stderr.contains("not found") {
                return Err(MutagenRunnerError::CommandFailed(format!(
                    "Failed to resume session {}: {}",
                    session_id, stderr
                )));
            }
        }

        Ok(())
    }
}

// ============================================================================
// Reconcile API
// ============================================================================

/// Session status for status callbacks
#[derive(Debug, Clone)]
pub struct SessionStatusInfo {
    pub name: String,
    pub status: String,
}

/// Pauses all mutagen sessions belonging to a project.
///
/// This is a safety function that can be called independently of reconcile,
/// e.g. on SIGINT or before any Docker operations that might remove volumes.
/// Sessions are identified by their CRC32 suffix in the session name.
pub async fn pause_project_sessions(
    mutagen_bin: &Path,
    project_crc32: u32,
) -> Result<usize, MutagenRunnerError> {
    if !mutagen_bin.exists() {
        return Ok(0);
    }

    let backend = RealMutagen::new(mutagen_bin.to_path_buf());
    let suffix = format!("{:08x}", project_crc32);

    let sessions = backend.list_sessions().await?;
    let mut paused = 0;
    for session in sessions.iter().filter(|s| s.name.ends_with(&suffix)) {
        if backend.pause_session(&session.identifier).await.is_ok() {
            paused += 1;
        }
    }

    Ok(paused)
}

/// Reconciles mutagen sessions to the desired state.
///
/// 1. Takes a list of DesiredSessions
/// 2. Runs the reconcile loop until all sessions are "watching"
/// 3. Calls the status_callback with current status on each iteration
///
/// Pass an empty sessions list to terminate all sessions for this project.
///
/// # Arguments
/// * `mutagen_bin` - Path to the mutagen binary
/// * `sessions` - List of desired sessions
/// * `project_crc32` - CRC32 of the project path (for session naming uniqueness)
/// * `status_callback` - Called with current session statuses on each iteration
pub async fn reconcile_sessions<F>(
    mutagen_bin: &Path,
    sessions: Vec<state::DesiredSession>,
    project_crc32: u32,
    mut status_callback: F,
) -> Result<(), MutagenRunnerError>
where
    F: FnMut(Vec<SessionStatusInfo>),
{
    // Validate binary exists before doing anything.
    // If the binary is missing and we can't list/pause sessions,
    // we must NOT proceed — existing sessions could sync empty state
    // back to local and delete files.
    if !mutagen_bin.exists() {
        return Err(MutagenRunnerError::CommandFailed(format!(
            "Mutagen binary not found at '{}'. Cannot safely manage sessions.",
            mutagen_bin.display()
        )));
    }

    let backend = RealMutagen::new(mutagen_bin.to_path_buf());
    let desired = state::DesiredState::from_sessions(sessions, project_crc32);
    let suffix = format!("{:08x}", project_crc32);

    // Empty sessions means cleanup all sessions for this project
    if desired.sessions.is_empty() {
        let current_sessions = backend.list_sessions().await?;
        let project_sessions: Vec<_> = current_sessions
            .iter()
            .filter(|s| s.name.ends_with(&suffix))
            .collect();

        // First: pause all sessions to stop syncing immediately.
        // This prevents mutagen from syncing empty state back to local
        // when containers/volumes are already gone.
        for session in &project_sessions {
            let _ = backend.pause_session(&session.identifier).await;
        }

        // Then: terminate all paused sessions
        for session in &project_sessions {
            let _ = backend.terminate_session(&session.identifier).await;
        }

        status_callback(vec![]);
        return Ok(());
    }

    // SAFETY: Pause ALL existing project sessions immediately.
    // This prevents data loss when:
    // - Process was interrupted (Ctrl+C) and restarted: old sessions may still
    //   be running while Docker containers are being recreated
    // - Docker containers are recreated: sessions might briefly see empty remote
    //   state and sync deletions back to local
    // Sessions that match the desired state will be resumed in the reconcile loop.
    {
        let current_sessions = backend.list_sessions().await?;
        for session in current_sessions.iter().filter(|s| s.name.ends_with(&suffix)) {
            let _ = backend.pause_session(&session.identifier).await;
        }
    }

    // Run the reconcile loop, but on ANY error, pause all project sessions
    // first to prevent data loss from syncing empty remote state.
    match reconcile_loop(&backend, &desired, &suffix, project_crc32, &mut status_callback).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // Safety: pause all project sessions before propagating error
            eprintln!("[mutagen] Error during reconcile, pausing all sessions for safety: {}", e);
            if let Ok(current_sessions) = backend.list_sessions().await {
                for session in current_sessions.iter().filter(|s| s.name.ends_with(&suffix)) {
                    let _ = backend.pause_session(&session.identifier).await;
                }
            }
            Err(e)
        }
    }
}

/// Internal reconcile loop, separated so errors can be caught and sessions paused.
async fn reconcile_loop<F>(
    backend: &RealMutagen,
    desired: &state::DesiredState,
    suffix: &str,
    _project_crc32: u32,
    status_callback: &mut F,
) -> Result<(), MutagenRunnerError>
where
    F: FnMut(Vec<SessionStatusInfo>),
{
    loop {
        // Get actual state
        let current_sessions = backend.list_sessions().await?;
        let actual = state::ActualState::from_mutagen_sessions(current_sessions.clone());

        // Build status info for callback
        let session_names: Vec<String> = desired.sessions.iter().map(|s| s.name.clone()).collect();
        let status_infos: Vec<SessionStatusInfo> = session_names
            .iter()
            .map(|name| {
                let status = actual
                    .find_by_name(name)
                    .map(|s| format!("{:?}", s.status).to_lowercase())
                    .unwrap_or_else(|| "pending".to_string());
                SessionStatusInfo {
                    name: name.clone(),
                    status,
                }
            })
            .collect();
        status_callback(status_infos);

        // Check if all sessions are complete
        if actual.all_complete(&session_names) {
            break;
        }

        // Reconcile: Create missing sessions, terminate changed ones
        for session in &desired.sessions {
            let prefix = format!("{}-", session.project_name);

            // Find existing sessions for this project
            let existing: Vec<_> = actual
                .sessions
                .iter()
                .filter(|s| s.name.starts_with(&prefix) && s.name.ends_with(suffix))
                .collect();

            let mut found_match = false;
            for existing_session in &existing {
                if existing_session.name == session.name {
                    // Perfect match - resume it (was paused at start for safety)
                    found_match = true;
                    let _ = backend.resume_session(&existing_session.identifier).await;
                } else {
                    // Config changed - terminate old session (already paused at start)
                    let _ = backend.terminate_session(&existing_session.identifier).await;
                }
            }

            if !found_match {
                // Create new session
                backend.create_session_from_desired(session, false).await?;
            }
        }

        // Small delay before next iteration
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    Ok(())
}

// ============================================================================
// Test Utilities
// ============================================================================

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;
    use std::sync::Mutex;

    /// Mock Mutagen Backend für Tests
    #[derive(Default)]
    pub struct MockMutagen {
        sessions: Mutex<Vec<MutagenSession>>,
        create_calls: Mutex<Vec<(String, bool)>>,
        terminate_calls: Mutex<Vec<String>>,
        pause_calls: Mutex<Vec<String>>,
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

        /// Prüft welche Sessions pausiert wurden
        pub fn paused_sessions(&self) -> Vec<String> {
            self.pause_calls.lock().unwrap().clone()
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

    #[async_trait]
    impl MutagenBackend for MockMutagen {
        async fn list_sessions(&self) -> Result<Vec<MutagenSession>, MutagenRunnerError> {
            Ok(self.sessions.lock().unwrap().clone())
        }

        async fn create_session_from_desired(
            &self,
            desired: &state::DesiredSession,
            no_watch: bool,
        ) -> Result<String, MutagenRunnerError> {
            let session_name = desired.name.clone();
            self.create_calls.lock().unwrap().push((session_name.clone(), no_watch));

            // Add the session to the list
            let session = mock_session(&session_name);
            self.sessions.lock().unwrap().push(session);

            Ok(session_name)
        }

        async fn terminate_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
            self.terminate_calls.lock().unwrap().push(session_id.to_string());
            // Remove the session from the list
            self.sessions.lock().unwrap().retain(|s| s.identifier != session_id);
            Ok(())
        }

        async fn pause_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
            self.pause_calls.lock().unwrap().push(session_id.to_string());
            Ok(())
        }

        async fn resume_session(&self, session_id: &str) -> Result<(), MutagenRunnerError> {
            // Mock: just track the call, no actual effect needed for tests
            let _ = session_id;
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

    #[tokio::test]
    async fn test_mock_backend_create_session() {
        let backend = MockMutagen::new();

        let session = state::DesiredSession::new(
            "test-12345678".to_string(),
            "test".to_string(),
            PathBuf::from("/test"),
            "docker://container/path".to_string(),
            SyncMode::TwoWay,
            vec![],
        );

        let result = backend.create_session_from_desired(&session, false).await;
        assert!(result.is_ok());
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_mock_backend_terminate_session() {
        let backend = MockMutagen::new();
        backend.add_session(mock_session("test-session"));

        let result = backend.terminate_session("mock-test-session").await;
        assert!(result.is_ok());
        assert_eq!(backend.terminated_sessions().len(), 1);
        assert_eq!(backend.list_sessions().await.unwrap().len(), 0);
    }
}
