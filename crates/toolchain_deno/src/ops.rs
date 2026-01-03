use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use ebdev_task_runner::{Command, TaskRunnerHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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
    let (handle, state_cwd) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone())
    };

    let command = Command::Exec {
        cmd: args.cmd,
        cwd: args.cwd.or(state_cwd),
        env: args.env,
        name: args.name,
    };

    execute_command(handle, command).await
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

    let command = Command::Shell {
        script: args.script,
        cwd: args.cwd.or(state_cwd),
        env: args.env,
        name: args.name,
    };

    execute_command(handle, command).await
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

    let command = Command::DockerExec {
        container: args.container,
        cmd: args.cmd,
        user: args.user,
        env: args.env,
        name: args.name,
    };

    execute_command(handle, command).await
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

    let command = Command::DockerRun {
        image: args.image,
        cmd: args.cmd,
        volumes: args.volumes,
        workdir: args.workdir,
        network: args.network,
        env: args.env,
        name: args.name,
    };

    execute_command(handle, command).await
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

async fn execute_command(
    handle: Option<TaskRunnerHandle>,
    command: Command,
) -> Result<ExecResult, JsErrorBox> {
    let h = handle.ok_or_else(|| {
        JsErrorBox::generic("No task runner handle. Tasks must be run via 'ebdev task'.")
    })?;

    let result = h.execute(command).await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    Ok(ExecResult {
        exit_code: result.exit_code,
        stdout: String::new(), // Output goes to UI
        stderr: String::new(),
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
    ],
);
