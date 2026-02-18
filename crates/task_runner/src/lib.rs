pub mod backend;
pub mod command;
pub mod executor;
pub mod ui;

use command::{CommandId, CommandRequest, ExecutorMessage};
use executor::Executor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

// Re-exports
pub use command::Command;
pub use command::CommandResult;
pub use command::OutputEvent;
pub use command::RegisteredTask;
pub use command::TuiEvent;
pub use ebdev_remote::OutputStream;
pub use ui::{HeadlessUI, TaskRunnerUI, TuiUI};

#[derive(Debug, Error)]
pub enum TaskRunnerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Task runner terminated (TUI closed or crashed)")]
    ChannelClosed,

    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("TUI requires interactive terminal (try without --tui)")]
    NotATty,
}

/// Counter for command IDs
static COMMAND_ID: AtomicU64 = AtomicU64::new(1);

fn next_command_id() -> CommandId {
    COMMAND_ID.fetch_add(1, Ordering::SeqCst)
}

/// Handle for sending commands to the executor
#[derive(Clone)]
pub struct TaskRunnerHandle {
    tx: mpsc::UnboundedSender<ExecutorMessage>,
    /// Receiver for TUI events (shared via Arc for cloning)
    tui_event_rx: std::sync::Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<TuiEvent>>>,
}

impl TaskRunnerHandle {
    /// Execute a command and wait for the result
    pub async fn execute(&self, command: Command) -> Result<CommandResult, TaskRunnerError> {
        let (result_tx, result_rx) = oneshot::channel();
        let request = CommandRequest {
            id: next_command_id(),
            command,
            result_tx,
            output_tx: None,
        };

        self.tx
            .send(ExecutorMessage::Execute(request))
            .map_err(|_| TaskRunnerError::ChannelClosed)?;

        result_rx.await.map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Execute a command with streaming output events.
    /// Returns a receiver for OutputEvent (Output chunks + final Done).
    pub fn execute_streaming(
        &self,
        command: Command,
    ) -> Result<mpsc::UnboundedReceiver<command::OutputEvent>, TaskRunnerError> {
        let (result_tx, _result_rx) = oneshot::channel(); // result comes via OutputEvent::Done
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let request = CommandRequest {
            id: next_command_id(),
            command,
            result_tx,
            output_tx: Some(output_tx),
        };

        self.tx
            .send(ExecutorMessage::Execute(request))
            .map_err(|_| TaskRunnerError::ChannelClosed)?;

        Ok(output_rx)
    }

    /// Signal the start of a parallel group
    pub fn parallel_begin(&self, count: usize) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::ParallelBegin { count })
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Signal the end of a parallel group
    pub fn parallel_end(&self) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::ParallelEnd)
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Begin a new stage (collapses previous tasks, shows stage header)
    pub fn stage_begin(&self, name: &str) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::StageBegin { name: name.to_string() })
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Register a task that can be triggered from the TUI
    pub fn task_register(&self, name: &str, description: &str) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::TaskRegister {
                name: name.to_string(),
                description: description.to_string(),
            })
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Unregister a task
    pub fn task_unregister(&self, name: &str) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::TaskUnregister { name: name.to_string() })
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Log a message (works correctly in both headless and TUI mode)
    pub fn log(&self, message: &str) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::Log { message: message.to_string() })
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }

    /// Poll for a task trigger event from the TUI (non-blocking)
    pub async fn poll_task_trigger(&self) -> Option<String> {
        let mut rx = self.tui_event_rx.lock().await;
        match rx.try_recv() {
            Ok(TuiEvent::TaskTriggered { name }) => Some(name),
            Err(_) => None,
        }
    }

    /// Shutdown the executor
    pub fn shutdown(&self) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::Shutdown)
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }
}

/// Create a new task runner (internal)
/// Returns a handle for sending commands, a receiver for the executor, and a sender for TUI events
fn create_task_runner() -> (
    TaskRunnerHandle,
    mpsc::UnboundedReceiver<ExecutorMessage>,
    mpsc::UnboundedSender<TuiEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (tui_event_tx, tui_event_rx) = mpsc::unbounded_channel();
    let handle = TaskRunnerHandle {
        tx,
        tui_event_rx: std::sync::Arc::new(tokio::sync::Mutex::new(tui_event_rx)),
    };
    (handle, rx, tui_event_tx)
}

/// Run the task runner with TUI visualization
/// This spawns the executor in a separate thread and returns the handle
pub fn run_with_tui(
    task_name: String,
    default_cwd: Option<String>,
    debug_log: Option<PathBuf>,
    embedded_linux_binary: &'static [u8],
) -> Result<(TaskRunnerHandle, std::thread::JoinHandle<std::io::Result<()>>), TaskRunnerError> {
    use std::io::IsTerminal;

    // Check if stdout is a TTY
    if !std::io::stdout().is_terminal() {
        return Err(TaskRunnerError::NotATty);
    }

    let (handle, rx, tui_event_tx) = create_task_runner();

    // Use a sync channel to signal when TUI is ready
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let thread_handle = std::thread::spawn(move || {
        let mut executor = Executor::new(rx, default_cwd, tui_event_tx, embedded_linux_binary);
        if let Some(log_path) = debug_log {
            executor = executor.with_debug_log(log_path);
        }
        match TuiUI::new(task_name) {
            Ok(mut ui) => {
                // Signal that we're ready
                let _ = ready_tx.send(Ok(()));
                executor.run(&mut ui)
            }
            Err(e) => {
                let _ = ready_tx.send(Err(format!("TUI init failed: {}", e)));
                Err(e)
            }
        }
    });

    // Wait for TUI to be ready
    match ready_rx.recv() {
        Ok(Ok(())) => Ok((handle, thread_handle)),
        Ok(Err(e)) => Err(TaskRunnerError::CommandFailed(e)),
        Err(_) => Err(TaskRunnerError::CommandFailed("TUI thread died before initialization".into())),
    }
}

/// Run the task runner in headless mode
/// This spawns the executor in a separate thread and returns the handle
pub fn run_headless(
    default_cwd: Option<String>,
    debug_log: Option<PathBuf>,
    embedded_linux_binary: &'static [u8],
) -> (TaskRunnerHandle, std::thread::JoinHandle<std::io::Result<()>>) {
    let (handle, rx, tui_event_tx) = create_task_runner();

    let thread_handle = std::thread::spawn(move || {
        let mut executor = Executor::new(rx, default_cwd, tui_event_tx, embedded_linux_binary);
        if let Some(log_path) = debug_log {
            executor = executor.with_debug_log(log_path);
        }
        let mut ui = HeadlessUI::new();
        executor.run(&mut ui)
    });

    (handle, thread_handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_headless_echo() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "hello".into()],
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_parallel_actually_concurrent() {
        let (handle, _thread) = run_headless(None, None, b"");

        handle.parallel_begin(2).unwrap();

        let start = std::time::Instant::now();

        let r1 = handle.execute(Command::Shell {
            script: "sleep 0.5".into(),
            cwd: None,
            env: None,
            name: Some("Sleep 1".into()),
            timeout: None,
            ignore_error: false,
            interactive: false,
        });

        let r2 = handle.execute(Command::Shell {
            script: "sleep 0.5".into(),
            cwd: None,
            env: None,
            name: Some("Sleep 2".into()),
            timeout: None,
            ignore_error: false,
            interactive: false,
        });

        let (result1, result2) = tokio::join!(r1, r2);
        let elapsed = start.elapsed();

        assert!(result1.is_ok());
        assert!(result2.is_ok());
        assert!(result1.unwrap().success);
        assert!(result2.unwrap().success);

        // If parallel: ~0.5s. If serialized: ~1.0s. Assert < 0.9s.
        assert!(
            elapsed.as_secs_f64() < 0.9,
            "Parallel tasks took {:.2}s, expected < 0.9s (tasks are serialized!)",
            elapsed.as_secs_f64()
        );

        handle.parallel_end().unwrap();
        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_headless_parallel() {
        let (handle, _thread) = run_headless(None, None, b"");

        handle.parallel_begin(2).unwrap();

        let r1 = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "task1".into()],
            cwd: None,
            env: None,
            name: Some("Task 1".into()),
            timeout: None,
            ignore_error: false,
            interactive: false,
        });

        let r2 = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "task2".into()],
            cwd: None,
            env: None,
            name: Some("Task 2".into()),
            timeout: None,
            ignore_error: false,
            interactive: false,
        });

        let (result1, result2) = tokio::join!(r1, r2);
        assert!(result1.is_ok());
        assert!(result2.is_ok());

        handle.parallel_end().unwrap();
        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_exec() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "interactive".into()],
            cwd: None,
            env: None,
            name: Some("Interactive echo".into()),
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_shell() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Shell {
            script: "echo hello && echo world".into(),
            cwd: None,
            env: None,
            name: Some("Interactive shell".into()),
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_failure() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["false".into()],
            cwd: None,
            env: None,
            name: Some("Interactive false".into()),
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(!result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_env_and_cwd() {
        let (handle, _thread) = run_headless(None, None, b"");

        // Test that env and cwd are passed through in interactive mode
        let mut env = std::collections::HashMap::new();
        env.insert("EBDEV_TEST_VAR".into(), "42".into());

        let result = handle.execute(Command::Shell {
            script: "test \"$EBDEV_TEST_VAR\" = \"42\"".into(),
            cwd: Some("/tmp".into()),
            env: Some(env),
            name: Some("Interactive env+cwd".into()),
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0, "env/cwd not passed through in interactive mode");
        assert!(result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_stdout_capture_exec() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "hello world".into()],
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).await.unwrap();

        assert!(result.success);
        // PTY converts \n to \r\n
        assert!(
            result.stdout.contains("hello world"),
            "stdout should contain 'hello world', got: {:?}",
            result.stdout,
        );

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_stdout_capture_shell() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Shell {
            script: "echo foo; echo bar".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).await.unwrap();

        assert!(result.success);
        assert!(
            result.stdout.contains("foo"),
            "stdout should contain 'foo', got: {:?}",
            result.stdout,
        );
        assert!(
            result.stdout.contains("bar"),
            "stdout should contain 'bar', got: {:?}",
            result.stdout,
        );

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_stderr_merged_in_pty_mode() {
        // With PTY enabled, stderr is merged into stdout
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Shell {
            script: "echo errout >&2".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: true,
            interactive: false,
        }).await.unwrap();

        // PTY merges stderr into stdout
        assert!(
            result.stdout.contains("errout"),
            "with PTY, stderr output should appear in stdout, got: {:?}",
            result.stdout,
        );

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_stdout_capture_on_failure() {
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Shell {
            script: "echo before-fail; exit 1".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: true,
            interactive: false,
        }).await.unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, 1);
        assert!(
            result.stdout.contains("before-fail"),
            "stdout should contain output even on failure, got: {:?}",
            result.stdout,
        );

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_stdout_empty() {
        // Interactive commands inherit stdio, so stdout/stderr are empty
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "interactive".into()],
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await.unwrap();

        assert!(result.success);
        assert!(result.stdout.is_empty(), "interactive commands should not capture stdout");
        assert!(result.stderr.is_empty(), "interactive commands should not capture stderr");

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_interactive_then_normal() {
        // Verify that after an interactive command, normal PTY commands still work
        let (handle, _thread) = run_headless(None, None, b"");

        let result = handle.execute(Command::Exec {
            cmd: vec!["true".into()],
            cwd: None,
            env: None,
            name: Some("Interactive first".into()),
            timeout: None,
            ignore_error: false,
            interactive: true,
        }).await;
        assert!(result.is_ok());
        assert!(result.unwrap().success);

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "back to normal".into()],
            cwd: None,
            env: None,
            name: Some("Normal after interactive".into()),
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).await;
        assert!(result.is_ok());
        assert!(result.unwrap().success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_streaming_output_events() {
        let (handle, _thread) = run_headless(None, None, b"");

        let mut output_rx = handle.execute_streaming(Command::Shell {
            script: "echo streaming-test".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).unwrap();

        let mut collected_output = String::new();
        let mut got_done = false;

        while let Some(event) = output_rx.recv().await {
            match event {
                OutputEvent::Output { stream: _, data } => {
                    collected_output.push_str(&String::from_utf8_lossy(&data));
                }
                OutputEvent::Done(result) => {
                    assert!(result.success);
                    assert_eq!(result.exit_code, 0);
                    got_done = true;
                    break;
                }
            }
        }

        assert!(got_done, "should receive Done event");
        assert!(
            collected_output.contains("streaming-test"),
            "streamed output should contain 'streaming-test', got: {:?}",
            collected_output,
        );

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_streaming_multiple_lines() {
        let (handle, _thread) = run_headless(None, None, b"");

        let mut output_rx = handle.execute_streaming(Command::Shell {
            script: "echo line1; echo line2; echo line3".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).unwrap();

        let mut chunks = Vec::new();
        while let Some(event) = output_rx.recv().await {
            match event {
                OutputEvent::Output { stream: _, data } => {
                    chunks.push(String::from_utf8_lossy(&data).into_owned());
                }
                OutputEvent::Done(result) => {
                    assert!(result.success);
                    break;
                }
            }
        }

        let full_output = chunks.join("");
        assert!(full_output.contains("line1"), "missing line1 in: {:?}", full_output);
        assert!(full_output.contains("line2"), "missing line2 in: {:?}", full_output);
        assert!(full_output.contains("line3"), "missing line3 in: {:?}", full_output);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_streaming_failure() {
        let (handle, _thread) = run_headless(None, None, b"");

        let mut output_rx = handle.execute_streaming(Command::Shell {
            script: "echo before-error; exit 42".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: true,
            interactive: false,
        }).unwrap();

        let mut output = String::new();
        let mut final_result = None;

        while let Some(event) = output_rx.recv().await {
            match event {
                OutputEvent::Output { stream: _, data } => {
                    output.push_str(&String::from_utf8_lossy(&data));
                }
                OutputEvent::Done(result) => {
                    final_result = Some(result);
                    break;
                }
            }
        }

        let result = final_result.expect("should receive Done event");
        assert!(!result.success);
        assert_eq!(result.exit_code, 42);
        assert!(output.contains("before-error"), "output before failure should be streamed");

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_streaming_and_stdout_capture_both_work() {
        // Verify that streaming doesn't break stdout capture in the result
        let (handle, _thread) = run_headless(None, None, b"");

        let mut output_rx = handle.execute_streaming(Command::Shell {
            script: "echo captured-too".into(),
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
            interactive: false,
        }).unwrap();

        let mut final_result = None;
        while let Some(event) = output_rx.recv().await {
            match event {
                OutputEvent::Output { .. } => {}
                OutputEvent::Done(result) => {
                    final_result = Some(result);
                    break;
                }
            }
        }

        let result = final_result.unwrap();
        assert!(result.success);
        assert!(
            result.stdout.contains("captured-too"),
            "stdout should still be captured in result, got: {:?}",
            result.stdout,
        );

        let _ = handle.shutdown();
    }
}
