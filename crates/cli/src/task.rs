use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::JoinHandle;

use ebdev_task_runner::TaskRunnerHandle;

/// Run a task with TUI visualization
pub async fn run_task_with_tui(
    config_path: &std::path::Path,
    task_name: &str,
    base_path: &PathBuf,
    debug_log: Option<std::path::PathBuf>,
    mutagen_path: Option<PathBuf>,
    task_env: std::collections::HashMap<String, String>,
    embedded_binary: &'static [u8],
    task_args: Vec<String>,
    flag_overrides: std::collections::HashMap<String, serde_json::Value>,
) -> anyhow::Result<ExitCode> {
    let (handle, thread) = match ebdev_task_runner::run_with_tui(
        task_name.to_string(),
        Some(base_path.to_string_lossy().to_string()),
        debug_log,
        embedded_binary,
    ) {
        Ok(r) => r,
        Err(ebdev_task_runner::TaskRunnerError::NotATty) => {
            eprintln!("Error: TUI requires an interactive terminal.");
            eprintln!("Run without --tui flag or use an interactive terminal.");
            return Ok(ExitCode::FAILURE);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return Ok(ExitCode::FAILURE);
        }
    };

    run_task_with_runner(config_path, task_name, handle, thread, mutagen_path, task_env, embedded_binary, task_args, flag_overrides).await
}

/// Run a task in headless mode with PTY support
pub async fn run_task_headless(
    config_path: &std::path::Path,
    task_name: &str,
    base_path: &PathBuf,
    debug_log: Option<std::path::PathBuf>,
    mutagen_path: Option<PathBuf>,
    task_env: std::collections::HashMap<String, String>,
    embedded_binary: &'static [u8],
    task_args: Vec<String>,
    flag_overrides: std::collections::HashMap<String, serde_json::Value>,
) -> anyhow::Result<ExitCode> {
    let (handle, thread) = ebdev_task_runner::run_headless(
        Some(base_path.to_string_lossy().to_string()),
        debug_log,
        embedded_binary,
    );

    run_task_with_runner(config_path, task_name, handle, thread, mutagen_path, task_env, embedded_binary, task_args, flag_overrides).await
}

/// Shared logic: run deno task, then shutdown the runner and check results.
async fn run_task_with_runner(
    config_path: &std::path::Path,
    task_name: &str,
    handle: TaskRunnerHandle,
    thread: JoinHandle<std::io::Result<()>>,
    mutagen_path: Option<PathBuf>,
    task_env: std::collections::HashMap<String, String>,
    embedded_binary: &'static [u8],
    task_args: Vec<String>,
    flag_overrides: std::collections::HashMap<String, serde_json::Value>,
) -> anyhow::Result<ExitCode> {
    let handle_for_shutdown = handle.clone();

    let deno_result = ebdev_toolchain_deno::run_task(
        config_path, task_name, Some(handle), mutagen_path, task_env, embedded_binary, task_args, flag_overrides,
    ).await;

    let _ = handle_for_shutdown.shutdown();
    let thread_result = thread.join();

    if let Err(e) = deno_result {
        eprintln!("Task failed: {}", e);
        return Ok(ExitCode::FAILURE);
    }

    if let Err(e) = thread_result {
        eprintln!("Runner thread error: {:?}", e);
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}
