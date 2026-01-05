use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;

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
    },
    /// Execute a shell script
    Shell {
        script: String,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        name: Option<String>,
        timeout: Option<Duration>,
        ignore_error: bool,
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
}

/// Result of command execution
#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub success: bool,
    pub timed_out: bool,
}

/// A request to execute a command, with a channel to send the result back
pub struct CommandRequest {
    pub id: CommandId,
    pub command: Command,
    pub result_tx: oneshot::Sender<CommandResult>,
}

/// A registered task that can be triggered from the TUI
#[derive(Debug, Clone, Serialize)]
pub struct RegisteredTask {
    pub name: String,
    pub description: String,
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
    /// Shutdown the executor
    Shutdown,
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
