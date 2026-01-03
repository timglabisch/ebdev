use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Unique ID for each task
static TASK_ID: AtomicU64 = AtomicU64::new(1);

fn next_task_id() -> u64 {
    TASK_ID.fetch_add(1, Ordering::SeqCst)
}

/// Events sent from ops to the task runner for visualization
#[derive(Debug, Clone)]
pub enum TaskEvent {
    /// A new task started
    TaskStarted {
        id: u64,
        name: String,
        cmd_display: String,
    },
    /// Task completed successfully
    TaskCompleted {
        id: u64,
        exit_code: i32,
    },
    /// Task failed
    TaskFailed {
        id: u64,
        error: String,
    },
    /// Output from a task
    TaskOutput {
        id: u64,
        stdout: String,
        stderr: String,
    },
    /// Begin parallel group
    ParallelBegin {
        count: usize,
    },
    /// End parallel group
    ParallelEnd,
}

/// Channel for sending task events
pub type TaskEventSender = mpsc::UnboundedSender<TaskEvent>;

/// State stored in Deno runtime for task runner ops
#[derive(Default)]
pub struct TaskRunnerState {
    /// Event sender for visualization (None = no TUI)
    pub event_sender: Option<TaskEventSender>,
    /// Working directory override
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExecArgs {
    cmd: Vec<String>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ShellArgs {
    script: String,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DockerExecArgs {
    container: String,
    cmd: Vec<String>,
    user: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DockerRunArgs {
    image: String,
    cmd: Vec<String>,
    volumes: Option<Vec<String>>,
    workdir: Option<String>,
    network: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExecResult {
    #[serde(rename = "exitCode")]
    exit_code: i32,
    stdout: String,
    stderr: String,
}

#[op2(async)]
#[serde]
pub async fn op_exec(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ExecArgs,
) -> Result<ExecResult, JsErrorBox> {
    let task_id = next_task_id();
    let display_name = args.name.clone().unwrap_or_else(|| args.cmd.join(" "));
    let cmd_display = args.cmd.join(" ");

    let (event_sender, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.event_sender.clone(), runner_state.cwd.clone())
    };

    // Send start event
    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::TaskStarted {
            id: task_id,
            name: display_name.clone(),
            cmd_display: cmd_display.clone(),
        });
    }

    let cwd = args.cwd.as_deref().or(state_cwd.as_deref());
    let result = run_command(&args.cmd, cwd, args.env.as_ref()).await;

    match &result {
        Ok(res) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskOutput {
                    id: task_id,
                    stdout: res.stdout.clone(),
                    stderr: res.stderr.clone(),
                });
                let _ = sender.send(TaskEvent::TaskCompleted {
                    id: task_id,
                    exit_code: res.exit_code,
                });
            }
        }
        Err(e) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskFailed {
                    id: task_id,
                    error: e.to_string(),
                });
            }
        }
    }

    result
}

#[op2(async)]
#[serde]
pub async fn op_shell(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ShellArgs,
) -> Result<ExecResult, JsErrorBox> {
    let task_id = next_task_id();
    let display_name = args.name.clone().unwrap_or_else(|| {
        if args.script.len() > 40 {
            format!("{}...", &args.script[..37])
        } else {
            args.script.clone()
        }
    });

    let (event_sender, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.event_sender.clone(), runner_state.cwd.clone())
    };

    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::TaskStarted {
            id: task_id,
            name: display_name.clone(),
            cmd_display: args.script.clone(),
        });
    }

    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let shell_arg = if cfg!(windows) { "/C" } else { "-c" };

    let cwd = args.cwd.as_deref().or(state_cwd.as_deref());
    let result = run_command(
        &[shell.to_string(), shell_arg.to_string(), args.script],
        cwd,
        args.env.as_ref(),
    ).await;

    match &result {
        Ok(res) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskOutput {
                    id: task_id,
                    stdout: res.stdout.clone(),
                    stderr: res.stderr.clone(),
                });
                let _ = sender.send(TaskEvent::TaskCompleted {
                    id: task_id,
                    exit_code: res.exit_code,
                });
            }
        }
        Err(e) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskFailed {
                    id: task_id,
                    error: e.to_string(),
                });
            }
        }
    }

    result
}

#[op2(async)]
#[serde]
pub async fn op_docker_exec(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerExecArgs,
) -> Result<ExecResult, JsErrorBox> {
    let task_id = next_task_id();
    let display_name = args.name.clone().unwrap_or_else(|| {
        format!("docker:{} {}", args.container, args.cmd.join(" "))
    });

    let (event_sender, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.event_sender.clone(), runner_state.cwd.clone())
    };

    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::TaskStarted {
            id: task_id,
            name: display_name.clone(),
            cmd_display: format!("docker exec {} {}", args.container, args.cmd.join(" ")),
        });
    }

    let mut docker_cmd = vec![
        "docker".to_string(),
        "exec".to_string(),
    ];

    if let Some(user) = &args.user {
        docker_cmd.push("-u".to_string());
        docker_cmd.push(user.clone());
    }

    if let Some(env) = &args.env {
        for (k, v) in env {
            docker_cmd.push("-e".to_string());
            docker_cmd.push(format!("{}={}", k, v));
        }
    }

    docker_cmd.push(args.container);
    docker_cmd.extend(args.cmd);

    let result = run_command(&docker_cmd, state_cwd.as_deref(), None).await;

    match &result {
        Ok(res) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskOutput {
                    id: task_id,
                    stdout: res.stdout.clone(),
                    stderr: res.stderr.clone(),
                });
                let _ = sender.send(TaskEvent::TaskCompleted {
                    id: task_id,
                    exit_code: res.exit_code,
                });
            }
        }
        Err(e) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskFailed {
                    id: task_id,
                    error: e.to_string(),
                });
            }
        }
    }

    result
}

#[op2(async)]
#[serde]
pub async fn op_docker_run(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerRunArgs,
) -> Result<ExecResult, JsErrorBox> {
    let task_id = next_task_id();
    let display_name = args.name.clone().unwrap_or_else(|| {
        format!("docker:{} {}", args.image, args.cmd.join(" "))
    });

    let (event_sender, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.event_sender.clone(), runner_state.cwd.clone())
    };

    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::TaskStarted {
            id: task_id,
            name: display_name.clone(),
            cmd_display: format!("docker run {} {}", args.image, args.cmd.join(" ")),
        });
    }

    let mut docker_cmd = vec![
        "docker".to_string(),
        "run".to_string(),
        "--rm".to_string(),
    ];

    if let Some(volumes) = &args.volumes {
        for v in volumes {
            docker_cmd.push("-v".to_string());
            docker_cmd.push(v.clone());
        }
    }

    if let Some(workdir) = &args.workdir {
        docker_cmd.push("-w".to_string());
        docker_cmd.push(workdir.clone());
    }

    if let Some(network) = &args.network {
        docker_cmd.push("--network".to_string());
        docker_cmd.push(network.clone());
    }

    if let Some(env) = &args.env {
        for (k, v) in env {
            docker_cmd.push("-e".to_string());
            docker_cmd.push(format!("{}={}", k, v));
        }
    }

    docker_cmd.push(args.image);
    docker_cmd.extend(args.cmd);

    let result = run_command(&docker_cmd, state_cwd.as_deref(), None).await;

    match &result {
        Ok(res) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskOutput {
                    id: task_id,
                    stdout: res.stdout.clone(),
                    stderr: res.stderr.clone(),
                });
                let _ = sender.send(TaskEvent::TaskCompleted {
                    id: task_id,
                    exit_code: res.exit_code,
                });
            }
        }
        Err(e) => {
            if let Some(sender) = &event_sender {
                let _ = sender.send(TaskEvent::TaskFailed {
                    id: task_id,
                    error: e.to_string(),
                });
            }
        }
    }

    result
}

#[op2(async)]
pub async fn op_parallel_begin(
    state: Rc<RefCell<OpState>>,
    #[bigint] count: u64,
) -> Result<(), JsErrorBox> {
    let event_sender = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().event_sender.clone()
    };

    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::ParallelBegin { count: count as usize });
    }
    Ok(())
}

#[op2(async)]
pub async fn op_parallel_end(
    state: Rc<RefCell<OpState>>,
) -> Result<(), JsErrorBox> {
    let event_sender = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().event_sender.clone()
    };

    if let Some(sender) = &event_sender {
        let _ = sender.send(TaskEvent::ParallelEnd);
    }
    Ok(())
}

async fn run_command(
    cmd: &[String],
    cwd: Option<&str>,
    env: Option<&HashMap<String, String>>,
) -> Result<ExecResult, JsErrorBox> {
    if cmd.is_empty() {
        return Err(JsErrorBox::generic("Empty command"));
    }

    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    // Inherit stdio so output is visible in terminal
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    if let Some(env) = env {
        for (k, v) in env {
            command.env(k, v);
        }
    }

    let status = command.status().await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    Ok(ExecResult {
        exit_code: status.code().unwrap_or(-1),
        stdout: String::new(), // Output goes directly to terminal
        stderr: String::new(),
    })
}

/// Initialize the task runner state in OpState
pub fn init_task_runner_state(
    state: &mut OpState,
    event_sender: Option<TaskEventSender>,
    cwd: Option<String>,
) {
    state.put(TaskRunnerState { event_sender, cwd });
}

deno_core::extension!(
    ebdev_task_runner,
    ops = [
        op_exec,
        op_shell,
        op_docker_exec,
        op_docker_run,
        op_parallel_begin,
        op_parallel_end,
    ],
);
