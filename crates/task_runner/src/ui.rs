use crate::command::{CommandId, CommandResult, RegisteredTask};
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

    /// Wird aufgerufen wenn ein Task registriert wird (für Command Palette)
    fn on_task_registered(&mut self, _name: &str, _description: &str) {}

    /// Wird aufgerufen wenn ein Task deregistriert wird
    fn on_task_unregistered(&mut self, _name: &str) {}

    /// Gibt zurück ob ein Task getriggert werden soll (von TUI Command Palette)
    /// Gibt den Namen des zu triggernden Tasks zurück, falls vorhanden
    fn poll_triggered_task(&mut self) -> Option<String> { None }

    /// Log a message (works correctly in both headless and TUI mode)
    fn on_log(&mut self, message: &str);

    /// Returns whether the UI should auto-quit when tasks complete
    /// Default is true (auto-quit enabled)
    fn should_auto_quit(&self) -> bool { true }

    /// Prüft ob der Benutzer abbrechen möchte
    fn check_quit(&mut self) -> io::Result<bool>;

    /// Wird in der Hauptschleife aufgerufen (TUI: draw + events, Headless: noop/sleep)
    fn tick(&mut self) -> io::Result<()>;

    /// Setzt die Terminal-Größe für PTY-Output
    fn set_terminal_size(&mut self, _rows: u16, _cols: u16) {}

    /// Suspend the UI (for interactive commands that need real terminal access)
    fn suspend(&mut self) -> io::Result<()> { Ok(()) }

    /// Resume the UI after an interactive command finishes
    fn resume(&mut self) -> io::Result<()> { Ok(()) }

    /// Wird am Ende aufgerufen für Cleanup
    fn cleanup(&mut self) -> io::Result<()> { Ok(()) }
}

// ============================================================================
// HeadlessUI - Einfache stdout-basierte Ausgabe
// ============================================================================

/// Headless UI - leitet Output direkt an stdout weiter
pub struct HeadlessUI {
    current_task: Option<(CommandId, String)>,
    task_starts: std::collections::HashMap<CommandId, (String, std::time::Instant)>,
    current_stage: Option<String>,
    in_parallel: bool,
}

impl HeadlessUI {
    pub fn new() -> Self {
        Self {
            current_task: None,
            task_starts: std::collections::HashMap::new(),
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
        self.task_starts.insert(id, (name.to_string(), std::time::Instant::now()));
        if self.in_parallel {
            println!("\x1b[1;34m▶ [{}] {}\x1b[0m", id, name);
        } else {
            println!("\x1b[1;34m▶ {}\x1b[0m", name);
        }
    }

    fn on_task_output(&mut self, _id: CommandId, output: &[u8]) {
        let _ = io::stdout().write_all(output);
        let _ = io::stdout().flush();
    }

    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult) {
        let (name, duration) = self.task_starts.remove(&id)
            .map(|(n, t)| (n, t.elapsed().as_secs_f64()))
            .unwrap_or_else(|| ("?".to_string(), 0.0));
        if result.success {
            if self.in_parallel {
                println!("\x1b[1;32m✓ [{}] {} ({:.1}s)\x1b[0m", id, name, duration);
            } else {
                println!("\x1b[1;32m✓ {} ({:.1}s)\x1b[0m", name, duration);
            }
        } else if self.in_parallel {
            eprintln!("\x1b[1;31m✗ [{}] {} failed (exit code {}, {:.1}s)\x1b[0m", id, name, result.exit_code, duration);
        } else {
            eprintln!("\x1b[1;31m✗ {} failed (exit code {}, {:.1}s)\x1b[0m", name, result.exit_code, duration);
        }
        if self.current_task.as_ref().map(|(i, _)| *i) == Some(id) {
            self.current_task = None;
        }
    }

    fn on_task_error(&mut self, id: CommandId, error: &str) {
        let name = self.task_starts.remove(&id)
            .map(|(n, _)| n)
            .unwrap_or_else(|| "?".to_string());
        if self.in_parallel {
            eprintln!("\x1b[1;31m✗ [{}] {} error: {}\x1b[0m", id, name, error);
        } else {
            eprintln!("\x1b[1;31m✗ {} error: {}\x1b[0m", name, error);
        }
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

    fn on_log(&mut self, message: &str) {
        println!("{}", message);
    }

    fn check_quit(&mut self) -> io::Result<bool> {
        // Headless mode: kein interaktives Abbrechen
        Ok(false)
    }

    fn tick(&mut self) -> io::Result<()> {
        // Yield CPU to avoid busy-loop starving execution threads
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(())
    }
}

// ============================================================================
// TuiUI - Ratatui-basierte TUI
// ============================================================================

use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ansi_to_tui::IntoText;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

// Display constants
const MAX_STAGE_NAME_LEN: usize = 20;
const MAX_TASK_NAME_LEN: usize = 25;

// Command Palette constants
const PALETTE_MAX_WIDTH: u16 = 60;
const PALETTE_MAX_HEIGHT: u16 = 15;
const PALETTE_MARGIN: u16 = 4;
const PALETTE_NAME_LEN: usize = 20;
const PALETTE_DESC_LEN: usize = 30;

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

/// Format byte count for human display
fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
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
    /// Registered tasks for Command Palette
    registered_tasks: Vec<RegisteredTask>,
    /// Command Palette state
    command_palette_open: bool,
    command_palette_input: String,
    command_palette_selected: usize,
    /// Task that was triggered and needs to be returned via poll_triggered_task
    triggered_task: Option<String>,
    /// Auto-quit when tasks complete (disabled when user interacts with Command Palette)
    auto_quit: bool,
    /// Auto-scroll to bottom when new output arrives
    auto_scroll: bool,
    /// Pinned task index: None = stacked mode (default), Some(idx) = show only that task
    pinned_task: Option<usize>,
    /// Stored geometry of the left task list panel (for mouse hit-testing)
    task_list_area: Rc<Cell<Rect>>,
    /// Number of non-task lines at top of task list (collapsed stages + stage header) for click mapping
    task_list_offset: Rc<Cell<usize>>,
}

impl TuiUI {
    pub fn new(task_name: String) -> io::Result<Self> {
        io::stdout().execute(EnterAlternateScreen)?;
        io::stdout().execute(EnableMouseCapture)?;
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
            registered_tasks: Vec::new(),
            command_palette_open: false,
            command_palette_input: String::new(),
            command_palette_selected: 0,
            triggered_task: None,
            auto_quit: true,
            auto_scroll: true,
            pinned_task: None,
            task_list_area: Rc::new(Cell::new(Rect::default())),
            task_list_offset: Rc::new(Cell::new(0)),
        })
    }

    /// Get filtered tasks matching the current input
    fn get_filtered_tasks(&self) -> Vec<&RegisteredTask> {
        let input_lower = self.command_palette_input.to_lowercase();
        self.registered_tasks
            .iter()
            .filter(|t| {
                if input_lower.is_empty() {
                    true
                } else {
                    t.name.to_lowercase().contains(&input_lower)
                        || t.description.to_lowercase().contains(&input_lower)
                }
            })
            .collect()
    }

    fn draw(&mut self) -> io::Result<()> {
        // Collect all data we need before borrowing terminal mutably
        let command_palette_open = self.command_palette_open;
        let command_palette_input = self.command_palette_input.clone();
        let command_palette_selected = self.command_palette_selected;
        let filtered_tasks: Vec<RegisteredTask> = self.get_filtered_tasks()
            .into_iter()
            .cloned()
            .collect();
        let has_registered_tasks = !self.registered_tasks.is_empty();
        let auto_quit = self.auto_quit;
        let pinned_task = self.pinned_task;

        // Only compute auto-scroll for pinned mode (stacked mode always auto-scrolls)
        if pinned_task.is_some() {
            let terminal = self.terminal.as_mut().unwrap();
            let visible_height = terminal.size()?.height.saturating_sub(10) as usize;

            // Get content height for focused task
            let content_height = if self.focused_task < self.tasks.len() {
                self.tasks[self.focused_task].screen_text().lines.len()
            } else {
                0
            };
            let max_scroll = content_height.saturating_sub(visible_height);

            // Apply auto-scroll: jump to bottom
            if self.auto_scroll {
                self.scroll_offset = max_scroll as u16;
            }

            // Clamp scroll_offset to valid range
            self.scroll_offset = (self.scroll_offset as usize).min(max_scroll) as u16;

            // Re-enable auto_scroll if we're at the bottom
            if self.scroll_offset as usize >= max_scroll && max_scroll > 0 {
                self.auto_scroll = true;
            }
        }

        let tasks = &self.tasks;
        let task_name = &self.task_name;
        let focused_task = self.focused_task;
        let scroll_offset = self.scroll_offset;
        let collapsed_stages = &self.collapsed_stages;
        let current_stage = &self.current_stage;
        let task_list_area_rc = self.task_list_area.clone();
        let task_list_offset_rc = self.task_list_offset.clone();

        let terminal = self.terminal.as_mut().unwrap();
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
                Self::draw_tasks(frame, chunks[1], tasks, collapsed_stages, current_stage.as_deref(), focused_task, scroll_offset, pinned_task, &task_list_area_rc, &task_list_offset_rc);
            }

            // Help line
            Self::draw_help(frame, chunks[2], has_registered_tasks, auto_quit);

            // Command Palette overlay
            if command_palette_open {
                Self::draw_command_palette(frame, area, &command_palette_input, &filtered_tasks, command_palette_selected);
            }
        })?;

        Ok(())
    }

    fn draw_command_palette(frame: &mut Frame, area: Rect, input: &str, filtered_tasks: &[RegisteredTask], selected: usize) {
        use ratatui::widgets::Clear;

        // Center the palette
        let width = PALETTE_MAX_WIDTH.min(area.width.saturating_sub(PALETTE_MARGIN));
        let height = PALETTE_MAX_HEIGHT.min(area.height.saturating_sub(PALETTE_MARGIN));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let palette_area = Rect::new(x, y, width, height);

        // Clear background
        frame.render_widget(Clear, palette_area);

        // Layout: input + list
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Input
                Constraint::Min(3),    // Task list
            ])
            .split(palette_area);

        // Input field
        let input_text = if input.is_empty() {
            Span::styled("Type to search...", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw(input)
        };
        let input_widget = Paragraph::new(input_text)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Run Task (/) "));
        frame.render_widget(input_widget, chunks[0]);

        // Task list
        let lines: Vec<Line> = if filtered_tasks.is_empty() {
            vec![Line::from(Span::styled(
                "  No tasks registered",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            filtered_tasks.iter().enumerate().map(|(i, task)| {
                let is_selected = i == selected;
                let prefix = if is_selected { "→ " } else { "  " };
                let style = if is_selected {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let name = truncate_string(&task.name, PALETTE_NAME_LEN);
                let desc = truncate_string(&task.description, PALETTE_DESC_LEN);

                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(name, style),
                    Span::raw("  "),
                    Span::styled(desc, Style::default().fg(Color::DarkGray)),
                ])
            }).collect()
        };

        let task_list = Paragraph::new(lines)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)));
        frame.render_widget(task_list, chunks[1]);
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

    fn draw_tasks(
        frame: &mut Frame,
        area: Rect,
        tasks: &[TaskInfo],
        collapsed_stages: &[CollapsedStage],
        current_stage: Option<&str>,
        focused_task: usize,
        scroll_offset: u16,
        pinned_task: Option<usize>,
        task_list_area_rc: &Rc<Cell<Rect>>,
        task_list_offset_rc: &Rc<Cell<usize>>,
    ) {
        // Split area: task list on left, output on right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(40.min(area.width / 3)),
                Constraint::Min(20),
            ])
            .split(area);

        // Store task list area for mouse hit-testing
        task_list_area_rc.set(chunks[0]);

        // Task list with collapsed stages — returns non-task line count
        let offset = Self::draw_task_list(frame, chunks[0], tasks, collapsed_stages, current_stage, focused_task, pinned_task);
        task_list_offset_rc.set(offset);

        // Right panel: pinned mode vs stacked mode
        if let Some(pin_idx) = pinned_task {
            // Pinned mode: show single task with scroll control
            if pin_idx < tasks.len() {
                Self::draw_task_output(frame, chunks[1], &tasks[pin_idx], scroll_offset);
            }
        } else {
            // Stacked mode: show up to 3 running tasks
            Self::draw_stacked_outputs(frame, chunks[1], tasks);
        }
    }

    fn draw_stacked_outputs(frame: &mut Frame, area: Rect, tasks: &[TaskInfo]) {
        // Collect running tasks (up to 3)
        let mut display_tasks: Vec<&TaskInfo> = tasks
            .iter()
            .filter(|t| matches!(t.state, TaskState::Running))
            .take(3)
            .collect();

        // If no running tasks but tasks exist, show the last task
        if display_tasks.is_empty() {
            if let Some(last) = tasks.last() {
                display_tasks.push(last);
            }
        }

        if display_tasks.is_empty() {
            return;
        }

        let count = display_tasks.len() as u32;
        let constraints: Vec<Constraint> = (0..count)
            .map(|_| Constraint::Ratio(1, count))
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        for (i, task) in display_tasks.iter().enumerate() {
            // Auto-scroll to bottom in stacked mode
            Self::draw_task_output(frame, chunks[i], task, u16::MAX);
        }
    }

    /// Draw task list. Returns the number of non-task lines at the top (for mouse click mapping).
    fn draw_task_list(frame: &mut Frame, area: Rect, tasks: &[TaskInfo], collapsed_stages: &[CollapsedStage], current_stage: Option<&str>, focused_task: usize, pinned_task: Option<usize>) -> usize {
        let mut lines: Vec<Line> = Vec::new();
        let mut non_task_lines: usize = 0;

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
            non_task_lines += 1;
        }

        // Draw current stage header if present
        if let Some(stage_name) = current_stage {
            if !collapsed_stages.is_empty() {
                lines.push(Line::from(""));  // Empty line separator
                non_task_lines += 1;
            }
            let header = format!("═══ {} ═══", stage_name);
            lines.push(Line::from(Span::styled(
                header,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));  // Empty line after header
            non_task_lines += 2;
        }

        // Draw current tasks
        for (i, task) in tasks.iter().enumerate() {
            let is_focused = i == focused_task;
            let is_pinned = pinned_task == Some(i);
            let (icon, style) = task.icon_and_style();
            let duration_str = format!(" ({:.1}s)", task.duration().as_secs_f32());
            let prefix = if is_pinned {
                "▸ "
            } else if is_focused {
                "> "
            } else {
                "  "
            };
            let name = truncate_string(&task.name, MAX_TASK_NAME_LEN);

            let line_style = if is_pinned || is_focused {
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

        non_task_lines
    }

    fn draw_task_output(frame: &mut Frame, area: Rect, task: &TaskInfo, scroll_offset: u16) {
        let text = task.screen_text();

        let border_style = match &task.state {
            TaskState::Running => Style::default().fg(Color::Yellow),
            TaskState::Completed { exit_code, .. } if *exit_code == 0 => Style::default().fg(Color::Green),
            TaskState::Completed { .. } | TaskState::Failed { .. } => Style::default().fg(Color::Red),
        };

        // If the task name fits in the border title, show it there.
        // Otherwise show a truncated title and prepend the full name as content lines.
        let inner_width = area.width.saturating_sub(2) as usize; // borders
        let title_text = format!(" {} ", task.name);
        let title_fits = title_text.len() <= inner_width;

        let mut all_lines: Vec<Line> = Vec::new();
        let title = if title_fits {
            title_text
        } else {
            // Wrap full command name into content lines at the top
            let name = &task.name;
            let mut pos = 0;
            while pos < name.len() {
                let end = (pos + inner_width).min(name.len());
                all_lines.push(Line::from(Span::styled(
                    &name[pos..end],
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                pos = end;
            }
            all_lines.push(Line::from(""));
            " … ".to_string()
        };

        all_lines.extend(text.lines);

        let content_height = all_lines.len();
        let visible_height = area.height.saturating_sub(2) as usize;

        let max_scroll = content_height.saturating_sub(visible_height);
        let scroll = (scroll_offset as usize).min(max_scroll);

        let visible_lines: Vec<Line> = all_lines
            .into_iter()
            .skip(scroll)
            .take(visible_height)
            .collect();

        let output = Paragraph::new(visible_lines).block(
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

    fn draw_help(frame: &mut Frame, area: Rect, has_registered_tasks: bool, auto_quit: bool) {
        // Build help line with auto-exit indicator
        let mut spans = Vec::new();

        // Auto-exit indicator (red background when active)
        if auto_quit {
            spans.push(Span::styled(
                " AUTO-EXIT ",
                Style::default().fg(Color::White).bg(Color::Red),
            ));
            spans.push(Span::raw(" "));
        }

        // Help text
        let help_text = if has_registered_tasks {
            "j/k: select | Enter: pin | ↑↓: scroll | /: run task | q: quit"
        } else {
            "j/k: select | Enter: pin | ↑↓: scroll | q: quit"
        };
        spans.push(Span::styled(help_text, Style::default().fg(Color::DarkGray)));

        let help = Paragraph::new(Line::from(spans));
        frame.render_widget(help, area);
    }

    /// Focus a task by index and reset scroll state
    fn focus_task(&mut self, idx: usize) {
        self.focused_task = idx;
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    /// Toggle pin on a task index. If already pinned, unpin.
    fn toggle_pin(&mut self, idx: usize) {
        if self.pinned_task == Some(idx) {
            self.pinned_task = None;
        } else {
            self.pinned_task = Some(idx);
        }
        self.scroll_offset = 0;
        self.auto_scroll = true;
    }

    /// Scroll output by delta lines (negative = up, positive = down)
    fn scroll_by(&mut self, delta: i32) {
        if self.pinned_task.is_none() {
            return;
        }
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(delta.unsigned_abs() as u16);
            self.auto_scroll = false;
        } else {
            self.scroll_offset = self.scroll_offset.saturating_add(delta as u16);
        }
    }

    fn handle_input(&mut self) -> io::Result<bool> {
        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if self.command_palette_open {
                        return self.handle_command_palette_input(key.code);
                    }
                    self.handle_key(key.code)?;
                }
                Event::Mouse(mouse) => {
                    self.handle_mouse(mouse);
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn handle_key(&mut self, code: KeyCode) -> io::Result<()> {
        let task_count = self.tasks.len();

        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('/') => {
                if !self.registered_tasks.is_empty() {
                    self.command_palette_open = true;
                    self.command_palette_input.clear();
                    self.command_palette_selected = 0;
                    self.auto_quit = false;
                }
            }
            // j / Tab: next task
            KeyCode::Char('j') | KeyCode::Tab => {
                if task_count > 0 {
                    self.focus_task((self.focused_task + 1) % task_count);
                }
            }
            // k / Shift+Tab: previous task
            KeyCode::Char('k') | KeyCode::BackTab => {
                if task_count > 0 {
                    self.focus_task((self.focused_task + task_count - 1) % task_count);
                }
            }
            // Enter: toggle pin on focused task
            KeyCode::Enter => {
                if task_count > 0 {
                    self.toggle_pin(self.focused_task);
                }
            }
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => self.scroll_by(-10),
            KeyCode::PageDown => self.scroll_by(10),
            KeyCode::End => {
                self.scroll_offset = u16::MAX;
                self.auto_scroll = true;
            }
            KeyCode::Home => {
                if self.pinned_task.is_some() {
                    self.scroll_offset = 0;
                    self.auto_scroll = false;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(task_idx) = self.task_index_from_click(mouse.column, mouse.row) {
                    self.focus_task(task_idx);
                    self.toggle_pin(task_idx);
                }
            }
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
            _ => {}
        }
    }

    /// Map a mouse click position to a task index, if it falls on a task row in the task list.
    fn task_index_from_click(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.task_list_area.get();
        if col < area.x || col >= area.x + area.width || row < area.y || row >= area.y + area.height {
            return None;
        }
        // Subtract area y, border (1 row), and non-task offset lines
        let row_in_list = row.saturating_sub(area.y + 1) as usize;
        let offset = self.task_list_offset.get();
        if row_in_list < offset {
            return None;
        }
        let task_idx = row_in_list - offset;
        if task_idx < self.tasks.len() { Some(task_idx) } else { None }
    }

    fn handle_command_palette_input(&mut self, key: KeyCode) -> io::Result<bool> {
        let filtered_count = self.get_filtered_tasks().len();

        match key {
            KeyCode::Esc => {
                // Close Command Palette
                self.command_palette_open = false;
                self.command_palette_input.clear();
            }
            KeyCode::Enter => {
                // Trigger selected task
                let filtered_tasks = self.get_filtered_tasks();
                if let Some(task) = filtered_tasks.get(self.command_palette_selected) {
                    self.triggered_task = Some(task.name.clone());
                }
                // Close palette
                self.command_palette_open = false;
                self.command_palette_input.clear();
            }
            KeyCode::Up => {
                if filtered_count > 0 {
                    self.command_palette_selected = self.command_palette_selected
                        .saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if filtered_count > 0 {
                    self.command_palette_selected = (self.command_palette_selected + 1)
                        .min(filtered_count.saturating_sub(1));
                }
            }
            KeyCode::Backspace => {
                self.command_palette_input.pop();
                // Reset selection when input changes
                self.command_palette_selected = 0;
            }
            KeyCode::Char(c) => {
                self.command_palette_input.push(c);
                // Reset selection when input changes
                self.command_palette_selected = 0;
            }
            _ => {}
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

        // Auto-focus on new task only when not pinned
        if self.pinned_task.is_none() {
            self.focused_task = idx;
        }
    }

    fn on_task_output(&mut self, id: CommandId, output: &[u8]) {
        if let Some(&idx) = self.task_map.get(&id) {
            if let Some(task) = self.tasks.get(idx) {
                task.append_output(output);
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
            self.pinned_task = None;
        }

        // Set new stage name
        self.current_stage = Some(name.to_string());
    }

    fn on_task_registered(&mut self, name: &str, description: &str) {
        // Remove existing task with same name if any
        self.registered_tasks.retain(|t| t.name != name);
        self.registered_tasks.push(RegisteredTask {
            name: name.to_string(),
            description: description.to_string(),
        });
    }

    fn on_task_unregistered(&mut self, name: &str) {
        self.registered_tasks.retain(|t| t.name != name);
    }

    fn poll_triggered_task(&mut self) -> Option<String> {
        self.triggered_task.take()
    }

    fn on_log(&mut self, message: &str) {
        // Append log message to the currently focused task's output
        // If no task exists, we store it for later or ignore it
        if let Some(task) = self.tasks.get(self.focused_task) {
            if let Ok(mut parser) = task.parser.lock() {
                // Format as a log line and process through the PTY parser
                let formatted = format!("{}\r\n", message);
                parser.process(formatted.as_bytes());
            }
        }
    }

    fn should_auto_quit(&self) -> bool {
        self.auto_quit
    }

    fn check_quit(&mut self) -> io::Result<bool> {
        Ok(self.should_quit)
    }

    fn tick(&mut self) -> io::Result<()> {
        self.draw()?;
        self.handle_input()?;

        // Auto-focus on running task only when not pinned
        if self.pinned_task.is_none() {
            if let Some(idx) = self.tasks.iter().position(|t| t.state == TaskState::Running) {
                self.focused_task = idx;
            }
        }

        Ok(())
    }

    fn set_terminal_size(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    fn suspend(&mut self) -> io::Result<()> {
        if self.terminal.is_some() {
            io::stdout().execute(DisableMouseCapture)?;
            io::stdout().execute(LeaveAlternateScreen)?;
            disable_raw_mode()?;
        }
        Ok(())
    }

    fn resume(&mut self) -> io::Result<()> {
        if self.terminal.is_some() {
            enable_raw_mode()?;
            io::stdout().execute(EnterAlternateScreen)?;
            io::stdout().execute(EnableMouseCapture)?;
            self.terminal.as_mut().unwrap().clear()?;
        }
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if self.terminal.is_some() {
            io::stdout().execute(DisableMouseCapture)?;
            io::stdout().execute(LeaveAlternateScreen)?;
            disable_raw_mode()?;
            self.terminal = None;
        }

        // Print output of failed tasks so the user can see what went wrong
        for task in &self.tasks {
            if !task.state.is_failed() {
                continue;
            }

            let raw = task.raw_output.lock().unwrap_or_else(|e| e.into_inner());
            if raw.is_empty() {
                continue;
            }

            // Print header
            let duration = task.state.duration().map(|d| format!(" ({:.1}s)", d.as_secs_f64())).unwrap_or_default();
            eprintln!("\n\x1b[1;31m--- Failed: {}{} ---\x1b[0m", task.name, duration);

            // Print last portion of output (max ~8KB to avoid flooding terminal)
            let output = if raw.len() > 8192 {
                eprintln!("  (showing last 8KB of {} total)\n", format_bytes(raw.len()));
                &raw[raw.len() - 8192..]
            } else {
                &raw
            };

            // Write raw output (preserves ANSI colors)
            let _ = io::stderr().write_all(output);
            let _ = io::stderr().flush();
            eprintln!("\n\x1b[1;31m--- End: {} ---\x1b[0m", task.name);
        }

        Ok(())
    }
}

impl Drop for TuiUI {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
