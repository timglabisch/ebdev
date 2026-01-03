pub mod command;
pub mod executor;
pub mod ui;

use command::{CommandId, CommandRequest, ExecutorMessage};
use executor::Executor;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

// Re-exports
pub use command::Command;
pub use command::CommandResult;
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
}

impl TaskRunnerHandle {
    /// Execute a command and wait for the result
    pub async fn execute(&self, command: Command) -> Result<CommandResult, TaskRunnerError> {
        let (result_tx, result_rx) = oneshot::channel();
        let request = CommandRequest {
            id: next_command_id(),
            command,
            result_tx,
        };

        self.tx
            .send(ExecutorMessage::Execute(request))
            .map_err(|_| TaskRunnerError::ChannelClosed)?;

        result_rx.await.map_err(|_| TaskRunnerError::ChannelClosed)
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

    /// Shutdown the executor
    pub fn shutdown(&self) -> Result<(), TaskRunnerError> {
        self.tx
            .send(ExecutorMessage::Shutdown)
            .map_err(|_| TaskRunnerError::ChannelClosed)
    }
}

/// Create a new task runner (internal)
/// Returns a handle for sending commands and a receiver for the executor
fn create_task_runner() -> (TaskRunnerHandle, mpsc::UnboundedReceiver<ExecutorMessage>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (TaskRunnerHandle { tx }, rx)
}

/// Run the task runner with TUI visualization
/// This spawns the executor in a separate thread and returns the handle
pub fn run_with_tui(
    task_name: String,
    default_cwd: Option<String>,
) -> Result<(TaskRunnerHandle, std::thread::JoinHandle<std::io::Result<()>>), TaskRunnerError> {
    use std::io::IsTerminal;

    // Check if stdout is a TTY
    if !std::io::stdout().is_terminal() {
        return Err(TaskRunnerError::NotATty);
    }

    let (handle, rx) = create_task_runner();

    // Use a sync channel to signal when TUI is ready
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let thread_handle = std::thread::spawn(move || {
        let mut executor = Executor::new(rx, default_cwd);
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
) -> (TaskRunnerHandle, std::thread::JoinHandle<std::io::Result<()>>) {
    let (handle, rx) = create_task_runner();

    let thread_handle = std::thread::spawn(move || {
        let mut executor = Executor::new(rx, default_cwd);
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
        let (handle, _thread) = run_headless(None);

        let result = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "hello".into()],
            cwd: None,
            env: None,
            name: None,
            timeout: None,
            ignore_error: false,
        }).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.success);

        let _ = handle.shutdown();
    }

    #[tokio::test]
    async fn test_headless_parallel() {
        let (handle, _thread) = run_headless(None);

        handle.parallel_begin(2).unwrap();

        let r1 = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "task1".into()],
            cwd: None,
            env: None,
            name: Some("Task 1".into()),
            timeout: None,
            ignore_error: false,
        });

        let r2 = handle.execute(Command::Exec {
            cmd: vec!["echo".into(), "task2".into()],
            cwd: None,
            env: None,
            name: Some("Task 2".into()),
            timeout: None,
            ignore_error: false,
        });

        let (result1, result2) = tokio::join!(r1, r2);
        assert!(result1.is_ok());
        assert!(result2.is_ok());

        handle.parallel_end().unwrap();
        let _ = handle.shutdown();
    }
}
