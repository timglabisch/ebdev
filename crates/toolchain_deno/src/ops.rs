use deno_core::{op2, OpState};
use deno_error::JsErrorBox;
use ebdev_mutagen_runner::{reconcile_sessions, state::DesiredSession, PollingConfig, SessionStatus, SessionStatusInfo, StagingProgress, SyncMode};
use ebdev_task_runner::{Command, MutagenSessionProgress, MutagenSyncPhase, OutputEvent, OutputStream, TaskRunnerHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as TokioMutex};

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

/// State for bridge filesystem operations (holds embedded binary)
pub struct BridgeState {
    pub embedded_linux_binary: &'static [u8],
}

/// State stored in Deno runtime for mutagen ops
pub struct MutagenState {
    /// Path to the mutagen binary
    pub mutagen_path: Option<PathBuf>,
    /// Path to the .ebdev.ts config file (for computing project CRC32)
    pub config_path: PathBuf,
}

/// State for streaming executions
#[derive(Default)]
pub struct StreamingState {
    streams: HashMap<u32, Arc<TokioMutex<mpsc::UnboundedReceiver<OutputEvent>>>>,
    next_id: u32,
}

impl StreamingState {
    fn insert(&mut self, rx: mpsc::UnboundedReceiver<OutputEvent>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.streams.insert(id, Arc::new(TokioMutex::new(rx)));
        id
    }
}

/// Event returned by op_stream_next to JavaScript
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "output")]
    Output {
        stream: String,
        data: String,
    },
    #[serde(rename = "done")]
    Done {
        result: ExecResult,
    },
}

/// Args for op_start_stream (supports all command types)
#[derive(Debug, Deserialize)]
pub struct StreamStartArgs {
    #[serde(rename = "type")]
    command_type: String,
    cmd: Option<Vec<String>>,
    script: Option<String>,
    container: Option<String>,
    image: Option<String>,
    user: Option<String>,
    volumes: Option<Vec<String>>,
    workdir: Option<String>,
    network: Option<String>,
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
    stdout: String,
    stderr: String,
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

#[op2(async)]
pub async fn op_compact_mode(
    state: Rc<RefCell<OpState>>,
    enabled: bool,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.set_compact_mode(enabled)
            .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    }
    Ok(())
}

#[op2(async)]
pub async fn op_clear_completed(
    state: Rc<RefCell<OpState>>,
) -> Result<(), JsErrorBox> {
    let handle = {
        let state = state.borrow();
        state.borrow::<TaskRunnerState>().handle.clone()
    };

    if let Some(h) = handle {
        h.clear_completed()
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
    polling: Option<PollingArg>,
}

#[derive(Debug, Deserialize)]
pub struct PollingArg {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_polling_interval")]
    interval: u32,
}

fn default_polling_interval() -> u32 {
    10
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
    let mut desired_sessions: Vec<DesiredSession> = Vec::new();
    for s in args.sessions {
        let mode = match s.mode.as_deref() {
            Some(m) => SyncMode::parse(m).ok_or_else(|| {
                JsErrorBox::generic(format!(
                    "Unknown sync mode '{}'. Valid modes: {}",
                    m,
                    SyncMode::known_modes().join(", ")
                ))
            })?,
            None => SyncMode::default(),
        };

        let alpha = base_dir.join(&s.directory);
        let session_name = format!("{}-{:08x}", s.name, project_crc32);

        let mut session = DesiredSession::new(
            session_name,
            s.name,
            alpha,
            s.target,
            mode,
            s.ignore.unwrap_or_default(),
        );
        if let Some(p) = s.polling {
            session.polling = PollingConfig {
                enabled: p.enabled,
                interval: p.interval,
            };
        }
        desired_sessions.push(session);
    }

    // Run reconcile with status updates
    let handle_clone = handle.clone();
    reconcile_sessions(
        &mutagen_path,
        desired_sessions,
        project_crc32,
        move |statuses: Vec<SessionStatusInfo>| {
            if let Some(h) = &handle_clone {
                let sessions: Vec<MutagenSessionProgress> = statuses
                    .iter()
                    .map(map_status_info)
                    .collect();
                let _ = h.mutagen_sync_status(sessions);
            }
        },
    )
    .await
    .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    // Clear widget after completion
    if let Some(h) = &handle {
        let _ = h.mutagen_sync_clear();
    }

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

/// Convert a SessionStatusInfo (from mutagen_runner) into a MutagenSessionProgress (for task_runner UI).
fn map_status_info(info: &SessionStatusInfo) -> MutagenSessionProgress {
    let short_name = info.name.split('-').next().unwrap_or(&info.name).to_string();
    let (phase, status_label, base_percent) = map_session_status(&info.status);

    let (current_file, files_done, files_total, total_received_bytes, percent) =
        if let Some(sp) = &info.staging_progress {
            let p = if sp.expected_files > 0 {
                (sp.received_files as f64 / sp.expected_files as f64 * 100.0) as u8
            } else {
                base_percent
            };
            let file = if sp.path.is_empty() { None } else { Some(sp.path.clone()) };
            (file, sp.received_files, sp.expected_files, sp.total_received_size, p)
        } else {
            (None, 0, 0, 0, base_percent)
        };

    MutagenSessionProgress {
        name: short_name,
        phase,
        status_label,
        percent,
        current_file,
        files_done,
        files_total,
        total_received_bytes,
        endpoint_files: info.endpoint_files,
        endpoint_dirs: info.endpoint_dirs,
        polling_interval: info.polling_interval,
        sync_mode: info.sync_mode.clone(),
    }
}

pub(crate) fn map_session_status(status: &SessionStatus) -> (MutagenSyncPhase, String, u8) {
    match status {
        SessionStatus::Watching => (MutagenSyncPhase::Ready, "watching".into(), 100),
        SessionStatus::WaitingForRescan => (MutagenSyncPhase::Ready, "waiting for rescan".into(), 90),
        SessionStatus::Scanning => (MutagenSyncPhase::Active, "scanning".into(), 40),
        SessionStatus::Syncing => (MutagenSyncPhase::Active, "syncing".into(), 70),
        SessionStatus::Connecting => (MutagenSyncPhase::Pending, "connecting".into(), 20),
        SessionStatus::Disconnected => (MutagenSyncPhase::Pending, "disconnected".into(), 0),
        SessionStatus::Halted(msg) => (MutagenSyncPhase::Halted(msg.clone()), format!("halted: {}", msg), 0),
        SessionStatus::Unknown(s) => (MutagenSyncPhase::Pending, s.clone(), 10),
    }
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
        stdout: result.stdout,
        stderr: result.stderr,
    })
}

// =============================================================================
// Streaming Execution Ops
// =============================================================================

#[op2(async)]
pub async fn op_start_stream(
    state: Rc<RefCell<OpState>>,
    #[serde] args: StreamStartArgs,
) -> Result<u32, JsErrorBox> {
    let (handle, state_cwd, state_env) = {
        let state = state.borrow();
        let runner_state = state.borrow::<TaskRunnerState>();
        (runner_state.handle.clone(), runner_state.cwd.clone(), runner_state.env.clone())
    };

    let h = handle.ok_or_else(|| {
        JsErrorBox::generic("No task runner handle. Tasks must be run via 'ebdev task'.")
    })?;

    let command = match args.command_type.as_str() {
        "exec" => Command::Exec {
            cmd: args.cmd.ok_or_else(|| JsErrorBox::generic("cmd required for exec"))?,
            cwd: args.cwd.or(state_cwd),
            env: Some(merge_env(&state_env, args.env)),
            name: args.name,
            timeout: args.timeout.map(Duration::from_secs),
            ignore_error: args.ignore_error,
            interactive: args.interactive,
        },
        "shell" => Command::Shell {
            script: args.script.ok_or_else(|| JsErrorBox::generic("script required for shell"))?,
            cwd: args.cwd.or(state_cwd),
            env: Some(merge_env(&state_env, args.env)),
            name: args.name,
            timeout: args.timeout.map(Duration::from_secs),
            ignore_error: args.ignore_error,
            interactive: args.interactive,
        },
        "docker_exec" => Command::DockerExec {
            container: args.container.ok_or_else(|| JsErrorBox::generic("container required"))?,
            cmd: args.cmd.ok_or_else(|| JsErrorBox::generic("cmd required"))?,
            user: args.user,
            env: args.env,
            name: args.name,
            timeout: args.timeout.map(Duration::from_secs),
            ignore_error: args.ignore_error,
            interactive: args.interactive,
        },
        "docker_run" => Command::DockerRun {
            image: args.image.ok_or_else(|| JsErrorBox::generic("image required"))?,
            cmd: args.cmd.ok_or_else(|| JsErrorBox::generic("cmd required"))?,
            volumes: args.volumes,
            workdir: args.workdir,
            network: args.network,
            env: args.env,
            name: args.name,
            timeout: args.timeout.map(Duration::from_secs),
            ignore_error: args.ignore_error,
            interactive: args.interactive,
        },
        other => return Err(JsErrorBox::generic(format!("Unknown command type: {}", other))),
    };

    let output_rx = h.execute_streaming(command)
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;

    let stream_id = {
        let mut op_state = state.borrow_mut();
        let streaming = op_state.borrow_mut::<StreamingState>();
        streaming.insert(output_rx)
    };

    Ok(stream_id)
}

#[op2(async)]
#[serde]
pub async fn op_stream_next(
    state: Rc<RefCell<OpState>>,
    stream_id: u32,
) -> Result<StreamEvent, JsErrorBox> {
    let rx_arc = {
        let op_state = state.borrow();
        let streaming = op_state.borrow::<StreamingState>();
        streaming.streams.get(&stream_id)
            .cloned()
            .ok_or_else(|| JsErrorBox::generic(format!("Invalid stream ID: {}", stream_id)))?
    };

    let mut rx = rx_arc.lock().await;
    match rx.recv().await {
        Some(OutputEvent::Output { stream, data }) => {
            Ok(StreamEvent::Output {
                stream: match stream {
                    OutputStream::Stdout => "stdout".to_string(),
                    OutputStream::Stderr => "stderr".to_string(),
                },
                data: String::from_utf8_lossy(&data).into_owned(),
            })
        }
        Some(OutputEvent::Done(result)) => {
            drop(rx);
            // Clean up the stream
            {
                let mut op_state = state.borrow_mut();
                let streaming = op_state.borrow_mut::<StreamingState>();
                streaming.streams.remove(&stream_id);
            }
            Ok(StreamEvent::Done {
                result: ExecResult {
                    exit_code: result.exit_code,
                    success: result.success,
                    timed_out: result.timed_out,
                    stdout: result.stdout,
                    stderr: result.stderr,
                },
            })
        }
        None => {
            // Channel closed unexpectedly
            {
                let mut op_state = state.borrow_mut();
                let streaming = op_state.borrow_mut::<StreamingState>();
                streaming.streams.remove(&stream_id);
            }
            Err(JsErrorBox::generic("Stream closed unexpectedly"))
        }
    }
}

// =============================================================================
// Local Filesystem Ops
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct FsPathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
pub struct FsWriteArgs {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct FsMkdirArgs {
    path: String,
    #[serde(default = "default_true")]
    recursive: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
pub struct FsRemoveArgs {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Serialize)]
pub struct StatResult {
    exists: bool,
    #[serde(rename = "isFile")]
    is_file: bool,
    #[serde(rename = "isDir")]
    is_dir: bool,
    size: u64,
}

#[op2(async)]
pub async fn op_fs_write_file(
    #[serde] args: FsWriteArgs,
) -> Result<(), JsErrorBox> {
    tokio::fs::write(&args.path, args.content.as_bytes())
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.writeFile '{}': {}", args.path, e)))
}

#[op2(async)]
#[string]
pub async fn op_fs_read_file(
    #[serde] args: FsPathArgs,
) -> Result<String, JsErrorBox> {
    tokio::fs::read_to_string(&args.path)
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.readFile '{}': {}", args.path, e)))
}

#[op2(async)]
pub async fn op_fs_append_file(
    #[serde] args: FsWriteArgs,
) -> Result<(), JsErrorBox> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.path)
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.appendFile '{}': {}", args.path, e)))?;
    file.write_all(args.content.as_bytes())
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.appendFile '{}': {}", args.path, e)))
}

#[op2(async)]
pub async fn op_fs_mkdir(
    #[serde] args: FsMkdirArgs,
) -> Result<(), JsErrorBox> {
    if args.recursive {
        tokio::fs::create_dir_all(&args.path)
            .await
            .map_err(|e| JsErrorBox::generic(format!("fs.mkdir '{}': {}", args.path, e)))
    } else {
        tokio::fs::create_dir(&args.path)
            .await
            .map_err(|e| JsErrorBox::generic(format!("fs.mkdir '{}': {}", args.path, e)))
    }
}

#[op2(async)]
pub async fn op_fs_remove(
    #[serde] args: FsRemoveArgs,
) -> Result<(), JsErrorBox> {
    if args.recursive {
        tokio::fs::remove_dir_all(&args.path)
            .await
            .map_err(|e| JsErrorBox::generic(format!("fs.rm '{}': {}", args.path, e)))
    } else {
        match tokio::fs::remove_file(&args.path).await {
            Ok(()) => Ok(()),
            Err(_) => tokio::fs::remove_dir(&args.path)
                .await
                .map_err(|e| JsErrorBox::generic(format!("fs.rm '{}': {}", args.path, e))),
        }
    }
}

#[op2(async)]
pub async fn op_fs_exists(
    #[serde] args: FsPathArgs,
) -> Result<bool, JsErrorBox> {
    Ok(tokio::fs::try_exists(&args.path)
        .await
        .unwrap_or(false))
}

#[op2(async)]
#[serde]
pub async fn op_fs_stat(
    #[serde] args: FsPathArgs,
) -> Result<StatResult, JsErrorBox> {
    match tokio::fs::metadata(&args.path).await {
        Ok(meta) => Ok(StatResult {
            exists: true,
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            size: meta.len(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StatResult {
            exists: false,
            is_file: false,
            is_dir: false,
            size: 0,
        }),
        Err(e) => Err(JsErrorBox::generic(format!("fs.stat '{}': {}", args.path, e))),
    }
}

// =============================================================================
// Docker Filesystem Ops
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DockerFsPathArgs {
    container: String,
    path: String,
}

#[derive(Debug, Deserialize)]
pub struct DockerFsWriteArgs {
    container: String,
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct DockerFsMkdirArgs {
    container: String,
    path: String,
    #[serde(default = "default_true")]
    recursive: bool,
}

#[derive(Debug, Deserialize)]
pub struct DockerFsRemoveArgs {
    container: String,
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[op2(async)]
pub async fn op_docker_fs_write_file(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsWriteArgs,
) -> Result<(), JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    ebdev_remote::remote_write_file(&args.container, binary, &args.path, args.content.as_bytes())
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))
}

#[op2(async)]
#[string]
pub async fn op_docker_fs_read_file(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsPathArgs,
) -> Result<String, JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    let data = ebdev_remote::remote_read_file(&args.container, binary, &args.path)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    String::from_utf8(data)
        .map_err(|e| JsErrorBox::generic(format!("docker.fs.readFile: invalid UTF-8: {}", e)))
}

#[op2(async)]
pub async fn op_docker_fs_append_file(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsWriteArgs,
) -> Result<(), JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    ebdev_remote::remote_append_file(&args.container, binary, &args.path, args.content.as_bytes())
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))
}

#[op2(async)]
pub async fn op_docker_fs_mkdir(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsMkdirArgs,
) -> Result<(), JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    ebdev_remote::remote_mkdir(&args.container, binary, &args.path, args.recursive)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))
}

#[op2(async)]
pub async fn op_docker_fs_remove(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsRemoveArgs,
) -> Result<(), JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    ebdev_remote::remote_remove(&args.container, binary, &args.path, args.recursive)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))
}

#[op2(async)]
pub async fn op_docker_fs_exists(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsPathArgs,
) -> Result<bool, JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    let stat = ebdev_remote::remote_stat(&args.container, binary, &args.path)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    Ok(stat.exists)
}

#[op2(async)]
#[serde]
pub async fn op_docker_fs_stat(
    state: Rc<RefCell<OpState>>,
    #[serde] args: DockerFsPathArgs,
) -> Result<StatResult, JsErrorBox> {
    let binary = {
        let state = state.borrow();
        let bridge = state.borrow::<BridgeState>();
        bridge.embedded_linux_binary
    };
    let stat = ebdev_remote::remote_stat(&args.container, binary, &args.path)
        .await
        .map_err(|e| JsErrorBox::generic(e.to_string()))?;
    Ok(StatResult {
        exists: stat.exists,
        is_file: stat.is_file,
        is_dir: stat.is_dir,
        size: stat.size,
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
    state.put(StreamingState::default());
}

/// Initialize the bridge state in OpState
pub fn init_bridge_state(
    state: &mut OpState,
    embedded_linux_binary: &'static [u8],
) {
    state.put(BridgeState { embedded_linux_binary });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_session_status_watching() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Watching);
        assert_eq!(phase, MutagenSyncPhase::Ready);
        assert_eq!(label, "watching");
        assert_eq!(percent, 100);
    }

    #[test]
    fn test_map_session_status_waiting_for_rescan() {
        let (phase, label, percent) = map_session_status(&SessionStatus::WaitingForRescan);
        assert_eq!(phase, MutagenSyncPhase::Ready);
        assert_eq!(label, "waiting for rescan");
        assert_eq!(percent, 90);
    }

    #[test]
    fn test_map_session_status_scanning() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Scanning);
        assert_eq!(phase, MutagenSyncPhase::Active);
        assert_eq!(label, "scanning");
        assert_eq!(percent, 40);
    }

    #[test]
    fn test_map_session_status_syncing() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Syncing);
        assert_eq!(phase, MutagenSyncPhase::Active);
        assert_eq!(label, "syncing");
        assert_eq!(percent, 70);
    }

    #[test]
    fn test_map_session_status_connecting() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Connecting);
        assert_eq!(phase, MutagenSyncPhase::Pending);
        assert_eq!(label, "connecting");
        assert_eq!(percent, 20);
    }

    #[test]
    fn test_map_session_status_disconnected() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Disconnected);
        assert_eq!(phase, MutagenSyncPhase::Pending);
        assert_eq!(label, "disconnected");
        assert_eq!(percent, 0);
    }

    #[test]
    fn test_map_session_status_halted() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Halted("root empty".into()));
        assert_eq!(phase, MutagenSyncPhase::Halted("root empty".into()));
        assert_eq!(label, "halted: root empty");
        assert_eq!(percent, 0);
    }

    #[test]
    fn test_map_session_status_unknown() {
        let (phase, label, percent) = map_session_status(&SessionStatus::Unknown("custom-state".into()));
        assert_eq!(phase, MutagenSyncPhase::Pending);
        assert_eq!(label, "custom-state");
        assert_eq!(percent, 10);
    }

    // ========================================================================
    // map_status_info tests
    // ========================================================================

    #[test]
    fn test_map_status_info_short_name_extraction() {
        let info = SessionStatusInfo {
            name: "frontend-12345678".to_string(),
            status: SessionStatus::Watching,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.name, "frontend");
    }

    #[test]
    fn test_map_status_info_without_staging() {
        let info = SessionStatusInfo {
            name: "app-aabb".to_string(),
            status: SessionStatus::Scanning,
            staging_progress: None,
            endpoint_files: 1200,
            endpoint_dirs: 50,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.phase, MutagenSyncPhase::Active);
        assert_eq!(result.status_label, "scanning");
        assert_eq!(result.percent, 40); // base_percent for scanning
        assert!(result.current_file.is_none());
        assert_eq!(result.files_done, 0);
        assert_eq!(result.files_total, 0);
        assert_eq!(result.endpoint_files, 1200);
        assert_eq!(result.endpoint_dirs, 50);
    }

    #[test]
    fn test_map_status_info_with_staging_progress() {
        let info = SessionStatusInfo {
            name: "allother-12345678".to_string(),
            status: SessionStatus::Syncing,
            staging_progress: Some(StagingProgress {
                path: "vendor/autoload.php".to_string(),
                received_files: 5000,
                expected_files: 10000,
                received_size: 1024,
                expected_size: 1024,
                total_received_size: 500_000,
            }),
            endpoint_files: 80000,
            endpoint_dirs: 2000,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.name, "allother");
        assert_eq!(result.percent, 50); // 5000/10000
        assert_eq!(result.current_file, Some("vendor/autoload.php".to_string()));
        assert_eq!(result.files_done, 5000);
        assert_eq!(result.files_total, 10000);
        assert_eq!(result.total_received_bytes, 500_000);
    }

    #[test]
    fn test_map_status_info_staging_empty_path() {
        let info = SessionStatusInfo {
            name: "src-aabb".to_string(),
            status: SessionStatus::Syncing,
            staging_progress: Some(StagingProgress {
                path: "".to_string(),
                received_files: 100,
                expected_files: 200,
                received_size: 0,
                expected_size: 0,
                total_received_size: 0,
            }),
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert!(result.current_file.is_none()); // empty path → None
        assert_eq!(result.percent, 50);
    }

    #[test]
    fn test_map_status_info_staging_zero_expected() {
        let info = SessionStatusInfo {
            name: "x-1234".to_string(),
            status: SessionStatus::Syncing,
            staging_progress: Some(StagingProgress {
                path: "foo.txt".to_string(),
                received_files: 0,
                expected_files: 0, // zero → use base_percent
                received_size: 0,
                expected_size: 0,
                total_received_size: 0,
            }),
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.percent, 70); // base_percent for Syncing
    }

    #[test]
    fn test_map_status_info_with_polling() {
        let info = SessionStatusInfo {
            name: "app-12345678".to_string(),
            status: SessionStatus::Watching,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: Some(10),
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.polling_interval, Some(10));
    }

    #[test]
    fn test_map_status_info_without_polling() {
        let info = SessionStatusInfo {
            name: "app-12345678".to_string(),
            status: SessionStatus::Watching,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        };
        let result = map_status_info(&info);
        assert_eq!(result.polling_interval, None);
    }

    #[test]
    fn test_map_status_info_with_sync_mode() {
        let info = SessionStatusInfo {
            name: "app-12345678".to_string(),
            status: SessionStatus::Watching,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: Some("1w-create".to_string()),
        };
        let result = map_status_info(&info);
        assert_eq!(result.sync_mode, Some("1w-create".to_string()));
    }

    // ========================================================================
    // Pipeline tests: JSON → MutagenSessionArg → DesiredSession → build_create_args
    // ========================================================================

    /// Simulates the full pipeline: JSON → MutagenSessionArg → DesiredSession → build_create_args
    fn pipeline_to_args(json: serde_json::Value) -> Vec<String> {
        use ebdev_mutagen_runner::build_create_args;

        let arg: MutagenSessionArg = serde_json::from_value(json).unwrap();
        let mut session = DesiredSession::new(
            format!("{}-{:08x}", arg.name, 0x12345678u32),
            arg.name.clone(),
            std::path::PathBuf::from("/base").join(&arg.directory),
            arg.target.clone(),
            SyncMode::TwoWaySafe,
            arg.ignore.unwrap_or_default(),
        );
        if let Some(p) = arg.polling {
            session.polling = PollingConfig {
                enabled: p.enabled,
                interval: p.interval,
            };
        }
        build_create_args(&session, false)
    }

    #[test]
    fn test_polling_pipeline_enabled() {
        let args = pipeline_to_args(serde_json::json!({
            "name": "app",
            "target": "docker://container/path",
            "directory": "src",
            "polling": { "enabled": true, "interval": 5 }
        }));
        assert!(args.contains(&"--watch-mode=force-poll".to_string()),
            "Expected --watch-mode=force-poll in args: {:?}", args);
        assert!(args.contains(&"--watch-polling-interval=5".to_string()),
            "Expected --watch-polling-interval=5 in args: {:?}", args);
    }

    #[test]
    fn test_polling_pipeline_default() {
        let args = pipeline_to_args(serde_json::json!({
            "name": "app",
            "target": "docker://container/path",
            "directory": "src"
        }));
        assert!(!args.iter().any(|a| a.starts_with("--watch-mode")),
            "Expected no --watch-mode in args: {:?}", args);
    }

    #[test]
    fn test_polling_pipeline_disabled() {
        let args = pipeline_to_args(serde_json::json!({
            "name": "app",
            "target": "docker://container/path",
            "directory": "src",
            "polling": { "enabled": false }
        }));
        assert!(!args.iter().any(|a| a.starts_with("--watch-mode")),
            "Expected no --watch-mode in args: {:?}", args);
    }
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
        op_compact_mode,
        op_clear_completed,
        op_mutagen_reconcile,
        op_mutagen_pause_all,
        op_start_stream,
        op_stream_next,
        // Local filesystem ops
        op_fs_write_file,
        op_fs_read_file,
        op_fs_append_file,
        op_fs_mkdir,
        op_fs_remove,
        op_fs_exists,
        op_fs_stat,
        // Docker filesystem ops
        op_docker_fs_write_file,
        op_docker_fs_read_file,
        op_docker_fs_append_file,
        op_docker_fs_mkdir,
        op_docker_fs_remove,
        op_docker_fs_exists,
        op_docker_fs_stat,
    ],
);
