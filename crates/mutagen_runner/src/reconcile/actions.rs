//! Actions - Aktionen die vom Reconciler abgeleitet werden
//!
//! Actions sind die Ausgabe der reconcile() Funktion und beschreiben
//! was der Controller tun soll.

use crate::state::DesiredSession;

/// Eine Aktion die vom Controller ausgeführt werden soll.
///
/// Actions sind das Ergebnis der reconcile() Funktion und beschreiben
/// was getan werden muss, um vom Ist- zum Soll-Zustand zu gelangen.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Cleanup: Sessions für gelöschte Projekte terminieren
    Cleanup {
        /// Sessions die terminiert werden sollen
        sessions_to_terminate: Vec<SessionToTerminate>,
    },

    /// Zur angegebenen Stage wechseln
    AdvanceStage {
        /// Die Stage zu der gewechselt werden soll
        stage: i32,
        /// Anzahl der Sessions in dieser Stage
        session_count: usize,
        /// Ist das die finale Stage?
        is_final: bool,
    },

    /// Sessions für die aktuelle Stage synchronisieren
    SyncSessions {
        /// Sessions die erstellt werden sollen
        to_create: Vec<SessionToCreate>,
        /// Sessions die beibehalten werden
        to_keep: Vec<String>,
        /// Sessions die terminiert werden sollen (Config-Änderung)
        to_terminate: Vec<SessionToTerminate>,
    },

    /// Warten bis Sessions complete sind
    WaitForCompletion {
        /// Die Session-Namen auf die gewartet werden soll
        session_names: Vec<String>,
    },

    /// Stage wurde abgeschlossen
    StageComplete {
        /// Die abgeschlossene Stage
        stage: i32,
        /// Ist das die finale Stage?
        is_final: bool,
    },

    /// Sessions der aktuellen Stage terminieren (nach non-final stage)
    TerminateStage {
        /// Die Session-Namen die terminiert werden sollen
        session_names: Vec<String>,
    },

    /// In Watch-Mode gehen (finale Stage mit keep_open)
    EnterWatchMode,

    /// Sync komplett abgeschlossen
    Complete,
}

/// Eine Session die erstellt werden soll.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionToCreate {
    /// Die gewünschte Session-Konfiguration
    pub session: DesiredSession,
    /// Ob im no-watch Modus erstellt werden soll
    pub no_watch: bool,
}

/// Eine Session die terminiert werden soll.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionToTerminate {
    /// Die Mutagen Session ID
    pub identifier: String,
    /// Der Session-Name (für Logging)
    pub name: String,
}

impl Action {
    /// Prüft ob dies eine "finale" Action ist (Complete oder EnterWatchMode).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::EnterWatchMode)
    }

    /// Prüft ob dies eine Action ist die State-Änderungen verursacht.
    pub fn modifies_state(&self) -> bool {
        matches!(
            self,
            Self::AdvanceStage { .. }
                | Self::SyncSessions { .. }
                | Self::StageComplete { .. }
                | Self::EnterWatchMode
                | Self::Complete
        )
    }
}

/// Eine Liste von Actions mit Hilfsmethoden.
#[derive(Debug, Clone, Default)]
pub struct ActionList {
    actions: Vec<Action>,
}

impl ActionList {
    /// Erstellt eine neue leere ActionList.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fügt eine Action hinzu.
    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// Gibt alle Actions zurück.
    pub fn into_vec(self) -> Vec<Action> {
        self.actions
    }

    /// Prüft ob die Liste leer ist.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Gibt die Anzahl der Actions zurück.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Prüft ob eine Complete-Action enthalten ist.
    pub fn has_complete(&self) -> bool {
        self.actions.iter().any(|a| matches!(a, Action::Complete))
    }

    /// Prüft ob eine EnterWatchMode-Action enthalten ist.
    pub fn has_watch_mode(&self) -> bool {
        self.actions
            .iter()
            .any(|a| matches!(a, Action::EnterWatchMode))
    }
}

impl From<Vec<Action>> for ActionList {
    fn from(actions: Vec<Action>) -> Self {
        Self { actions }
    }
}

impl IntoIterator for ActionList {
    type Item = Action;
    type IntoIter = std::vec::IntoIter<Action>;

    fn into_iter(self) -> Self::IntoIter {
        self.actions.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use ebdev_mutagen_config::{PermissionsConfig, PollingConfig, SyncMode};

    fn make_desired_session(name: &str) -> DesiredSession {
        DesiredSession {
            name: name.to_string(),
            project_name: name.split('-').next().unwrap().to_string(),
            alpha: PathBuf::from("/local"),
            beta: "docker://app".to_string(),
            mode: SyncMode::TwoWay,
            stage: 0,
            ignore: vec![],
            polling: PollingConfig::default(),
            permissions: PermissionsConfig::default(),
            config_hash: 12345,
        }
    }

    #[test]
    fn test_action_is_terminal() {
        assert!(Action::Complete.is_terminal());
        assert!(Action::EnterWatchMode.is_terminal());
        assert!(!Action::AdvanceStage { stage: 0, session_count: 1, is_final: false }.is_terminal());
    }

    #[test]
    fn test_action_modifies_state() {
        assert!(Action::Complete.modifies_state());
        assert!(Action::AdvanceStage { stage: 0, session_count: 1, is_final: false }.modifies_state());
        assert!(!Action::Cleanup { sessions_to_terminate: vec![] }.modifies_state());
        assert!(!Action::WaitForCompletion { session_names: vec![] }.modifies_state());
    }

    #[test]
    fn test_action_list_basic() {
        let mut list = ActionList::new();
        assert!(list.is_empty());

        list.push(Action::Complete);
        assert!(!list.is_empty());
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_action_list_has_complete() {
        let mut list = ActionList::new();
        assert!(!list.has_complete());

        list.push(Action::Complete);
        assert!(list.has_complete());
    }

    #[test]
    fn test_action_list_has_watch_mode() {
        let mut list = ActionList::new();
        assert!(!list.has_watch_mode());

        list.push(Action::EnterWatchMode);
        assert!(list.has_watch_mode());
    }

    #[test]
    fn test_sync_sessions_action() {
        let session = make_desired_session("frontend-abc123");
        let action = Action::SyncSessions {
            to_create: vec![SessionToCreate {
                session,
                no_watch: false,
            }],
            to_keep: vec!["backend-abc123".to_string()],
            to_terminate: vec![],
        };

        assert!(action.modifies_state());
    }

    #[test]
    fn test_cleanup_action() {
        let action = Action::Cleanup {
            sessions_to_terminate: vec![SessionToTerminate {
                identifier: "sess-1".to_string(),
                name: "old-session".to_string(),
            }],
        };

        assert!(!action.modifies_state()); // Cleanup modifiziert den Controller-State nicht
        assert!(!action.is_terminal());
    }
}
