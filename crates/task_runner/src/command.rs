use ebdev_remote::OutputStream;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Unique ID for commands
pub type CommandId = u64;

/// Default timeout for commands (5 minutes)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// A command to be executed
#[derive(Debug, Clone, Serialize)]
pub enum Command {
    /// Execute a local command
    Exec {
        cmd: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        name: Option<String>,
        timeout: Option<Duration>,
        ignore_error: bool,
        interactive: bool,
    },
    /// Execute a shell script
    Shell {
        script: String,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        name: Option<String>,
        timeout: Option<Duration>,
        ignore_error: bool,
        interactive: bool,
    },
    /// Execute in a docker container
    DockerExec {
        container: String,
        cmd: Vec<String>,
        user: Option<String>,
        env: Option<HashMap<String, String>>,
        name: Option<String>,
        timeout: Option<Duration>,
        ignore_error: bool,
        interactive: bool,
    },
    /// Run a new docker container
    DockerRun {
        image: String,
        cmd: Vec<String>,
        volumes: Option<Vec<String>>,
        workdir: Option<String>,
        network: Option<String>,
        env: Option<HashMap<String, String>>,
        name: Option<String>,
        timeout: Option<Duration>,
        ignore_error: bool,
        interactive: bool,
    },
}

impl Command {
    /// Get the display name for this command
    pub fn display_name(&self) -> String {
        match self {
            Command::Exec { cmd, name, .. } => {
                name.clone().unwrap_or_else(|| cmd.join(" "))
            }
            Command::Shell { script, name, .. } => {
                name.clone().unwrap_or_else(|| {
                    if script.len() > 40 {
                        format!("{}...", &script[..37])
                    } else {
                        script.clone()
                    }
                })
            }
            Command::DockerExec { container, cmd, name, .. } => {
                name.clone().unwrap_or_else(|| {
                    format!("docker:{} {}", container, cmd.join(" "))
                })
            }
            Command::DockerRun { image, cmd, name, .. } => {
                name.clone().unwrap_or_else(|| {
                    format!("docker:{} {}", image, cmd.join(" "))
                })
            }
        }
    }

    /// Get the timeout for this command
    pub fn timeout(&self) -> Duration {
        match self {
            Command::Exec { timeout, .. } => timeout.unwrap_or(DEFAULT_TIMEOUT),
            Command::Shell { timeout, .. } => timeout.unwrap_or(DEFAULT_TIMEOUT),
            Command::DockerExec { timeout, .. } => timeout.unwrap_or(DEFAULT_TIMEOUT),
            Command::DockerRun { timeout, .. } => timeout.unwrap_or(DEFAULT_TIMEOUT),
        }
    }

    /// Check if errors should be ignored for this command
    pub fn ignore_error(&self) -> bool {
        match self {
            Command::Exec { ignore_error, .. } => *ignore_error,
            Command::Shell { ignore_error, .. } => *ignore_error,
            Command::DockerExec { ignore_error, .. } => *ignore_error,
            Command::DockerRun { ignore_error, .. } => *ignore_error,
        }
    }

    /// Check if this command should run interactively (with real terminal)
    pub fn interactive(&self) -> bool {
        match self {
            Command::Exec { interactive, .. } => *interactive,
            Command::Shell { interactive, .. } => *interactive,
            Command::DockerExec { interactive, .. } => *interactive,
            Command::DockerRun { interactive, .. } => *interactive,
        }
    }
}

/// Result of command execution
#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub success: bool,
    pub timed_out: bool,
    /// Captured stdout output (with PTY: combined stdout+stderr)
    pub stdout: String,
    /// Captured stderr output (with PTY: empty, since PTY merges streams)
    pub stderr: String,
}

/// Event sent to streaming output channel
pub enum OutputEvent {
    /// Output chunk with stream type
    Output { stream: OutputStream, data: Vec<u8> },
    /// Command finished
    Done(CommandResult),
}

/// A request to execute a command, with a channel to send the result back
pub struct CommandRequest {
    pub id: CommandId,
    pub command: Command,
    pub result_tx: oneshot::Sender<CommandResult>,
    /// Optional channel for streaming output events (used by onOutput/onStdout/onStderr callbacks)
    pub output_tx: Option<mpsc::UnboundedSender<OutputEvent>>,
}

/// A registered task that can be triggered from the TUI
#[derive(Debug, Clone, Serialize)]
pub struct RegisteredTask {
    pub name: String,
    pub description: String,
}

/// Display data for a feature flag in the TUI
#[derive(Debug, Clone)]
pub struct FlagDisplay {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub default_enabled: bool,
    pub requires: Vec<String>,
    /// Original saved value from flags.json (preserved for round-trip fidelity)
    pub saved_value: Option<serde_json::Value>,
}

/// Control messages from Deno to the executor
pub enum ExecutorMessage {
    /// Execute a command
    Execute(CommandRequest),
    /// Begin a parallel group
    ParallelBegin { count: usize },
    /// End a parallel group
    ParallelEnd,
    /// Begin a new stage (collapses previous stage, shows new header)
    StageBegin { name: String },
    /// Register a task that can be triggered from TUI
    TaskRegister { name: String, description: String },
    /// Unregister a task
    TaskUnregister { name: String },
    /// Log a message (works correctly in both headless and TUI mode)
    Log { message: String },
    /// Update mutagen sync status in the TUI
    MutagenSyncStatus { sessions: Vec<MutagenSessionProgress> },
    /// Clear the mutagen sync widget
    MutagenSyncClear,
    /// Set compact mode (hide/show sidebar)
    CompactMode { enabled: bool },
    /// Clear all completed stages from the task list
    ClearCompleted,
    /// Kill a running task
    Kill { id: CommandId },
    /// Set feature flags for the Flags tab
    SetFlags { flags: Vec<FlagDisplay> },
    /// Shutdown the executor
    Shutdown,
}

/// Sync-Phase einer Mutagen-Session (vereinfacht für Display)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MutagenSyncPhase {
    /// Noch nicht verbunden (dim)
    Pending,
    /// Aktiv: scanning/syncing (gelb)
    Active,
    /// Fertig: watching/waiting-for-rescan (grün)
    Ready,
    /// Fehler: halted (rot)
    Halted(String),
}

/// Progress-Info für eine einzelne Mutagen-Session
#[derive(Debug, Clone, Serialize)]
pub struct MutagenSessionProgress {
    /// Kurzname (z.B. "app", nicht der volle Session-Name mit CRC)
    pub name: String,
    /// Aktuelle Phase
    pub phase: MutagenSyncPhase,
    /// Lesbarer Status-Text (z.B. "watching", "scanning", "connecting")
    pub status_label: String,
    /// Fortschritt in Prozent (0-100)
    pub percent: u8,
    /// Current file being staged (only during staging)
    pub current_file: Option<String>,
    /// Number of files received so far
    pub files_done: u64,
    /// Total number of files expected
    pub files_total: u64,
    /// Total bytes received so far
    pub total_received_bytes: u64,
    /// Sum of files across both endpoints (grows during scanning)
    pub endpoint_files: u64,
    /// Sum of directories across both endpoints (grows during scanning)
    pub endpoint_dirs: u64,
    /// Polling interval in seconds, if polling is enabled for this session
    pub polling_interval: Option<u32>,
    /// Short sync mode label (e.g., "2way", "1w-create", "1w-replica")
    pub sync_mode: Option<String>,
}

/// Events from TUI back to TypeScript
#[derive(Debug, Clone, Serialize)]
pub enum TuiEvent {
    /// A registered task was triggered by the user
    TaskTriggered { name: String },
}

/// Serializable version of ExecutorMessage for debug logging
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum DebugMessage {
    Execute {
        id: CommandId,
        command: Command,
    },
    ParallelBegin {
        count: usize,
    },
    ParallelEnd,
    StageBegin {
        name: String,
    },
    TaskRegister {
        name: String,
        description: String,
    },
    TaskUnregister {
        name: String,
    },
    Log {
        message: String,
    },
    MutagenSyncStatus {
        session_count: usize,
    },
    MutagenSyncClear,
    Shutdown,
    /// PTY output event
    PtyOutput {
        id: CommandId,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_utf8: Option<String>,
        data_len: usize,
    },
    /// PTY completed event
    PtyCompleted {
        id: CommandId,
        result: CommandResult,
    },
    /// PTY error event
    PtyError {
        id: CommandId,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutagen_sync_phase_equality() {
        assert_eq!(MutagenSyncPhase::Ready, MutagenSyncPhase::Ready);
        assert_eq!(MutagenSyncPhase::Pending, MutagenSyncPhase::Pending);
        assert_ne!(MutagenSyncPhase::Active, MutagenSyncPhase::Ready);
        assert_ne!(
            MutagenSyncPhase::Halted("a".into()),
            MutagenSyncPhase::Halted("b".into())
        );
    }
}
