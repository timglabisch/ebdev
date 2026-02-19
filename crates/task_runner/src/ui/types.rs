use crate::command::CommandId;
use ratatui::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use ansi_to_tui::IntoText;

// Display constants
pub const MAX_STAGE_NAME_LEN: usize = 20;
pub const MAX_TASK_NAME_LEN: usize = 25;

/// Task state for TUI visualization
#[derive(Debug, Clone, PartialEq)]
pub enum TaskState {
    Running,
    Completed { exit_code: i32, duration: Duration },
    Failed { error: String, duration: Duration },
}

impl TaskState {
    /// Returns true if the task failed (non-zero exit code or error)
    pub fn is_failed(&self) -> bool {
        match self {
            TaskState::Failed { .. } => true,
            TaskState::Completed { exit_code, .. } => *exit_code != 0,
            TaskState::Running => false,
        }
    }

    /// Returns true if the task completed successfully
    pub fn is_success(&self) -> bool {
        matches!(self, TaskState::Completed { exit_code, .. } if *exit_code == 0)
    }

    /// Returns the duration if the task has finished
    pub fn duration(&self) -> Option<Duration> {
        match self {
            TaskState::Completed { duration, .. } | TaskState::Failed { duration, .. } => Some(*duration),
            TaskState::Running => None,
        }
    }
}

/// Collapsed stage info for TUI
#[derive(Debug, Clone)]
pub struct CollapsedStage {
    pub name: String,
    pub task_count: usize,
    pub total_duration: Duration,
    pub success: bool,
    pub failed_count: usize,
}

impl CollapsedStage {
    /// Create a collapsed stage summary from a list of tasks
    pub fn from_tasks(name: String, tasks: &[TaskInfo]) -> Self {
        let task_count = tasks.len();
        let failed_count = tasks.iter().filter(|t| t.state.is_failed()).count();
        let success = failed_count == 0;
        let total_duration: Duration = tasks.iter()
            .map(|t| t.state.duration().unwrap_or_else(|| t.started_at.elapsed()))
            .sum();

        Self {
            name,
            task_count,
            total_duration,
            success,
            failed_count,
        }
    }
}

/// Task info for TUI
pub struct TaskInfo {
    pub id: CommandId,
    pub name: String,
    pub state: TaskState,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub started_at: Instant,
    /// Raw output buffer for ANSI color rendering
    pub raw_output: Arc<Mutex<Vec<u8>>>,
}

impl TaskInfo {
    pub fn new(id: CommandId, name: String, rows: u16, cols: u16) -> Self {
        Self {
            id,
            name,
            state: TaskState::Running,
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 500))),
            started_at: Instant::now(),
            raw_output: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn append_output(&self, data: &[u8]) {
        if let Ok(mut parser) = self.parser.lock() {
            parser.process(data);
        }
        if let Ok(mut raw) = self.raw_output.lock() {
            raw.extend_from_slice(data);
        }
    }

    /// Get the current duration (elapsed for running, stored for completed)
    pub fn duration(&self) -> Duration {
        self.state.duration().unwrap_or_else(|| self.started_at.elapsed())
    }

    /// Get icon and style for this task's state
    pub fn icon_and_style(&self) -> (&'static str, Style) {
        match &self.state {
            TaskState::Running => ("●", Style::default().fg(Color::Yellow)),
            TaskState::Completed { exit_code, .. } => {
                if *exit_code == 0 {
                    ("✓", Style::default().fg(Color::Green))
                } else {
                    ("✗", Style::default().fg(Color::Red))
                }
            }
            TaskState::Failed { .. } => ("✗", Style::default().fg(Color::Red)),
        }
    }

    /// Get screen content as ratatui Text with ANSI colors preserved
    pub fn screen_text(&self) -> Text<'static> {
        let raw = match self.raw_output.lock() {
            Ok(r) => r.clone(),
            Err(_) => return Text::default(),
        };

        match raw.into_text() {
            Ok(text) => text,
            Err(_) => Text::default(),
        }
    }
}

/// Format byte count for human display
pub fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Truncate a string to max_len, adding "..." if truncated
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    } else {
        s.to_string()
    }
}
