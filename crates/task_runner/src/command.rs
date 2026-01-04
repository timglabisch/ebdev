use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;

/// Escape a string for safe use in a shell command (single-quote escaping).
/// This wraps the string in single quotes and escapes any embedded single quotes.
/// In single quotes, all characters are literal except single quote itself.
/// To include a single quote: end quote, escaped quote, start quote: '\''
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace("'", "'\\''"))
}

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

    /// Convert to actual command line arguments
    pub fn to_cmd_args(&self) -> (String, Vec<String>) {
        match self {
            Command::Exec { cmd, .. } => {
                (cmd[0].clone(), cmd[1..].to_vec())
            }
            Command::Shell { script, .. } => {
                let shell = if cfg!(windows) { "cmd" } else { "sh" };
                let arg = if cfg!(windows) { "/C" } else { "-c" };
                (shell.to_string(), vec![arg.to_string(), script.clone()])
            }
            Command::DockerExec { container, cmd, user, env, .. } => {
                // Build docker exec command string with proper shell escaping
                // -t allocates a pseudo-TTY so programs output colored text
                let mut docker_cmd = String::from("docker exec -t");
                if let Some(u) = user {
                    docker_cmd.push_str(&format!(" -u {}", shell_escape(u)));
                }
                if let Some(e) = env {
                    for (k, v) in e {
                        docker_cmd.push_str(&format!(" -e {}={}", shell_escape(k), shell_escape(v)));
                    }
                }
                docker_cmd.push_str(&format!(" {}", shell_escape(container)));
                for arg in cmd {
                    docker_cmd.push_str(&format!(" {}", shell_escape(arg)));
                }
                // Wrap in shell like shell() does - this fixes PTY handling
                let shell = if cfg!(windows) { "cmd" } else { "sh" };
                let arg = if cfg!(windows) { "/C" } else { "-c" };
                (shell.to_string(), vec![arg.to_string(), docker_cmd])
            }
            Command::DockerRun { image, cmd, volumes, workdir, network, env, .. } => {
                // Build docker run command string with proper shell escaping
                // -t allocates a pseudo-TTY so programs output colored text
                let mut docker_cmd = String::from("docker run --rm -t");
                if let Some(vols) = volumes {
                    for v in vols {
                        docker_cmd.push_str(&format!(" -v {}", shell_escape(v)));
                    }
                }
                if let Some(w) = workdir {
                    docker_cmd.push_str(&format!(" -w {}", shell_escape(w)));
                }
                if let Some(n) = network {
                    docker_cmd.push_str(&format!(" --network {}", shell_escape(n)));
                }
                if let Some(e) = env {
                    for (k, v) in e {
                        docker_cmd.push_str(&format!(" -e {}={}", shell_escape(k), shell_escape(v)));
                    }
                }
                docker_cmd.push_str(&format!(" {}", shell_escape(image)));
                for arg in cmd {
                    docker_cmd.push_str(&format!(" {}", shell_escape(arg)));
                }
                // Wrap in shell like shell() does - this fixes PTY handling
                let shell = if cfg!(windows) { "cmd" } else { "sh" };
                let arg = if cfg!(windows) { "/C" } else { "-c" };
                (shell.to_string(), vec![arg.to_string(), docker_cmd])
            }
        }
    }

    /// Get the working directory for this command
    pub fn cwd(&self) -> Option<&str> {
        match self {
            Command::Exec { cwd, .. } => cwd.as_deref(),
            Command::Shell { cwd, .. } => cwd.as_deref(),
            Command::DockerExec { .. } => None,
            Command::DockerRun { .. } => None,
        }
    }

    /// Get environment variables for this command
    pub fn env(&self) -> Option<&HashMap<String, String>> {
        match self {
            Command::Exec { env, .. } => env.as_ref(),
            Command::Shell { env, .. } => env.as_ref(),
            Command::DockerExec { .. } => None, // Handled in to_cmd_args
            Command::DockerRun { .. } => None,  // Handled in to_cmd_args
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
