//! SyncController - Haupt-Controller für den Sync-Prozess
//!
//! Der Controller implementiert den Operator Pattern Loop:
//! 1. Hole aktuellen Zustand (ActualState)
//! 2. Berechne Actions (reconcile)
//! 3. Führe Actions aus
//! 4. Wiederhole bis fertig

use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use crate::controller::executor::{execute_action, ExecuteResult};
use crate::reconcile::{reconcile, Action};
use crate::state::{ActualState, ControllerState, DesiredState, StagePhase};
use crate::{MutagenBackend, MutagenRunnerError, SyncOptions, SyncUI};

/// Der Sync-Controller führt den Operator Pattern Loop aus.
pub struct SyncController<B: MutagenBackend, U: SyncUI> {
    backend: Arc<B>,
    desired: DesiredState,
    state: ControllerState,
    ui: U,
    /// Session-Namen für die aktuelle Stage (für Completion-Check)
    current_session_names: Vec<String>,
}

impl<B: MutagenBackend, U: SyncUI> SyncController<B, U> {
    /// Erstellt einen neuen SyncController.
    pub fn new(backend: Arc<B>, desired: DesiredState, options: SyncOptions, ui: U) -> Self {
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&desired.stages, desired.last_stage);

        Self {
            backend,
            desired,
            state,
            ui,
            current_session_names: Vec::new(),
        }
    }

    /// Führt den Sync-Prozess aus.
    ///
    /// Dies ist der Haupt-Loop des Operator Patterns:
    /// 1. Hole aktuellen Zustand
    /// 2. Berechne Actions
    /// 3. Führe Actions aus
    /// 4. Wiederhole bis fertig oder User abbricht
    pub async fn run(mut self) -> Result<(), MutagenRunnerError> {
        // Prüfe ob es überhaupt etwas zu tun gibt
        if self.state.stages_to_run.is_empty() {
            return Ok(());
        }

        loop {
            // Check for user abort
            if self.ui.check_quit()? {
                // Terminate current stage sessions if in non-final stage
                if !self.state.is_last_stage() && !self.current_session_names.is_empty() {
                    self.terminate_current_sessions().await?;
                }
                return Err(MutagenRunnerError::UserAborted);
            }

            // Hole aktuellen Zustand
            let actual = self.fetch_actual_state().await;

            // Berechne Actions
            let actions = reconcile(&self.desired, &actual, &self.state);

            // Wenn keine Actions und wir warten, prüfe Completion
            if actions.is_empty() && self.state.phase == StagePhase::WaitingForCompletion {
                // Tick für UI update
                self.ui.tick()?;
                sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Führe Actions aus
            for action in actions {
                // Speichere Session-Namen wenn WaitForCompletion
                if let Action::WaitForCompletion { ref session_names } = action {
                    self.current_session_names = session_names.clone();
                }

                // Aktualisiere UI mit Stage-Projekten wenn AdvanceStage
                if let Action::AdvanceStage { stage, is_final, .. } = action {
                    let sessions = self.desired.sessions_for_stage(stage);
                    self.ui.set_stage_sessions(
                        self.state.stages_to_run.iter().position(|&s| s == stage).unwrap_or(0),
                        &sessions,
                        is_final,
                    );
                }

                let result = execute_action(
                    action,
                    &mut self.state,
                    self.backend.as_ref(),
                    &mut self.ui,
                ).await?;

                match result {
                    ExecuteResult::Continue => continue,
                    ExecuteResult::Wait => break, // Wechsel zu Wait-Loop
                    ExecuteResult::Done => return Ok(()),
                }
            }

            // Wenn wir im Watch-Mode sind, bleibe in der Loop bis User abbricht
            if self.state.phase == StagePhase::Watching {
                loop {
                    self.ui.tick()?;
                    if self.ui.check_quit()? {
                        return Ok(());
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            }

            // Wenn fertig, beende
            if self.state.phase == StagePhase::Done {
                return Ok(());
            }

            // Kurze Pause zwischen Iterationen
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// Holt den aktuellen Zustand vom Backend.
    async fn fetch_actual_state(&self) -> ActualState {
        let sessions = self.backend.list_sessions().await;
        let actual = ActualState::from_mutagen_sessions(sessions);

        // Update UI mit Session-Status
        // (Die UI erwartet MutagenSession, nicht ActualSession)
        // self.ui.update_sessions(...) - skip for now

        actual
    }

    /// Terminiert die Sessions der aktuellen Stage.
    async fn terminate_current_sessions(&self) -> Result<(), MutagenRunnerError> {
        let sessions = self.backend.list_sessions().await;
        for session in sessions {
            if self.current_session_names.contains(&session.name) {
                let _ = self.backend.terminate_session(&session.identifier).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockMutagen, MockUI};
    use ebdev_mutagen_config::{DiscoveredProject, MutagenSyncProject, PollingConfig, SyncMode};
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
            },
            resolved_directory: PathBuf::from(format!("{}/{}", root, name)),
            config_path: PathBuf::from(format!("{}/.ebdev.ts", root)),
            root_config_path: PathBuf::from(format!("{}/.ebdev.ts", root)),
        }
    }

    #[tokio::test]
    async fn test_controller_empty_stages() {
        let backend = Arc::new(MockMutagen::new());
        let desired = DesiredState::default();
        let options = SyncOptions {
            run_init_stages: false,
            run_final_stage: false,
            keep_open: false,
        };
        let ui = MockUI::new();

        let controller = SyncController::new(backend, desired, options, ui);
        let result = controller.run().await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_controller_single_stage_no_keep_open() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let ui = MockUI::new().with_sessions_complete();

        let controller = SyncController::new(backend.clone(), desired, options, ui);
        let result = controller.run().await;

        assert!(result.is_ok());
        // Session sollte erstellt worden sein
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_controller_single_stage_with_keep_open() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: true,
        };
        // Quit after entering watch mode
        let ui = MockUI::new().with_sessions_complete().quit_after(5);

        let controller = SyncController::new(backend.clone(), desired, options, ui);
        let result = controller.run().await;

        // Should complete successfully (user quit from watch mode)
        assert!(result.is_ok());
        assert_eq!(backend.created_sessions().len(), 1);
    }

    #[tokio::test]
    async fn test_controller_user_abort() {
        let backend = Arc::new(MockMutagen::new());
        let project = make_project("frontend", "docker://app", 0, "/root");
        let desired = DesiredState::from_projects(&[project]);
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        // Quit immediately
        let ui = MockUI::new().quit_after(0);

        let controller = SyncController::new(backend, desired, options, ui);
        let result = controller.run().await;

        assert!(matches!(result, Err(MutagenRunnerError::UserAborted)));
    }

    #[tokio::test]
    async fn test_controller_multi_stage() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let desired = DesiredState::from_projects(&[p0, p1]);
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let ui = MockUI::new().with_sessions_complete();

        let controller = SyncController::new(backend.clone(), desired, options, ui);
        let result = controller.run().await;

        assert!(result.is_ok());
        // Both sessions should be created
        assert_eq!(backend.created_sessions().len(), 2);
        // Stage 0 session should be terminated
        assert!(!backend.terminated_sessions().is_empty());
    }

    #[tokio::test]
    async fn test_controller_init_only() {
        let backend = Arc::new(MockMutagen::new());
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let desired = DesiredState::from_projects(&[p0, p1]);
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: false,
            keep_open: false,
        };
        let ui = MockUI::new().with_sessions_complete();

        let controller = SyncController::new(backend.clone(), desired, options, ui);
        let result = controller.run().await;

        assert!(result.is_ok());
        // Only stage 0 session should be created
        assert_eq!(backend.created_sessions().len(), 1);
    }
}
