use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;

/// Unique ID for commands
pub type CommandId = u64;

/// Default timeout for commands (5 minutes)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// A command to be executed
#[derive(Debug, Clone)]
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
                let mut args = vec!["exec".to_string()];
                if let Some(u) = user {
                    args.push("-u".to_string());
                    args.push(u.clone());
                }
                if let Some(e) = env {
                    for (k, v) in e {
                        args.push("-e".to_string());
                        args.push(format!("{}={}", k, v));
                    }
                }
                args.push(container.clone());
                args.extend(cmd.clone());
                ("docker".to_string(), args)
            }
            Command::DockerRun { image, cmd, volumes, workdir, network, env, .. } => {
                let mut args = vec!["run".to_string(), "--rm".to_string()];
                if let Some(vols) = volumes {
                    for v in vols {
                        args.push("-v".to_string());
                        args.push(v.clone());
                    }
                }
                if let Some(w) = workdir {
                    args.push("-w".to_string());
                    args.push(w.clone());
                }
                if let Some(n) = network {
                    args.push("--network".to_string());
                    args.push(n.clone());
                }
                if let Some(e) = env {
                    for (k, v) in e {
                        args.push("-e".to_string());
                        args.push(format!("{}={}", k, v));
                    }
                }
                args.push(image.clone());
                args.extend(cmd.clone());
                ("docker".to_string(), args)
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
    /// Shutdown the executor
    Shutdown,
}

/// Events from TUI back to TypeScript
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// A registered task was triggered by the user
    TaskTriggered { name: String },
}
