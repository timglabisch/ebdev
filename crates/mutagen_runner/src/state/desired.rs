//! Desired State - Was laut Config existieren sollte
//!
//! Der DesiredState wird aus der Projekt-Konfiguration berechnet und repräsentiert
//! den Soll-Zustand des Systems.

use std::path::PathBuf;

use crate::config::{PermissionsConfig, PollingConfig, SyncMode};

/// Der gewünschte Zustand aller Mutagen-Sessions.
#[derive(Debug, Clone, Default)]
pub struct DesiredState {
    /// Alle gewünschten Sessions
    pub sessions: Vec<DesiredSession>,
    /// CRC32 des Root-Pfads (für Session-Namensgebung)
    pub root_crc32: u32,
}

impl DesiredState {
    /// Erstellt einen DesiredState direkt aus DesiredSessions.
    pub fn from_sessions(sessions: Vec<DesiredSession>, project_crc32: u32) -> Self {
        Self {
            sessions,
            root_crc32: project_crc32,
        }
    }
}

/// Eine gewünschte Mutagen-Session.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredSession {
    /// Session-Name (inkl. CRC32-Suffix, z.B. "frontend-abc12345")
    pub name: String,
    /// Projekt-Name ohne CRC32
    pub project_name: String,
    /// Lokaler Pfad (Alpha)
    pub alpha: PathBuf,
    /// Remote Target (Beta)
    pub beta: String,
    /// Sync-Modus
    pub mode: SyncMode,
    /// Ignore-Patterns
    pub ignore: Vec<String>,
    /// Polling-Konfiguration
    pub polling: PollingConfig,
    /// Permissions-Konfiguration
    pub permissions: PermissionsConfig,
}

impl DesiredSession {
    /// Erstellt eine DesiredSession direkt aus Parametern.
    pub fn new(
        session_name: String,
        project_name: String,
        alpha: PathBuf,
        beta: String,
        mode: SyncMode,
        ignore: Vec<String>,
    ) -> Self {
        Self {
            name: session_name,
            project_name,
            alpha,
            beta,
            mode,
            ignore,
            polling: PollingConfig::default(),
            permissions: PermissionsConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desired_session_new() {
        let session = DesiredSession::new(
            "test-12345678".to_string(),
            "test".to_string(),
            PathBuf::from("/test"),
            "docker://container/path".to_string(),
            SyncMode::TwoWay,
            vec!["node_modules".to_string()],
        );

        assert_eq!(session.name, "test-12345678");
        assert_eq!(session.project_name, "test");
        assert_eq!(session.beta, "docker://container/path");
        assert_eq!(session.mode, SyncMode::TwoWay);
        assert_eq!(session.ignore, vec!["node_modules"]);
    }

    #[test]
    fn test_desired_state_from_sessions() {
        let session = DesiredSession::new(
            "test-12345678".to_string(),
            "test".to_string(),
            PathBuf::from("/test"),
            "docker://container/path".to_string(),
            SyncMode::TwoWay,
            vec![],
        );

        let state = DesiredState::from_sessions(vec![session], 0x12345678);

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.root_crc32, 0x12345678);
    }

}
