//! Desired State - Was laut Config existieren sollte
//!
//! Der DesiredState wird aus der Projekt-Konfiguration berechnet und repräsentiert
//! den Soll-Zustand des Systems.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use ebdev_mutagen_config::{DiscoveredProject, PollingConfig, SyncMode};

/// Der gewünschte Zustand aller Mutagen-Sessions.
///
/// Wird aus der Projekt-Konfiguration berechnet und enthält alle Sessions,
/// die existieren sollten.
#[derive(Debug, Clone, Default)]
pub struct DesiredState {
    /// Alle gewünschten Sessions
    pub sessions: Vec<DesiredSession>,
    /// Sortierte Liste der Stages (aufsteigend)
    pub stages: Vec<i32>,
    /// Die letzte/höchste Stage-Nummer
    pub last_stage: i32,
    /// CRC32 des Root-Pfads (für Session-Namensgebung)
    pub root_crc32: u32,
}

impl DesiredState {
    /// Erstellt einen DesiredState aus einer Liste von Projekten.
    pub fn from_projects(projects: &[DiscoveredProject]) -> Self {
        if projects.is_empty() {
            return Self::default();
        }

        let root_crc32 = projects[0].root_crc32();

        let sessions: Vec<DesiredSession> = projects
            .iter()
            .map(|p| DesiredSession::from_project(p))
            .collect();

        let mut stages: Vec<i32> = sessions.iter().map(|s| s.stage).collect();
        stages.sort();
        stages.dedup();

        let last_stage = stages.last().copied().unwrap_or(0);

        Self {
            sessions,
            stages,
            last_stage,
            root_crc32,
        }
    }

    /// Gibt alle Sessions für eine bestimmte Stage zurück.
    pub fn sessions_for_stage(&self, stage: i32) -> Vec<&DesiredSession> {
        self.sessions.iter().filter(|s| s.stage == stage).collect()
    }

    /// Prüft ob eine Stage die finale Stage ist.
    pub fn is_final_stage(&self, stage: i32) -> bool {
        stage == self.last_stage
    }

    /// Gibt die nächste Stage nach der angegebenen zurück.
    pub fn next_stage(&self, current: i32) -> Option<i32> {
        self.stages.iter().find(|&&s| s > current).copied()
    }

    /// Gibt die erste Stage zurück.
    pub fn first_stage(&self) -> Option<i32> {
        self.stages.first().copied()
    }
}

/// Eine gewünschte Mutagen-Session.
///
/// Repräsentiert eine einzelne Sync-Session, die existieren sollte.
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
    /// Stage-Nummer
    pub stage: i32,
    /// Ignore-Patterns
    pub ignore: Vec<String>,
    /// Polling-Konfiguration
    pub polling: PollingConfig,
    /// Hash der relevanten Config-Felder (für Change Detection)
    pub config_hash: u64,
}

impl DesiredSession {
    /// Erstellt eine DesiredSession aus einem DiscoveredProject.
    pub fn from_project(project: &DiscoveredProject) -> Self {
        Self {
            name: project.session_name(),
            project_name: project.project.name.clone(),
            alpha: project.resolved_directory.clone(),
            beta: project.project.target.clone(),
            mode: project.project.mode.clone(),
            stage: project.project.stage,
            ignore: project.project.ignore.clone(),
            polling: project.project.polling.clone(),
            config_hash: Self::compute_config_hash(project),
        }
    }

    /// Berechnet einen Hash über alle relevanten Config-Felder.
    ///
    /// Wird verwendet um zu erkennen, ob sich die Konfiguration geändert hat.
    fn compute_config_hash(project: &DiscoveredProject) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Diese Felder bestimmen den Hash:
        project.resolved_directory.hash(&mut hasher);
        project.project.target.hash(&mut hasher);
        format!("{:?}", project.project.mode).hash(&mut hasher);
        project.project.ignore.hash(&mut hasher);
        project.project.polling.enabled.hash(&mut hasher);
        project.project.polling.interval.hash(&mut hasher);

        hasher.finish()
    }

    /// Prefix für Session-Namen (project_name + "-")
    pub fn name_prefix(&self) -> String {
        format!("{}-", self.project_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebdev_mutagen_config::MutagenSyncProject;

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
            config_path: PathBuf::from(format!("{}/.ebdev.toml", root)),
            root_config_path: PathBuf::from(format!("{}/.ebdev.toml", root)),
        }
    }

    #[test]
    fn test_desired_state_from_empty_projects() {
        let projects: Vec<DiscoveredProject> = vec![];
        let state = DesiredState::from_projects(&projects);

        assert!(state.sessions.is_empty());
        assert!(state.stages.is_empty());
    }

    #[test]
    fn test_desired_state_from_single_project() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let state = DesiredState::from_projects(&[project]);

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.stages, vec![0]);
        assert_eq!(state.last_stage, 0);
    }

    #[test]
    fn test_desired_state_from_multiple_stages() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1a = make_project("frontend", "docker://app", 1, "/root");
        let p1b = make_project("backend", "docker://app", 1, "/root");
        let p2 = make_project("config", "docker://app", 2, "/root");

        let state = DesiredState::from_projects(&[p0, p1a, p1b, p2]);

        assert_eq!(state.sessions.len(), 4);
        assert_eq!(state.stages, vec![0, 1, 2]);
        assert_eq!(state.last_stage, 2);
    }

    #[test]
    fn test_sessions_for_stage() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1a = make_project("frontend", "docker://app", 1, "/root");
        let p1b = make_project("backend", "docker://app", 1, "/root");

        let state = DesiredState::from_projects(&[p0, p1a, p1b]);

        assert_eq!(state.sessions_for_stage(0).len(), 1);
        assert_eq!(state.sessions_for_stage(1).len(), 2);
        assert_eq!(state.sessions_for_stage(2).len(), 0);
    }

    #[test]
    fn test_is_final_stage() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");

        let state = DesiredState::from_projects(&[p0, p1]);

        assert!(!state.is_final_stage(0));
        assert!(state.is_final_stage(1));
    }

    #[test]
    fn test_next_stage() {
        let p0 = make_project("shared", "docker://app", 0, "/root");
        let p1 = make_project("app", "docker://app", 1, "/root");
        let p2 = make_project("config", "docker://app", 2, "/root");

        let state = DesiredState::from_projects(&[p0, p1, p2]);

        assert_eq!(state.next_stage(0), Some(1));
        assert_eq!(state.next_stage(1), Some(2));
        assert_eq!(state.next_stage(2), None);
    }

    #[test]
    fn test_desired_session_from_project() {
        let project = make_project("frontend", "docker://app", 0, "/root");
        let session = DesiredSession::from_project(&project);

        assert_eq!(session.project_name, "frontend");
        assert_eq!(session.beta, "docker://app");
        assert_eq!(session.stage, 0);
        assert_eq!(session.mode, SyncMode::TwoWay);
    }

    #[test]
    fn test_config_hash_changes_with_target() {
        let p1 = make_project("test", "docker://target-v1", 0, "/root");
        let p2 = make_project("test", "docker://target-v2", 0, "/root");

        let s1 = DesiredSession::from_project(&p1);
        let s2 = DesiredSession::from_project(&p2);

        assert_ne!(s1.config_hash, s2.config_hash);
    }

    #[test]
    fn test_config_hash_same_for_identical_config() {
        let p1 = make_project("test", "docker://target", 0, "/root");
        let p2 = make_project("test", "docker://target", 0, "/root");

        let s1 = DesiredSession::from_project(&p1);
        let s2 = DesiredSession::from_project(&p2);

        assert_eq!(s1.config_hash, s2.config_hash);
    }
}
