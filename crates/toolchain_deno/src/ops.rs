use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use ebdev_task_runner::{Command, TaskRunnerHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// State stored in Deno runtime for task runner ops
#[derive(Default)]
pub struct TaskRunnerState {
    /// Handle to send commands to the task runner
    pub handle: Option<TaskRunnerHandle>,
    /// Working directory override
    pub cwd: Option<String>,
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
    let (handle, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone())
    };

    let command = Command::Exec {
        cmd: args.cmd.clone(),
        cwd: args.cwd.or(state_cwd),
        env: args.env,
        name: args.name.clone(),
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
    };

    execute_command(handle, command, args.ignore_error, args.name.unwrap_or_else(|| args.cmd.join(" "))).await
}

#[op2(async)]
#[serde]
pub async fn op_shell(
    state: Rc<RefCell<OpState>>,
    #[serde] args: ShellArgs,
) -> Result<ExecResult, JsErrorBox> {
    let (handle, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone())
    };

    let display_name = args.name.clone().unwrap_or_else(|| {
        if args.script.len() > 40 {
            format!("{}...", &args.script[..37])
        } else {
            args.script.clone()
        }
    });

    let command = Command::Shell {
        script: args.script,
        cwd: args.cwd.or(state_cwd),
        env: args.env,
        name: args.name,
        timeout: args.timeout.map(Duration::from_secs),
        ignore_error: args.ignore_error,
    };

    execute_command(handle, command, args.ignore_error, display_name).await
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
    };

    execute_command(handle, command, args.ignore_error, display_name).await
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
    };

    execute_command(handle, command, args.ignore_error, display_name).await
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

async fn execute_command(
    handle: Option<TaskRunnerHandle>,
    command: Command,
    ignore_error: bool,
    display_name: String,
) -> Result<ExecResult, JsErrorBox> {
    let h = handle.ok_or_else(|| {
        JsErrorBox::generic("No task runner handle. Tasks must be run via 'ebdev task'.")
    })?;

    let result = h.execute(command).await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    // Check for failure
    if !ignore_error {
        if result.timed_out {
            return Err(JsErrorBox::generic(format!(
                "Command '{}' timed out",
                display_name
            )));
        }
        if !result.success {
            return Err(JsErrorBox::generic(format!(
                "Command '{}' failed with exit code {}",
                display_name,
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

/// Initialize the task runner state in OpState
pub fn init_task_runner_state(
    state: &mut OpState,
    handle: Option<TaskRunnerHandle>,
    cwd: Option<String>,
) {
    state.put(TaskRunnerState { handle, cwd });
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
    ],
);
