//! Action Executor - Führt Actions aus
//!
//! Der Executor ist verantwortlich für die tatsächliche Ausführung der
//! vom Reconciler berechneten Actions.

use crate::reconcile::{Action, SessionToCreate, SessionToTerminate};
use crate::state::ControllerState;
use crate::{MutagenBackend, MutagenRunnerError, SyncUI};

/// Führt eine Action aus und aktualisiert den Controller-State.
///
/// Gibt zurück ob der Executor weitermachen soll (true) oder fertig ist (false).
pub async fn execute_action<B: MutagenBackend, U: SyncUI>(
    action: Action,
    state: &mut ControllerState,
    backend: &B,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    match action {
        Action::Cleanup { sessions_to_terminate } => {
            execute_cleanup(sessions_to_terminate, backend, ui).await
        }

        Action::AdvanceStage { stage, session_count, is_final } => {
            execute_advance_stage(stage, session_count, is_final, state, ui)
        }

        Action::SyncSessions { to_create, to_keep, to_terminate } => {
            execute_sync_sessions(to_create, to_keep, to_terminate, backend, ui).await
        }

        Action::WaitForCompletion { session_names } => {
            execute_wait_for_completion(session_names, state, ui)
        }

        Action::StageComplete { stage, is_final } => {
            execute_stage_complete(stage, is_final, ui)
        }

        Action::TerminateStage { session_names } => {
            execute_terminate_stage(session_names, state, backend).await
        }

        Action::EnterWatchMode => {
            execute_enter_watch_mode(state, ui)
        }

        Action::Complete => {
            execute_complete(state, ui)
        }
    }
}

/// Ergebnis einer Action-Ausführung
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteResult {
    /// Weiter mit nächster Action
    Continue,
    /// Warten auf externe Ereignisse (Completion, User-Input)
    Wait,
    /// Fertig (Complete oder Watch-Mode)
    Done,
}

// ============================================================================
// Action Implementations
// ============================================================================

async fn execute_cleanup<B: MutagenBackend, U: SyncUI>(
    sessions: Vec<SessionToTerminate>,
    backend: &B,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    for session in &sessions {
        let _ = backend.terminate_session(&session.identifier).await;
    }

    ui.on_cleanup(sessions.len());
    Ok(ExecuteResult::Continue)
}

fn execute_advance_stage<U: SyncUI>(
    stage: i32,
    session_count: usize,
    is_final: bool,
    state: &mut ControllerState,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    state.advance_to_stage(stage);

    // UI benötigt die Session-Anzahl für diese Stage
    ui.on_stage_start(stage, session_count, is_final);

    Ok(ExecuteResult::Continue)
}

async fn execute_sync_sessions<B: MutagenBackend, U: SyncUI>(
    to_create: Vec<SessionToCreate>,
    to_keep: Vec<String>,
    to_terminate: Vec<SessionToTerminate>,
    backend: &B,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    // Terminate sessions with changed config
    for session in &to_terminate {
        let _ = backend.terminate_session(&session.identifier).await;
    }

    // Create new sessions
    let mut created_names = Vec::new();
    for create in &to_create {
        backend.create_session_from_desired(&create.session, create.no_watch).await?;
        created_names.push(create.session.name.clone());
    }

    ui.mark_sessions_created(&created_names);
    ui.on_sync_result(to_terminate.len(), to_keep.len(), to_create.len());

    Ok(ExecuteResult::Continue)
}

fn execute_wait_for_completion<U: SyncUI>(
    session_names: Vec<String>,
    state: &mut ControllerState,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    state.start_waiting();

    if let Some(stage) = state.current_stage {
        ui.on_waiting(stage);
    }

    // Die Session-Namen werden für spätere Prüfung gespeichert
    // Der Controller wird in einer Wait-Loop prüfen ob alle complete sind
    let _ = session_names; // Wird vom Controller verwendet

    Ok(ExecuteResult::Wait)
}

fn execute_stage_complete<U: SyncUI>(
    stage: i32,
    is_final: bool,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    ui.on_stage_complete(stage, is_final);
    Ok(ExecuteResult::Continue)
}

async fn execute_terminate_stage<B: MutagenBackend>(
    session_names: Vec<String>,
    state: &mut ControllerState,
    backend: &B,
) -> Result<ExecuteResult, MutagenRunnerError> {
    state.start_finalizing();

    // Hole alle Sessions und terminiere die passenden
    let sessions = backend.list_sessions().await;
    for session in sessions {
        if session_names.contains(&session.name) {
            let _ = backend.terminate_session(&session.identifier).await;
        }
    }

    Ok(ExecuteResult::Continue)
}

fn execute_enter_watch_mode<U: SyncUI>(
    state: &mut ControllerState,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    state.enter_watch_mode();
    ui.on_watch_mode();
    Ok(ExecuteResult::Done)
}

fn execute_complete<U: SyncUI>(
    state: &mut ControllerState,
    ui: &mut U,
) -> Result<ExecuteResult, MutagenRunnerError> {
    state.mark_done();
    ui.on_complete();
    Ok(ExecuteResult::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DesiredSession, StagePhase};
    use crate::test_utils::{MockMutagen, MockUI};
    use crate::SyncOptions;
    use ebdev_mutagen_config::{PollingConfig, SyncMode};
    use std::path::PathBuf;

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
            config_hash: 12345,
        }
    }

    #[tokio::test]
    async fn test_execute_cleanup() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();

        // Add a session to terminate
        backend.add_session(crate::test_utils::mock_session("old-session"));

        let action = Action::Cleanup {
            sessions_to_terminate: vec![SessionToTerminate {
                identifier: "id-old-session".to_string(),
                name: "old-session".to_string(),
            }],
        };

        let mut state = ControllerState::new(SyncOptions::default());
        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Continue);
        assert!(ui.events.contains(&"cleanup:1".to_string()));
    }

    #[tokio::test]
    async fn test_execute_advance_stage() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();
        let mut state = ControllerState::new(SyncOptions::default());

        let action = Action::AdvanceStage { stage: 1, session_count: 1, is_final: false };
        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Continue);
        assert_eq!(state.current_stage, Some(1));
        assert_eq!(state.phase, StagePhase::Syncing);
    }

    #[tokio::test]
    async fn test_execute_sync_sessions_create() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();
        let mut state = ControllerState::new(SyncOptions::default());

        let session = make_desired_session("frontend-abc123");
        let action = Action::SyncSessions {
            to_create: vec![SessionToCreate { session, no_watch: false }],
            to_keep: vec![],
            to_terminate: vec![],
        };

        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Continue);
        assert_eq!(backend.created_sessions().len(), 1);
        assert!(ui.events.contains(&"sync_result:0:0:1".to_string()));
    }

    #[tokio::test]
    async fn test_execute_wait_for_completion() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();
        let mut state = ControllerState::new(SyncOptions::default());
        state.advance_to_stage(0);

        let action = Action::WaitForCompletion {
            session_names: vec!["session-1".to_string()],
        };

        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Wait);
        assert_eq!(state.phase, StagePhase::WaitingForCompletion);
    }

    #[tokio::test]
    async fn test_execute_enter_watch_mode() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();
        let mut state = ControllerState::new(SyncOptions::default());

        let action = Action::EnterWatchMode;
        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Done);
        assert_eq!(state.phase, StagePhase::Watching);
        assert!(ui.events.contains(&"watch_mode".to_string()));
    }

    #[tokio::test]
    async fn test_execute_complete() {
        let backend = MockMutagen::new();
        let mut ui = MockUI::new();
        let mut state = ControllerState::new(SyncOptions::default());

        let action = Action::Complete;
        let result = execute_action(action, &mut state, &backend, &mut ui).await.unwrap();

        assert_eq!(result, ExecuteResult::Done);
        assert_eq!(state.phase, StagePhase::Done);
        assert!(ui.events.contains(&"complete".to_string()));
    }
}
