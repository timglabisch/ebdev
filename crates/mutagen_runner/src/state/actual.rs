//! Actual State - Was Mutagen tatsächlich sagt
//!
//! Der ActualState wird durch Abfrage des Mutagen-Backends ermittelt und
//! repräsentiert den Ist-Zustand des Systems.

use crate::status::MutagenSession;

/// Der tatsächliche Zustand aller Mutagen-Sessions.
///
/// Wird durch Abfrage des Mutagen-Backends ermittelt.
#[derive(Debug, Clone, Default)]
pub struct ActualState {
    /// Alle existierenden Sessions
    pub sessions: Vec<ActualSession>,
}

impl ActualState {
    /// Erstellt einen ActualState aus einer Liste von MutagenSessions.
    pub fn from_mutagen_sessions(sessions: Vec<MutagenSession>) -> Self {
        Self {
            sessions: sessions.into_iter().map(ActualSession::from).collect(),
        }
    }

    /// Findet eine Session anhand ihres Namens.
    pub fn find_by_name(&self, name: &str) -> Option<&ActualSession> {
        self.sessions.iter().find(|s| s.name == name)
    }

    /// Prüft ob alle angegebenen Sessions "complete" sind.
    pub fn all_complete(&self, names: &[String]) -> bool {
        names.iter().all(|name| {
            self.find_by_name(name)
                .map(|s| s.status.is_complete())
                .unwrap_or(true) // Nicht existierende Sessions gelten als "complete"
        })
    }
}

/// Eine existierende Mutagen-Session.
#[derive(Debug, Clone)]
pub struct ActualSession {
    /// Mutagen Session ID
    pub identifier: String,
    /// Session Name
    pub name: String,
    /// Status der Session
    pub status: SessionStatus,
    /// Alpha-Pfad (lokal)
    pub alpha: String,
    /// Beta-Pfad (remote)
    pub beta: String,
}

impl From<MutagenSession> for ActualSession {
    fn from(session: MutagenSession) -> Self {
        let beta = session.beta_display();
        Self {
            identifier: session.identifier,
            name: session.name,
            status: SessionStatus::from_str(&session.status),
            alpha: session.alpha.path,
            beta,
        }
    }
}

/// Status einer Mutagen-Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session ist nicht verbunden
    Disconnected,
    /// Session verbindet sich
    Connecting,
    /// Session scannt Dateien
    Scanning,
    /// Session synchronisiert aktiv
    Syncing,
    /// Session wartet auf Rescan
    WaitingForRescan,
    /// Session beobachtet Änderungen (idle, complete)
    Watching,
    /// Session ist angehalten
    Halted(String),
    /// Unbekannter Status
    Unknown(String),
}

impl SessionStatus {
    /// Parst einen Status-String von Mutagen.
    pub fn from_str(s: &str) -> Self {
        match s {
            "disconnected" => Self::Disconnected,
            "connecting-alpha" | "connecting-beta" => Self::Connecting,
            "scanning" => Self::Scanning,
            "reconciling" | "staging-alpha" | "staging-beta" | "transitioning" | "saving" => {
                Self::Syncing
            }
            "waiting-for-rescan" => Self::WaitingForRescan,
            "watching" => Self::Watching,
            s if s.starts_with("halted") => Self::Halted(s.to_string()),
            other => Self::Unknown(other.to_string()),
        }
    }

    /// Prüft ob die Session "complete" ist (watching oder waiting-for-rescan).
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Watching | Self::WaitingForRescan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::EndpointStatus;

    fn mock_endpoint(path: &str) -> EndpointStatus {
        EndpointStatus {
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

    fn mock_mutagen_session(name: &str, status: &str) -> MutagenSession {
        MutagenSession {
            identifier: format!("id-{}", name),
            name: name.to_string(),
            status: status.to_string(),
            successful_cycles: 0,
            alpha: mock_endpoint("/local"),
            beta: mock_endpoint("/remote"),
        }
    }

    #[test]
    fn test_actual_state_from_mutagen_sessions() {
        let sessions = vec![
            mock_mutagen_session("frontend-abc123", "watching"),
            mock_mutagen_session("backend-abc123", "scanning"),
        ];

        let state = ActualState::from_mutagen_sessions(sessions);

        assert_eq!(state.sessions.len(), 2);
    }

    #[test]
    fn test_find_by_name() {
        let sessions = vec![
            mock_mutagen_session("frontend-abc123", "watching"),
            mock_mutagen_session("backend-abc123", "scanning"),
        ];

        let state = ActualState::from_mutagen_sessions(sessions);

        assert!(state.find_by_name("frontend-abc123").is_some());
        assert!(state.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_all_complete() {
        let sessions = vec![
            mock_mutagen_session("frontend-abc123", "watching"),
            mock_mutagen_session("backend-abc123", "watching"),
        ];

        let state = ActualState::from_mutagen_sessions(sessions);

        assert!(state.all_complete(&[
            "frontend-abc123".to_string(),
            "backend-abc123".to_string()
        ]));
    }

    #[test]
    fn test_all_complete_with_incomplete() {
        let sessions = vec![
            mock_mutagen_session("frontend-abc123", "watching"),
            mock_mutagen_session("backend-abc123", "scanning"),
        ];

        let state = ActualState::from_mutagen_sessions(sessions);

        assert!(!state.all_complete(&[
            "frontend-abc123".to_string(),
            "backend-abc123".to_string()
        ]));
    }

    #[test]
    fn test_session_status_from_str() {
        assert_eq!(
            SessionStatus::from_str("watching"),
            SessionStatus::Watching
        );
        assert_eq!(
            SessionStatus::from_str("scanning"),
            SessionStatus::Scanning
        );
        assert_eq!(
            SessionStatus::from_str("connecting-alpha"),
            SessionStatus::Connecting
        );
        assert_eq!(
            SessionStatus::from_str("reconciling"),
            SessionStatus::Syncing
        );
        assert!(matches!(
            SessionStatus::from_str("halted-on-root-empty"),
            SessionStatus::Halted(_)
        ));
    }

    #[test]
    fn test_session_status_is_complete() {
        assert!(SessionStatus::Watching.is_complete());
        assert!(SessionStatus::WaitingForRescan.is_complete());
        assert!(!SessionStatus::Scanning.is_complete());
        assert!(!SessionStatus::Connecting.is_complete());
    }
}
