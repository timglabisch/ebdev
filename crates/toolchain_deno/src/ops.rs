use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use ebdev_mutagen_runner::{reconcile_sessions, state::DesiredSession, SessionStatusInfo, SyncMode};
use ebdev_task_runner::{Command, TaskRunnerHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// State stored in Deno runtime for task runner ops
#[derive(Default)]
pub struct TaskRunnerState {
    /// Handle to send commands to the task runner
    pub handle: Option<TaskRunnerHandle>,
    /// Working directory override
    pub cwd: Option<String>,
    /// Extra environment variables injected into every exec/shell command
    pub env: HashMap<String, String>,
}

/// State stored in Deno runtime for mutagen ops
pub struct MutagenState {
    /// Path to the mutagen binary
    pub mutagen_path: Option<PathBuf>,
    /// Path to the .ebdev.ts config file (for computing project CRC32)
    pub config_path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct ExecArgs {
    cmd: Vec<String>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
    #[serde(default)]
    timeout: Option<u64>, // in seconds
    #[serde(default)]
    ignore_error: bool,
    #[serde(default)]
    interactive: bool,
}

#[derive(Debug, Deserialize)]
pub struct ShellArgs {
    script: String,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    ignore_error: bool,
    #[serde(default)]
    interactive: bool,
}

#[derive(Debug, Deserialize)]
pub struct DockerExecArgs {
    container: String,
    cmd: Vec<String>,
    user: Option<String>,
    env: Option<HashMap<String, String>>,
    name: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    ignore_error: bool,
    #[serde(default)]
    interactive: bool,
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
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    ignore_error: bool,
    #[serde(default)]
    interactive: bool,
}

#[derive(Debug, Serialize)]
pub struct ExecResult {
    #[serde(rename = "exitCode")]
    exit_code: i32,
    success: bool,
    #[serde(rename = "timedOut")]
    timed_out: bool,
}

#[op2(async)]
#[serde]
pub async fn op_exec(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ExecArgs,
) -> Result<ExecResult, JsErrorBox> {
    let (handle, state_cwd, state_env) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone(), runner_state.env.clone())
    };

    let command = Command::Exec {
        cmd: args.cmd.clone(),
        cwd: args.cwd.or(state_cwd),
        env: Some(merge_env(&state_env, args.env)),
        name: args.name.clone(),
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
        interactive: args.interactive,
    };

    execute_command(handle, command, args.ignore_error, args.name.unwrap_or_else(|| args.cmd.join(" ")), None).await
}

#[op2(async)]
#[serde]
pub async fn op_shell(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ShellArgs,
) -> Result<ExecResult, JsErrorBox> {
    let (handle, state_cwd, state_env) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone(), runner_state.env.clone())
    };

    let full_script = args.script.clone();
    let display_name = args.name.clone().unwrap_or_else(|| {
        if args.script.len() > 40 {
            format!("{}...", &args.script[..37])
        } else {
            args.script.clone()
        }
    });
    // Pass full script for error messages when display_name is truncated
    let full_command = if full_script.len() > 40 && args.name.is_none() {
        Some(full_script)
    } else {
        None
    };

    let command = Command::Shell {
        script: args.script,
        cwd: args.cwd.or(state_cwd),
        env: Some(merge_env(&state_env, args.env)),
        name: args.name,
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
        interactive: args.interactive,
    };

    execute_command(handle, command, args.ignore_error, display_name, full_command).await
}

#[op2(async)]
#[serde]
pub async fn op_docker_exec(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerExecArgs,
) -> Result<ExecResult, JsErrorBox> {
    let handle = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        runner_state.handle.clone()
    };

    let display_name = args.name.clone().unwrap_or_else(|| {
        format!("docker exec {} {}", args.container, args.cmd.join(" "))
    });

    let command = Command::DockerExec {
        container: args.container,
        cmd: args.cmd,
        user: args.user,
        env: args.env,
        name: args.name,
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
        interactive: args.interactive,
    };

    execute_command(handle, command, args.ignore_error, display_name, None).await
}

#[op2(async)]
#[serde]
pub async fn op_docker_run(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerRunArgs,
) -> Result<ExecResult, JsErrorBox> {
    let handle = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        runner_state.handle.clone()
    };

    let display_name = args.name.clone().unwrap_or_else(|| {
        format!("docker run {} {}", args.image, args.cmd.join(" "))
    });

    let command = Command::DockerRun {
        image: args.image,
        cmd: args.cmd,
        volumes: args.volumes,
        workdir: args.workdir,
        network: args.network,
        env: args.env,
        name: args.name,
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
        interactive: args.interactive,
    };

    execute_command(handle, command, args.ignore_error, display_name, None).await
}

#[op2(async)]
pub async fn op_parallel_begin(
    state: Rc<RefCell<OpState>>,
    #[bigint] count: u64,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.parallel_begin(count as usize)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
pub async fn op_parallel_end(
    state: Rc<RefCell<OpState>>,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.parallel_end()
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
pub async fn op_stage(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.stage_begin(&name)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
pub async fn op_task_register(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[string] description: String,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.task_register(&name, &description)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
pub async fn op_task_unregister(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.task_unregister(&name)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
#[string]
pub async fn op_poll_task_trigger(
    state: Rc<RefCell<OpState>>,
) -> Result<Option<String>, JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        let result = h.poll_task_trigger().await;
        if result.is_none() {
            // Small delay to prevent busy-waiting when no trigger is available
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(result)
    } else {
        Ok(None)
    }
}

#[op2(async)]
pub async fn op_log(
    state: Rc<RefCell<OpState>>,
    #[string] message: String,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.log(&message)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

// =============================================================================
// Mutagen Reconcile Op
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct MutagenSessionArg {
    name: String,
    target: String,
    directory: String,
    mode: Option<String>,
    ignore: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ReconcileArgs {
    sessions: Vec<MutagenSessionArg>,
    project: Option<String>,
}

#[op2(async)]
pub async fn op_mutagen_reconcile(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ReconcileArgs,
) -> Result<(), JsErrorBox> {
    let (mutagen_path, config_path, handle) = {
        let state = state.borrow();
        let mutagen_state = state.try_borrow::<MutagenState>();
        let runner_state = state.borrow::<TaskRunnerState>();

        let mutagen_state = mutagen_state.ok_or_else(|| {
            JsErrorBox::generic("Mutagen not configured. Ensure mutagen is in toolchain config.")
        })?;

        let mutagen_path = mutagen_state.mutagen_path.clone().ok_or_else(|| {
            JsErrorBox::generic("Mutagen binary not found. Ensure mutagen is in toolchain config.")
        })?;

        (mutagen_path, mutagen_state.config_path.clone(), runner_state.handle.clone())
    };

    // Compute project CRC32 (default: from config path)
    let project_crc32 = if let Some(project) = &args.project {
        // User provided explicit project - use its hash
        crc32fast::hash(project.as_bytes())
    } else {
        // Use CRC32 of absolute config path
        let path_str = config_path.to_string_lossy();
        crc32fast::hash(path_str.as_bytes())
    };

    // Build DesiredSessions from args
    let base_dir = config_path.parent().unwrap_or(&config_path);
    let desired_sessions: Vec<DesiredSession> = args
        .sessions
        .into_iter()
        .map(|s| {
            let mode = match s.mode.as_deref() {
                Some("one-way-create") => SyncMode::OneWayCreate,
                Some("one-way-replica") => SyncMode::OneWayReplica,
                _ => SyncMode::TwoWay,
            };

            let alpha = base_dir.join(&s.directory);
            let session_name = format!("{}-{:08x}", s.name, project_crc32);

            DesiredSession::new(
                session_name,
                s.name,
                alpha,
                s.target,
                mode,
                s.ignore.unwrap_or_default(),
            )
        })
        .collect();

    // Run reconcile with status updates
    let handle_clone = handle.clone();
    reconcile_sessions(
        &mutagen_path,
        desired_sessions,
        project_crc32,
        move |statuses: Vec<SessionStatusInfo>| {
            // Send status update to TUI if handle is available
            if let Some(h) = &handle_clone {
                let status_str = statuses
                    .iter()
                    .map(|s| format!("{}:{}", s.name.split('-').next().unwrap_or(&s.name), &s.status))
                    .collect::<Vec<_>>()
                    .join(" | ");
                let _ = h.log(&format!("[mutagen] {}", status_str));
            }
        },
    )
    .await
    .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    Ok(())
}

#[op2(async)]
pub async fn op_mutagen_pause_all(
    state: Rc<RefCell<OpState>>,
) -> Result<u32, JsErrorBox> {
    let (mutagen_path, config_path) = {
        let state = state.borrow();
        let mutagen_state = state.try_borrow::<MutagenState>().ok_or_else(|| {
            JsErrorBox::generic("Mutagen not configured. Ensure mutagen is in toolchain config.")
        })?;

        let mutagen_path = mutagen_state.mutagen_path.clone().ok_or_else(|| {
            JsErrorBox::generic("Mutagen binary not found. Ensure mutagen is in toolchain config.")
        })?;

        (mutagen_path, mutagen_state.config_path.clone())
    };

    let path_str = config_path.to_string_lossy();
    let project_crc32 = crc32fast::hash(path_str.as_bytes());

    let paused = ebdev_mutagen_runner::pause_project_sessions(&mutagen_path, project_crc32)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    Ok(paused as u32)
}

// =============================================================================
// Command Execution
// =============================================================================

async fn execute_command(
    handle: Option<TaskRunnerHandle>,
    command: Command,
    ignore_error: bool,
    display_name: String,
    full_command: Option<String>,
) -> Result<ExecResult, JsErrorBox> {
    let h = handle.ok_or_else(|| {
        JsErrorBox::generic("No task runner handle. Tasks must be run via 'ebdev task'.")
    })?;

    let result = h.execute(command).await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    // Check for failure
    if !ignore_error {
        let error_name = full_command.as_deref().unwrap_or(&display_name);
        if result.timed_out {
            return Err(JsErrorBox::generic(format!(
                "Command '{}' timed out",
                error_name
            )));
        }
        if !result.success {
            return Err(JsErrorBox::generic(format!(
                "Command '{}' failed with exit code {}",
                error_name,
                result.exit_code
            )));
        }
    }

    Ok(ExecResult {
        exit_code: result.exit_code,
        success: result.success,
        timed_out: result.timed_out,
    })
}

/// Merge base env vars with command-specific env vars (command wins on conflict)
fn merge_env(base: &HashMap<String, String>, command_env: Option<HashMap<String, String>>) -> HashMap<String, String> {
    let mut merged = base.clone();
    if let Some(env) = command_env {
        merged.extend(env);
    }
    merged
}

/// Initialize the task runner state in OpState
pub fn init_task_runner_state(
    state: &mut OpState,
    handle: Option<TaskRunnerHandle>,
    cwd: Option<String>,
    env: HashMap<String, String>,
) {
    state.put(TaskRunnerState { handle, cwd, env });
}

/// Initialize the mutagen state in OpState
pub fn init_mutagen_state(
    state: &mut OpState,
    mutagen_path: Option<PathBuf>,
    config_path: PathBuf,
) {
    state.put(MutagenState {
        mutagen_path,
        config_path,
    });
}

deno_core::extension!(
    ebdev_deno_ops,
    ops = [
        op_exec,
        op_shell,
        op_docker_exec,
        op_docker_run,
        op_parallel_begin,
        op_parallel_end,
        op_stage,
        op_task_register,
        op_task_unregister,
        op_poll_task_trigger,
        op_log,
        op_mutagen_reconcile,
        op_mutagen_pause_all,
    ],
);
