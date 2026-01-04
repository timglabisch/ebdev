//! Reconcile Module - Pure Function für State Reconciliation
//!
//! Dieses Modul enthält die reconcile() Funktion und die Action-Typen.
//!
//! Die reconcile() Funktion ist eine **pure function**:
//! - Keine Side Effects
//! - Deterministisch
//! - Perfekt testbar ohne Mocks

mod actions;

pub use actions::{Action, ActionList, SessionToCreate, SessionToTerminate};

// Die reconcile() Funktion wird in Phase 2 implementiert.
// Hier ist ein Placeholder:

use crate::state::{ActualState, ControllerState, DesiredState, StagePhase};

/// Berechnet die nächsten Aktionen basierend auf Ist/Soll-Zustand.
///
/// Dies ist eine **pure function** - sie hat keine Side Effects und ist
/// deterministisch. Das macht sie perfekt testbar ohne Mocks.
///
/// # Arguments
///
/// * `desired` - Der gewünschte Zustand (aus der Config)
/// * `actual` - Der aktuelle Zustand (von Mutagen)
/// * `state` - Der Controller-Zustand (aktuelle Stage, Phase, Optionen)
///
/// # Returns
///
/// Eine Liste von Actions die ausgeführt werden sollen.
///
/// # Example
///
/// ```ignore
/// let desired = DesiredState::from_projects(&projects);
/// let actual = ActualState::from_mutagen_sessions(sessions);
/// let state = ControllerState::new(options);
///
/// let actions = reconcile(&desired, &actual, &state);
///
/// for action in actions {
///     executor.execute(action).await?;
/// }
/// ```
pub fn reconcile(
    desired: &DesiredState,
    actual: &ActualState,
    state: &ControllerState,
) -> Vec<Action> {
    let mut actions = ActionList::new();

    match state.phase {
        StagePhase::NotStarted => {
            reconcile_not_started(desired, actual, state, &mut actions);
        }
        StagePhase::Syncing => {
            reconcile_syncing(desired, actual, state, &mut actions);
        }
        StagePhase::WaitingForCompletion => {
            reconcile_waiting(desired, actual, state, &mut actions);
        }
        StagePhase::Finalizing => {
            reconcile_finalizing(desired, actual, state, &mut actions);
        }
        StagePhase::Watching => {
            // Im Watch-Mode passiert nichts - User muss quiten
        }
        StagePhase::Done => {
            // Fertig - keine weiteren Actions
        }
    }

    actions.into_vec()
}

/// Reconciliation für NotStarted-Phase.
fn reconcile_not_started(
    desired: &DesiredState,
    actual: &ActualState,
    state: &ControllerState,
    actions: &mut ActionList,
) {
    // 1. Cleanup: Sessions für gelöschte Projekte terminieren
    let cleanup = compute_cleanup(desired, actual);
    if !cleanup.is_empty() {
        actions.push(Action::Cleanup {
            sessions_to_terminate: cleanup,
        });
    }

    // 2. Zur ersten Stage wechseln (oder fertig wenn keine Stages)
    if let Some(first_stage) = state.first_stage_to_run() {
        let is_final = state.stages_to_run.last() == Some(&first_stage);
        let session_count = desired.sessions_for_stage(first_stage).len();
        actions.push(Action::AdvanceStage {
            stage: first_stage,
            session_count,
            is_final,
        });
    } else {
        actions.push(Action::Complete);
    }
}

/// Reconciliation für Syncing-Phase.
fn reconcile_syncing(
    desired: &DesiredState,
    actual: &ActualState,
    state: &ControllerState,
    actions: &mut ActionList,
) {
    let current_stage = match state.current_stage {
        Some(s) => s,
        None => return, // Sollte nicht passieren
    };

    // Sessions für diese Stage synchronisieren
    let stage_sessions = desired.sessions_for_stage(current_stage);
    let is_final = state.is_last_stage();
    let no_watch = !is_final; // Non-final stages im no-watch Modus

    let (to_create, to_keep, to_terminate) =
        compute_session_sync(&stage_sessions, actual, desired.root_crc32, no_watch);

    if !to_create.is_empty() || !to_terminate.is_empty() || !to_keep.is_empty() {
        actions.push(Action::SyncSessions {
            to_create,
            to_keep,
            to_terminate,
        });
    }

    // Wechsel zur WaitForCompletion-Phase
    let session_names: Vec<String> = stage_sessions.iter().map(|s| s.name.clone()).collect();
    actions.push(Action::WaitForCompletion { session_names });
}

/// Reconciliation für WaitingForCompletion-Phase.
fn reconcile_waiting(
    desired: &DesiredState,
    actual: &ActualState,
    state: &ControllerState,
    actions: &mut ActionList,
) {
    let current_stage = match state.current_stage {
        Some(s) => s,
        None => return,
    };

    let stage_sessions = desired.sessions_for_stage(current_stage);
    let session_names: Vec<String> = stage_sessions.iter().map(|s| s.name.clone()).collect();

    // Prüfen ob alle Sessions complete sind
    if actual.all_complete(&session_names) {
        let is_final = state.is_last_stage();

        actions.push(Action::StageComplete {
            stage: current_stage,
            is_final,
        });

        if is_final {
            if state.options.keep_open {
                actions.push(Action::EnterWatchMode);
            } else {
                actions.push(Action::Complete);
            }
        } else {
            // Non-final stage: Sessions terminieren
            actions.push(Action::TerminateStage { session_names });
        }
    }
    // Wenn nicht alle complete: Warten (keine Action)
}

/// Reconciliation für Finalizing-Phase.
fn reconcile_finalizing(
    desired: &DesiredState,
    _actual: &ActualState,
    state: &ControllerState,
    actions: &mut ActionList,
) {
    // Nach dem Terminieren zur nächsten Stage wechseln
    if let Some(next_stage) = state.next_stage() {
        let is_final = state.stages_to_run.last() == Some(&next_stage);
        let session_count = desired.sessions_for_stage(next_stage).len();
        actions.push(Action::AdvanceStage {
            stage: next_stage,
            session_count,
            is_final,
        });
    } else {
        actions.push(Action::Complete);
    }
}

/// Berechnet welche Sessions für gelöschte Projekte terminiert werden sollen.
fn compute_cleanup(desired: &DesiredState, actual: &ActualState) -> Vec<SessionToTerminate> {
    if desired.sessions.is_empty() {
        return vec![];
    }

    let root_suffix = format!("{:08x}", desired.root_crc32);

    // Alle gültigen Session-Namen
    let valid_names: std::collections::HashSet<&str> =
        desired.sessions.iter().map(|s| s.name.as_str()).collect();

    // Prefixes aller bekannten Projekte
    let valid_prefixes: std::collections::HashSet<String> = desired
        .sessions
        .iter()
        .map(|s| format!("{}-", s.project_name))
        .collect();

    let mut to_terminate = Vec::new();

    for session in &actual.sessions {
        // Nur Sessions dieses Roots betrachten
        if !session.name.ends_with(&root_suffix) {
            continue;
        }

        // Prüfen ob das Projekt noch existiert
        let project_exists = valid_prefixes
            .iter()
            .any(|prefix| session.name.starts_with(prefix));

        if project_exists && !valid_names.contains(session.name.as_str()) {
            // Projekt existiert, aber Session-Name stimmt nicht (Config-Änderung)
            // -> Wird in sync_stage_sessions behandelt
            continue;
        }

        if !project_exists {
            // Projekt existiert nicht mehr
            to_terminate.push(SessionToTerminate {
                identifier: session.identifier.clone(),
                name: session.name.clone(),
            });
        }
    }

    to_terminate
}

/// Berechnet welche Sessions erstellt/beibehalten/terminiert werden sollen.
fn compute_session_sync(
    stage_sessions: &[&crate::state::DesiredSession],
    actual: &ActualState,
    root_crc32: u32,
    no_watch: bool,
) -> (Vec<SessionToCreate>, Vec<String>, Vec<SessionToTerminate>) {
    let root_suffix = format!("{:08x}", root_crc32);

    let mut to_create = Vec::new();
    let mut to_keep = Vec::new();
    let mut to_terminate = Vec::new();

    for desired in stage_sessions {
        let prefix = format!("{}-", desired.project_name);

        // Suche existierende Session für dieses Projekt
        let existing: Vec<_> = actual
            .sessions
            .iter()
            .filter(|s| s.name.starts_with(&prefix) && s.name.ends_with(&root_suffix))
            .collect();

        let mut found_match = false;

        for session in existing {
            if session.name == desired.name {
                // Perfekter Match - behalten
                to_keep.push(session.name.clone());
                found_match = true;
            } else {
                // Anderer Name (Config-Änderung) - terminieren
                to_terminate.push(SessionToTerminate {
                    identifier: session.identifier.clone(),
                    name: session.name.clone(),
                });
            }
        }

        if !found_match {
            to_create.push(SessionToCreate {
                session: (*desired).clone(),
                no_watch,
            });
        }
    }

    (to_create, to_keep, to_terminate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ActualSession, SessionStatus};
    use crate::SyncOptions;
    use ebdev_mutagen_config::{DiscoveredProject, MutagenSyncProject, PermissionsConfig, PollingConfig, SyncMode};
    use std::path::PathBuf;

    fn make_project(name: &str, target: &str, stage: i32, root: &str) -> DiscoveredProject {
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

    fn make_actual_session(name: &str, status: SessionStatus) -> ActualSession {
        ActualSession {
            identifier: format!("id-{}", name),
            name: name.to_string(),
            status,
            alpha: "/local".to_string(),
            beta: "docker://app".to_string(),
        }
    }

    // =========================================================================
    // Tests: NotStarted Phase
    // =========================================================================

    #[test]
    fn test_reconcile_not_started_empty_projects() {
        let desired = DesiredState::default();
        let actual = ActualState::default();
        let state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });

        let actions = reconcile(&desired, &actual, &state);

        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Complete));
    }

    #[test]
    fn test_reconcile_not_started_advances_to_first_stage() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let actual = ActualState::default();

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);

        let actions = reconcile(&desired, &actual, &state);

        // Sollte AdvanceStage enthalten
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::AdvanceStage { stage: 0, is_final: true, .. }
        )));
    }

    #[test]
    fn test_reconcile_not_started_with_cleanup() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project.clone()]);

        // Eine alte Session die nicht mehr existieren sollte
        let old_session_name = format!("old-project-{:08x}", project.root_crc32());
        let actual = ActualState {
            sessions: vec![make_actual_session(&old_session_name, SessionStatus::Watching)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);

        let actions = reconcile(&desired, &actual, &state);

        // Sollte Cleanup und AdvanceStage enthalten
        assert!(actions.iter().any(|a| matches!(a, Action::Cleanup { .. })));
        assert!(actions.iter().any(|a| matches!(a, Action::AdvanceStage { .. })));
    }

    // =========================================================================
    // Tests: Syncing Phase
    // =========================================================================

    #[test]
    fn test_reconcile_syncing_creates_new_session() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let actual = ActualState::default();

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);

        let actions = reconcile(&desired, &actual, &state);

        // Sollte SyncSessions mit to_create enthalten
        assert!(actions.iter().any(|a| {
            if let Action::SyncSessions { to_create, .. } = a {
                !to_create.is_empty()
            } else {
                false
            }
        }));
    }

    #[test]
    fn test_reconcile_syncing_keeps_existing_session() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project.clone()]);

        let session_name = project.session_name();
        let actual = ActualState {
            sessions: vec![make_actual_session(&session_name, SessionStatus::Watching)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);

        let actions = reconcile(&desired, &actual, &state);

        // Sollte SyncSessions mit to_keep enthalten
        assert!(actions.iter().any(|a| {
            if let Action::SyncSessions { to_keep, .. } = a {
                !to_keep.is_empty()
            } else {
                false
            }
        }));
    }

    // =========================================================================
    // Tests: WaitingForCompletion Phase
    // =========================================================================

    #[test]
    fn test_reconcile_waiting_not_complete() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project.clone()]);

        let session_name = project.session_name();
        let actual = ActualState {
            sessions: vec![make_actual_session(&session_name, SessionStatus::Scanning)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_waiting();

        let actions = reconcile(&desired, &actual, &state);

        // Keine Actions wenn noch nicht complete
        assert!(actions.is_empty());
    }

    #[test]
    fn test_reconcile_waiting_complete_final_stage_no_keep_open() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project.clone()]);

        let session_name = project.session_name();
        let actual = ActualState {
            sessions: vec![make_actual_session(&session_name, SessionStatus::Watching)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_waiting();

        let actions = reconcile(&desired, &actual, &state);

        // Sollte StageComplete und Complete enthalten
        assert!(actions.iter().any(|a| matches!(a, Action::StageComplete { .. })));
        assert!(actions.iter().any(|a| matches!(a, Action::Complete)));
    }

    #[test]
    fn test_reconcile_waiting_complete_final_stage_with_keep_open() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project.clone()]);

        let session_name = project.session_name();
        let actual = ActualState {
            sessions: vec![make_actual_session(&session_name, SessionStatus::Watching)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: true,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_waiting();

        let actions = reconcile(&desired, &actual, &state);

        // Sollte StageComplete und EnterWatchMode enthalten
        assert!(actions.iter().any(|a| matches!(a, Action::StageComplete { .. })));
        assert!(actions.iter().any(|a| matches!(a, Action::EnterWatchMode)));
    }

    #[test]
    fn test_reconcile_waiting_complete_non_final_stage() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let desired = DesiredState::from_projects(&[p0.clone(), p1]);

        let session_name = p0.session_name();
        let actual = ActualState {
            sessions: vec![make_actual_session(&session_name, SessionStatus::Watching)],
        };

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: true,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_waiting();

        let actions = reconcile(&desired, &actual, &state);

        // Sollte StageComplete und TerminateStage enthalten (nicht EnterWatchMode)
        assert!(actions.iter().any(|a| matches!(a, Action::StageComplete { stage: 0, is_final: false })));
        assert!(actions.iter().any(|a| matches!(a, Action::TerminateStage { .. })));
        assert!(!actions.iter().any(|a| matches!(a, Action::EnterWatchMode)));
    }

    // =========================================================================
    // Tests: Finalizing Phase
    // =========================================================================

    #[test]
    fn test_reconcile_finalizing_advances_to_next_stage() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let desired = DesiredState::from_projects(&[p0, p1]);
        let actual = ActualState::default();

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_finalizing();

        let actions = reconcile(&desired, &actual, &state);

        // Sollte AdvanceStage zu Stage 1 enthalten
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::AdvanceStage { stage: 1, is_final: true, .. }
        )));
    }

    #[test]
    fn test_reconcile_finalizing_completes_when_no_more_stages() {
        let project = make_project("app", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let actual = ActualState::default();

        let mut state = ControllerState::new(SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        });
        state.compute_stages_to_run(&desired.stages, desired.last_stage);
        state.advance_to_stage(0);
        state.start_finalizing();

        let actions = reconcile(&desired, &actual, &state);

        // Sollte Complete enthalten
        assert!(actions.iter().any(|a| matches!(a, Action::Complete)));
    }
}
