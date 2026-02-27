use std::path::PathBuf;
use std::process::ExitCode;

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
) -> anyhow::Result<ExitCode> {
    // Start TUI task runner in separate thread
    let (handle, tui_thread) = match ebdev_task_runner::run_with_tui(
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

    let handle_for_shutdown = handle.clone();
    let config_path = config_path.to_path_buf();
    let task_name = task_name.to_string();

    // Run Deno in main thread
    let deno_result = ebdev_toolchain_deno::run_task(&config_path, &task_name, Some(handle), mutagen_path, task_env, embedded_binary, task_args).await;

    // Signal shutdown to TUI
    if let Err(e) = handle_for_shutdown.shutdown() {
        eprintln!("Warning: Failed to send shutdown signal: {}", e);
    }

    // Wait for TUI thread
    let tui_result = tui_thread.join();

    // Check results
    if let Err(e) = deno_result {
        eprintln!("Task failed: {}", e);
        return Ok(ExitCode::FAILURE);
    }

    if let Err(e) = tui_result {
        eprintln!("TUI thread error: {:?}", e);
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
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
) -> anyhow::Result<ExitCode> {
    // Start headless task runner in separate thread
    let (handle, runner_thread) = ebdev_task_runner::run_headless(
        Some(base_path.to_string_lossy().to_string()),
        debug_log,
        embedded_binary,
    );

    let handle_for_shutdown = handle.clone();
    let config_path = config_path.to_path_buf();
    let task_name = task_name.to_string();

    // Run Deno in main thread
    let deno_result = ebdev_toolchain_deno::run_task(&config_path, &task_name, Some(handle), mutagen_path, task_env, embedded_binary, task_args).await;

    // Signal shutdown
    let _ = handle_for_shutdown.shutdown();

    // Wait for runner thread
    let runner_result = runner_thread.join();

    // Check results
    if let Err(e) = deno_result {
        eprintln!("Task failed: {}", e);
        return Ok(ExitCode::FAILURE);
    }

    if let Err(e) = runner_result {
        eprintln!("Runner thread error: {:?}", e);
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}
