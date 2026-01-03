use crate::command::{CommandId, CommandResult};
use std::io::{self, Write};

/// Trait für UI-Interaktionen während der Task-Ausführung.
/// Ermöglicht einheitliche Logik für Headless und TUI.
pub trait TaskRunnerUI {
    /// Wird aufgerufen wenn ein Task startet
    fn on_task_start(&mut self, id: CommandId, name: &str);

    /// Wird aufgerufen wenn ein Task Output produziert (PTY-Daten)
    fn on_task_output(&mut self, id: CommandId, output: &[u8]);

    /// Wird aufgerufen wenn ein Task abgeschlossen ist
    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult);

    /// Wird aufgerufen wenn ein Task fehlschlägt
    fn on_task_error(&mut self, id: CommandId, error: &str);

    /// Wird aufgerufen wenn eine Parallel-Gruppe beginnt
    fn on_parallel_begin(&mut self, count: usize);

    /// Wird aufgerufen wenn eine Parallel-Gruppe endet
    fn on_parallel_end(&mut self);

    /// Wird aufgerufen wenn eine neue Stage beginnt
    /// Kollabiert die vorherige Stage und zeigt den neuen Stage-Header
    fn on_stage_begin(&mut self, name: &str);

    /// Prüft ob der Benutzer abbrechen möchte
    fn check_quit(&mut self) -> io::Result<bool>;

    /// Wird in der Hauptschleife aufgerufen (TUI: draw + events, Headless: noop/sleep)
    fn tick(&mut self) -> io::Result<()>;

    /// Setzt die Terminal-Größe für PTY-Output
    fn set_terminal_size(&mut self, _rows: u16, _cols: u16) {}

    /// Wird am Ende aufgerufen für Cleanup
    fn cleanup(&mut self) -> io::Result<()> { Ok(()) }
}

// ============================================================================
// HeadlessUI - Einfache stdout-basierte Ausgabe
// ============================================================================

/// Headless UI - leitet Output direkt an stdout weiter
pub struct HeadlessUI {
    current_task: Option<(CommandId, String)>,
    current_stage: Option<String>,
    in_parallel: bool,
}

impl HeadlessUI {
    pub fn new() -> Self {
        Self {
            current_task: None,
            current_stage: None,
            in_parallel: false,
        }
    }
}

impl Default for HeadlessUI {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRunnerUI for HeadlessUI {
    fn on_task_start(&mut self, id: CommandId, name: &str) {
        self.current_task = Some((id, name.to_string()));
        if self.in_parallel {
            println!("[{}] Starting: {}", id, name);
        }
    }

    fn on_task_output(&mut self, _id: CommandId, output: &[u8]) {
        // Direkt an stdout weiterleiten
        let _ = io::stdout().write_all(output);
        let _ = io::stdout().flush();
    }

    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult) {
        if self.in_parallel {
            if result.success {
                println!("[{}] Completed (exit code {})", id, result.exit_code);
            } else {
                eprintln!("[{}] Failed (exit code {})", id, result.exit_code);
            }
        }
        if self.current_task.as_ref().map(|(i, _)| *i) == Some(id) {
            self.current_task = None;
        }
    }

    fn on_task_error(&mut self, id: CommandId, error: &str) {
        eprintln!("[{}] Error: {}", id, error);
        if self.current_task.as_ref().map(|(i, _)| *i) == Some(id) {
            self.current_task = None;
        }
    }

    fn on_parallel_begin(&mut self, count: usize) {
        self.in_parallel = true;
        println!("Running {} tasks in parallel...", count);
    }

    fn on_parallel_end(&mut self) {
        self.in_parallel = false;
    }

    fn on_stage_begin(&mut self, name: &str) {
        // Print stage divider
        if self.current_stage.is_some() {
            println!();
        }
        println!();
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!("  {}", name);
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!();
        self.current_stage = Some(name.to_string());
    }

    fn check_quit(&mut self) -> io::Result<bool> {
        // Headless mode: kein interaktives Abbrechen
        Ok(false)
    }

    fn tick(&mut self) -> io::Result<()> {
        // Headless: nichts zu tun
        Ok(())
    }
}

// ============================================================================
// TuiUI - Ratatui-basierte TUI
// ============================================================================

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

// Display constants
const MAX_STAGE_NAME_LEN: usize = 20;
const MAX_TASK_NAME_LEN: usize = 25;

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

/// Truncate a string to max_len, adding "..." if truncated
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

/// Task info for TUI
pub struct TaskInfo {
    pub id: CommandId,
    pub name: String,
    pub state: TaskState,
    pub parser: Arc<Mutex<vt100::Parser>>,
    pub started_at: Instant,
}

impl TaskInfo {
    pub fn new(id: CommandId, name: String, rows: u16, cols: u16) -> Self {
        Self {
            id,
            name,
            state: TaskState::Running,
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 500))),
            started_at: Instant::now(),
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

    pub fn screen_content(&self) -> Vec<String> {
        let parser = match self.parser.lock() {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let screen = parser.screen();
        let (rows, cols) = screen.size();

        let mut lines = Vec::new();
        for row in 0..rows {
            let mut line = String::new();
            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    line.push_str(&cell.contents());
                }
            }
            lines.push(line.trim_end().to_string());
        }

        // Remove trailing empty lines
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }

        lines
    }
}

/// TUI UI implementation
pub struct TuiUI {
    terminal: Option<Tui>,
    task_name: String,
    tasks: Vec<TaskInfo>,
    task_map: HashMap<CommandId, usize>,
    focused_task: usize,
    scroll_offset: u16,
    should_quit: bool,
    rows: u16,
    cols: u16,
    /// Collapsed stages from previous stage transitions
    collapsed_stages: Vec<CollapsedStage>,
    /// Current stage name (None = default stage)
    current_stage: Option<String>,
}

impl TuiUI {
    pub fn new(task_name: String) -> io::Result<Self> {
        io::stdout().execute(EnterAlternateScreen)?;
        enable_raw_mode()?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

        let size = terminal.size()?;
        let rows = size.height.saturating_sub(6);
        let cols = size.width.saturating_sub(45);

        Ok(Self {
            terminal: Some(terminal),
            task_name,
            tasks: Vec::new(),
            task_map: HashMap::new(),
            focused_task: 0,
            scroll_offset: 0,
            should_quit: false,
            rows,
            cols,
            collapsed_stages: Vec::new(),
            current_stage: None,
        })
    }

    fn draw(&mut self) -> io::Result<()> {
        let terminal = self.terminal.as_mut().unwrap();
        let tasks = &self.tasks;
        let task_name = &self.task_name;
        let focused_task = self.focused_task;
        let scroll_offset = self.scroll_offset;
        let collapsed_stages = &self.collapsed_stages;
        let current_stage = &self.current_stage;

        terminal.draw(|frame| {
            let area = frame.area();

            // Main layout: header + tasks + help
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Min(5),    // Tasks
                    Constraint::Length(1), // Help
                ])
                .split(area);

            // Header
            Self::draw_header(frame, chunks[0], task_name, tasks, collapsed_stages);

            // Tasks area
            if tasks.is_empty() && collapsed_stages.is_empty() {
                let waiting = Paragraph::new("Waiting for tasks...")
                    .style(Style::default().fg(Color::DarkGray))
                    .block(Block::default().borders(Borders::ALL).title(" Tasks "));
                frame.render_widget(waiting, chunks[1]);
            } else {
                Self::draw_tasks(frame, chunks[1], tasks, collapsed_stages, current_stage.as_deref(), focused_task, scroll_offset);
            }

            // Help line
            Self::draw_help(frame, chunks[2]);
        })?;

        Ok(())
    }

    fn draw_header(frame: &mut Frame, area: Rect, task_name: &str, tasks: &[TaskInfo], collapsed_stages: &[CollapsedStage]) {
        // Count from current tasks
        let completed = tasks.iter().filter(|t| t.state.is_success()).count();
        let failed = tasks.iter().filter(|t| t.state.is_failed()).count();
        let running = tasks.iter().filter(|t| matches!(t.state, TaskState::Running)).count();

        // Add counts from collapsed stages
        let collapsed_completed: usize = collapsed_stages.iter().map(|s| s.task_count - s.failed_count).sum();
        let collapsed_failed: usize = collapsed_stages.iter().map(|s| s.failed_count).sum();

        let total_completed = completed + collapsed_completed;
        let total_failed = failed + collapsed_failed;
        let total = tasks.len() + collapsed_stages.iter().map(|s| s.task_count).sum::<usize>();

        let status = if total_failed > 0 {
            format!(" Task: {} | {}/{} completed, {} failed, {} running ", task_name, total_completed, total, total_failed, running)
        } else {
            format!(" Task: {} | {}/{} completed, {} running ", task_name, total_completed, total, running)
        };

        let all_done = running == 0;
        let any_failed = total_failed > 0;
        let header_style = if any_failed {
            Style::default().fg(Color::Red)
        } else if all_done && !tasks.is_empty() {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Cyan)
        };

        let header = Paragraph::new(status)
            .style(header_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, area);
    }

    fn draw_tasks(frame: &mut Frame, area: Rect, tasks: &[TaskInfo], collapsed_stages: &[CollapsedStage], current_stage: Option<&str>, focused_task: usize, scroll_offset: u16) {
        // Split area: task list on left, output on right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(40.min(area.width / 3)),
                Constraint::Min(20),
            ])
            .split(area);

        // Task list with collapsed stages
        Self::draw_task_list(frame, chunks[0], tasks, collapsed_stages, current_stage, focused_task);

        // Output of focused task
        if focused_task < tasks.len() {
            Self::draw_task_output(frame, chunks[1], &tasks[focused_task], scroll_offset);
        }
    }

    fn draw_task_list(frame: &mut Frame, area: Rect, tasks: &[TaskInfo], collapsed_stages: &[CollapsedStage], current_stage: Option<&str>, focused_task: usize) {
        let mut lines: Vec<Line> = Vec::new();

        // Draw collapsed stages first
        for stage in collapsed_stages {
            let (icon, style) = if stage.success {
                ("✓", Style::default().fg(Color::Green))
            } else {
                ("✗", Style::default().fg(Color::Red))
            };

            let stage_name = truncate_string(&stage.name, MAX_STAGE_NAME_LEN);
            let task_info = if stage.failed_count > 0 {
                format!(" ({} tasks, {} failed, {:.1}s)", stage.task_count, stage.failed_count, stage.total_duration.as_secs_f32())
            } else {
                format!(" ({} tasks, {:.1}s)", stage.task_count, stage.total_duration.as_secs_f32())
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), style),
                Span::styled(stage_name, style.add_modifier(Modifier::BOLD)),
                Span::styled(task_info, Style::default().fg(Color::DarkGray)),
            ]));
        }

        // Draw current stage header if present
        if let Some(stage_name) = current_stage {
            if !collapsed_stages.is_empty() {
                lines.push(Line::from(""));  // Empty line separator
            }
            let header = format!("═══ {} ═══", stage_name);
            lines.push(Line::from(Span::styled(
                header,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));  // Empty line after header
        }

        // Draw current tasks
        for (i, task) in tasks.iter().enumerate() {
            let is_focused = i == focused_task;
            let (icon, style) = task.icon_and_style();
            let duration_str = format!(" ({:.1}s)", task.duration().as_secs_f32());
            let prefix = if is_focused { "> " } else { "  " };
            let name = truncate_string(&task.name, MAX_TASK_NAME_LEN);

            let line_style = if is_focused {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            };

            lines.push(Line::from(vec![
                Span::styled(prefix, line_style),
                Span::styled(format!("{} ", icon), style),
                Span::styled(name, line_style),
                Span::styled(duration_str, Style::default().fg(Color::DarkGray)),
            ]));
        }

        let task_list = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Tasks "));
        frame.render_widget(task_list, area);
    }

    fn draw_task_output(frame: &mut Frame, area: Rect, task: &TaskInfo, scroll_offset: u16) {
        let content = task.screen_content();
        let content_height = content.len();
        let visible_height = area.height.saturating_sub(2) as usize;

        let max_scroll = content_height.saturating_sub(visible_height);
        let scroll = (scroll_offset as usize).min(max_scroll);

        let visible_content: Vec<Line> = content
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .map(Line::from)
            .collect();

        let title = format!(" Output: {} ", task.name);
        let border_style = match &task.state {
            TaskState::Running => Style::default().fg(Color::Yellow),
            TaskState::Completed { exit_code, .. } if *exit_code == 0 => Style::default().fg(Color::Green),
            TaskState::Completed { .. } | TaskState::Failed { .. } => Style::default().fg(Color::Red),
        };

        let output = Paragraph::new(visible_content).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        );
        frame.render_widget(output, area);

        // Scrollbar
        if content_height > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll);
            frame.render_stateful_widget(
                scrollbar,
                area.inner(Margin::new(0, 1)),
                &mut scrollbar_state,
            );
        }
    }

    fn draw_help(frame: &mut Frame, area: Rect) {
        let help = Paragraph::new(" Up/Down/Tab: select task | PageUp/Down: scroll | q/Esc: quit ")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(help, area);
    }

    fn handle_input(&mut self) -> io::Result<bool> {
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            self.should_quit = true;
                            return Ok(true);
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            if !self.tasks.is_empty() {
                                self.focused_task = (self.focused_task + 1) % self.tasks.len();
                                self.scroll_offset = 0;
                            }
                        }
                        KeyCode::BackTab | KeyCode::Up => {
                            if !self.tasks.is_empty() {
                                self.focused_task = (self.focused_task + self.tasks.len() - 1) % self.tasks.len();
                                self.scroll_offset = 0;
                            }
                        }
                        KeyCode::PageUp => {
                            self.scroll_offset = self.scroll_offset.saturating_add(5);
                        }
                        KeyCode::PageDown => {
                            self.scroll_offset = self.scroll_offset.saturating_sub(5);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(false)
    }
}

impl TaskRunnerUI for TuiUI {
    fn on_task_start(&mut self, id: CommandId, name: &str) {
        let task = TaskInfo::new(id, name.to_string(), self.rows, self.cols);
        let idx = self.tasks.len();
        self.tasks.push(task);
        self.task_map.insert(id, idx);

        // Auto-focus auf neuen Task
        self.focused_task = idx;
    }

    fn on_task_output(&mut self, id: CommandId, output: &[u8]) {
        if let Some(&idx) = self.task_map.get(&id) {
            if let Some(task) = self.tasks.get(idx) {
                if let Ok(mut parser) = task.parser.lock() {
                    parser.process(output);
                }
            }
        }
    }

    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult) {
        if let Some(&idx) = self.task_map.get(&id) {
            if let Some(task) = self.tasks.get_mut(idx) {
                let duration = task.started_at.elapsed();
                task.state = TaskState::Completed {
                    exit_code: result.exit_code,
                    duration,
                };
            }
        }
    }

    fn on_task_error(&mut self, id: CommandId, error: &str) {
        if let Some(&idx) = self.task_map.get(&id) {
            if let Some(task) = self.tasks.get_mut(idx) {
                let duration = task.started_at.elapsed();
                task.state = TaskState::Failed {
                    error: error.to_string(),
                    duration,
                };
            }
        }
    }

    fn on_parallel_begin(&mut self, _count: usize) {
        // TUI zeigt aktuell keinen speziellen Parallel-Status an
    }

    fn on_parallel_end(&mut self) {
        // TUI zeigt aktuell keinen speziellen Parallel-Status an
    }

    fn on_stage_begin(&mut self, name: &str) {
        // Collapse current tasks into a stage summary if there are any
        if !self.tasks.is_empty() {
            let stage_name = self.current_stage.clone().unwrap_or_else(|| "Default".to_string());
            self.collapsed_stages.push(CollapsedStage::from_tasks(stage_name, &self.tasks));

            // Clear current tasks
            self.tasks.clear();
            self.task_map.clear();
            self.focused_task = 0;
        }

        // Set new stage name
        self.current_stage = Some(name.to_string());
    }

    fn check_quit(&mut self) -> io::Result<bool> {
        Ok(self.should_quit)
    }

    fn tick(&mut self) -> io::Result<()> {
        self.draw()?;
        self.handle_input()?;

        // Auto-focus auf laufenden Task
        if let Some(idx) = self.tasks.iter().position(|t| t.state == TaskState::Running) {
            self.focused_task = idx;
        }

        Ok(())
    }

    fn set_terminal_size(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if self.terminal.is_some() {
            io::stdout().execute(LeaveAlternateScreen)?;
            disable_raw_mode()?;
            self.terminal = None;
        }
        Ok(())
    }
}

impl Drop for TuiUI {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
