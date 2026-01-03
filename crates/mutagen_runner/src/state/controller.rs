//! Controller State - Minimaler Zustand des Sync-Controllers
//!
//! Der ControllerState enthält nur den minimal notwendigen Zustand,
//! der zwischen Reconciliation-Zyklen gehalten werden muss.

use crate::SyncOptions;

/// Der Zustand des Sync-Controllers.
///
/// Enthält nur den minimal notwendigen Zustand für die Reconciliation.
#[derive(Debug, Clone)]
pub struct ControllerState {
    /// Die aktuelle Stage (None wenn noch nicht gestartet)
    pub current_stage: Option<i32>,
    /// Die aktuelle Phase innerhalb der Stage
    pub phase: StagePhase,
    /// Die Sync-Optionen
    pub options: SyncOptions,
    /// Liste der Stages die ausgeführt werden sollen
    pub stages_to_run: Vec<i32>,
}

impl ControllerState {
    /// Erstellt einen neuen ControllerState.
    pub fn new(options: SyncOptions) -> Self {
        Self {
            current_stage: None,
            phase: StagePhase::NotStarted,
            options,
            stages_to_run: Vec::new(),
        }
    }

    /// Berechnet welche Stages ausgeführt werden sollen.
    pub fn compute_stages_to_run(&mut self, all_stages: &[i32], last_stage: i32) {
        self.stages_to_run = match (self.options.run_init_stages, self.options.run_final_stage) {
            (true, true) => all_stages.to_vec(),
            (true, false) => all_stages.iter().filter(|&&s| s != last_stage).copied().collect(),
            (false, true) => vec![last_stage],
            (false, false) => Vec::new(),
        };
    }

    /// Prüft ob eine Stage in den auszuführenden Stages enthalten ist.
    pub fn should_run_stage(&self, stage: i32) -> bool {
        self.stages_to_run.contains(&stage)
    }

    /// Gibt die erste Stage zurück die ausgeführt werden soll.
    pub fn first_stage_to_run(&self) -> Option<i32> {
        self.stages_to_run.first().copied()
    }

    /// Gibt die nächste Stage nach der aktuellen zurück.
    pub fn next_stage(&self) -> Option<i32> {
        let current = self.current_stage?;
        self.stages_to_run.iter().find(|&&s| s > current).copied()
    }

    /// Prüft ob die aktuelle Stage die letzte auszuführende Stage ist.
    pub fn is_last_stage(&self) -> bool {
        match self.current_stage {
            Some(current) => self.stages_to_run.last() == Some(&current),
            None => false,
        }
    }

    /// Prüft ob der Controller in Watch-Mode gehen soll.
    pub fn should_enter_watch_mode(&self) -> bool {
        self.is_last_stage() && self.options.keep_open
    }

    /// Wechselt zur angegebenen Stage.
    pub fn advance_to_stage(&mut self, stage: i32) {
        self.current_stage = Some(stage);
        self.phase = StagePhase::Syncing;
    }

    /// Setzt die Phase auf WaitingForCompletion.
    pub fn start_waiting(&mut self) {
        self.phase = StagePhase::WaitingForCompletion;
    }

    /// Setzt die Phase auf Finalizing.
    pub fn start_finalizing(&mut self) {
        self.phase = StagePhase::Finalizing;
    }

    /// Setzt die Phase auf Watching.
    pub fn enter_watch_mode(&mut self) {
        self.phase = StagePhase::Watching;
    }

    /// Setzt die Phase auf Done.
    pub fn mark_done(&mut self) {
        self.phase = StagePhase::Done;
    }
}

/// Die Phase innerhalb einer Stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagePhase {
    /// Noch nicht gestartet (Cleanup-Phase)
    NotStarted,
    /// Sessions werden erstellt/aktualisiert
    Syncing,
    /// Warten auf Sync-Completion
    WaitingForCompletion,
    /// Stage wird abgeschlossen (Sessions terminieren)
    Finalizing,
    /// Im Watch-Mode (finale Stage mit keep_open)
    Watching,
    /// Fertig
    Done,
}

impl StagePhase {
    /// Prüft ob der Sync abgeschlossen ist.
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Done)
    }

    /// Prüft ob im Watch-Mode.
    pub fn is_watching(&self) -> bool {
        matches!(self, Self::Watching)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_state_new() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let state = ControllerState::new(options);

        assert_eq!(state.current_stage, None);
        assert_eq!(state.phase, StagePhase::NotStarted);
    }

    #[test]
    fn test_compute_stages_to_run_all() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        assert_eq!(state.stages_to_run, vec![0, 1, 2]);
    }

    #[test]
    fn test_compute_stages_to_run_init_only() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: false,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        assert_eq!(state.stages_to_run, vec![0, 1]);
    }

    #[test]
    fn test_compute_stages_to_run_final_only() {
        let options = SyncOptions {
            run_init_stages: false,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        assert_eq!(state.stages_to_run, vec![2]);
    }

    #[test]
    fn test_compute_stages_to_run_nothing() {
        let options = SyncOptions {
            run_init_stages: false,
            run_final_stage: false,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        assert!(state.stages_to_run.is_empty());
    }

    #[test]
    fn test_first_stage_to_run() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        assert_eq!(state.first_stage_to_run(), Some(0));
    }

    #[test]
    fn test_next_stage() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);
        state.current_stage = Some(0);

        assert_eq!(state.next_stage(), Some(1));

        state.current_stage = Some(2);
        assert_eq!(state.next_stage(), None);
    }

    #[test]
    fn test_is_last_stage() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1, 2], 2);

        state.current_stage = Some(0);
        assert!(!state.is_last_stage());

        state.current_stage = Some(2);
        assert!(state.is_last_stage());
    }

    #[test]
    fn test_should_enter_watch_mode() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: true,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1], 1);

        state.current_stage = Some(0);
        assert!(!state.should_enter_watch_mode());

        state.current_stage = Some(1);
        assert!(state.should_enter_watch_mode());
    }

    #[test]
    fn test_should_not_enter_watch_mode_without_keep_open() {
        let options = SyncOptions {
            run_init_stages: true,
            run_final_stage: true,
            keep_open: false,
        };
        let mut state = ControllerState::new(options);
        state.compute_stages_to_run(&[0, 1], 1);

        state.current_stage = Some(1);
        assert!(!state.should_enter_watch_mode());
    }

    #[test]
    fn test_advance_to_stage() {
        let mut state = ControllerState::new(SyncOptions::default());
        state.advance_to_stage(1);

        assert_eq!(state.current_stage, Some(1));
        assert_eq!(state.phase, StagePhase::Syncing);
    }

    #[test]
    fn test_phase_transitions() {
        let mut state = ControllerState::new(SyncOptions::default());

        assert_eq!(state.phase, StagePhase::NotStarted);

        state.advance_to_stage(0);
        assert_eq!(state.phase, StagePhase::Syncing);

        state.start_waiting();
        assert_eq!(state.phase, StagePhase::WaitingForCompletion);

        state.start_finalizing();
        assert_eq!(state.phase, StagePhase::Finalizing);

        state.enter_watch_mode();
        assert_eq!(state.phase, StagePhase::Watching);

        state.mark_done();
        assert_eq!(state.phase, StagePhase::Done);
    }
}
