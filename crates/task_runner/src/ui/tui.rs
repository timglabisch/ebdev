use super::TaskRunnerUI;
use super::types::{CollapsedStage, TaskInfo, TaskState, format_bytes};
use super::widgets::command_palette::{self, CommandPaletteState};
use super::widgets::{header, help, task_list, task_output};
use crate::command::{CommandId, CommandResult, RegisteredTask};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::Duration;

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Scroll state for the output panel
struct ScrollState {
    offset: u16,
    auto_scroll: bool,
}

impl ScrollState {
    fn new() -> Self {
        Self { offset: 0, auto_scroll: true }
    }

    fn scroll_by(&mut self, delta: i32) {
        if delta < 0 {
            self.offset = self.offset.saturating_sub(delta.unsigned_abs() as u16);
            self.auto_scroll = false;
        } else {
            self.offset = self.offset.saturating_add(delta as u16);
        }
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.auto_scroll = true;
    }

    fn jump_to_end(&mut self) {
        self.offset = u16::MAX;
        self.auto_scroll = true;
    }

    fn jump_to_start(&mut self) {
        self.offset = 0;
        self.auto_scroll = false;
    }
}

/// TUI UI implementation
pub struct TuiUI {
    terminal: Option<Tui>,
    task_name: String,
    tasks: Vec<TaskInfo>,
    task_map: HashMap<CommandId, usize>,
    focused_task: usize,
    /// Scroll state for the output panel (pinned mode)
    output_scroll: ScrollState,
    /// Scroll offset for the task list panel
    task_list_scroll: usize,
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
    palette: CommandPaletteState,
    /// Task that was triggered and needs to be returned via poll_triggered_task
    triggered_task: Option<String>,
    /// Auto-quit when tasks complete (disabled when user interacts with Command Palette)
    auto_quit: bool,
    /// Pinned task index: None = stacked mode (default), Some(idx) = show only that task
    pinned_task: Option<usize>,
    /// Stored geometry of the left task list panel (for mouse hit-testing)
    task_list_area: Rc<Cell<Rect>>,
    /// Stored geometry of the output panel (for mouse hit-testing)
    output_area: Rc<Cell<Rect>>,
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
            output_scroll: ScrollState::new(),
            task_list_scroll: 0,
            should_quit: false,
            rows,
            cols,
            collapsed_stages: Vec::new(),
            current_stage: None,
            registered_tasks: Vec::new(),
            palette: CommandPaletteState::new(),
            triggered_task: None,
            auto_quit: true,
            pinned_task: None,
            task_list_area: Rc::new(Cell::new(Rect::default())),
            output_area: Rc::new(Cell::new(Rect::default())),
            task_list_offset: Rc::new(Cell::new(0)),
        })
    }

    /// Focus a task by index and reset output scroll state
    fn focus_task(&mut self, idx: usize) {
        self.focused_task = idx;
        self.output_scroll.reset();
        self.ensure_focused_visible();
    }

    /// Count non-task lines at top of the task list (collapsed stages + headers)
    fn non_task_line_count(&self) -> usize {
        let mut count = self.collapsed_stages.len();
        if self.current_stage.is_some() {
            if !self.collapsed_stages.is_empty() {
                count += 1; // empty separator
            }
            count += 2; // header + empty line after
        }
        count
    }

    /// Ensure the focused task is visible in the task list by adjusting scroll
    fn ensure_focused_visible(&mut self) {
        let area = self.task_list_area.get();
        let visible_height = area.height.saturating_sub(2) as usize; // borders
        if visible_height == 0 {
            return;
        }

        let focused_line = self.non_task_line_count() + self.focused_task;

        // Scroll up if focused is above viewport
        if focused_line < self.task_list_scroll {
            self.task_list_scroll = focused_line;
        }

        // Scroll down if focused is below viewport
        if focused_line >= self.task_list_scroll + visible_height {
            self.task_list_scroll = focused_line - visible_height + 1;
        }
    }

    /// Toggle pin on a task index. If already pinned, unpin.
    fn toggle_pin(&mut self, idx: usize) {
        if self.pinned_task == Some(idx) {
            self.pinned_task = None;
        } else {
            self.pinned_task = Some(idx);
        }
        self.output_scroll.reset();
    }

    /// Check if a mouse position is over the task list panel
    fn is_over_task_list(&self, col: u16, row: u16) -> bool {
        let area = self.task_list_area.get();
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    }

    /// Check if a mouse position is over the output panel
    fn is_over_output(&self, col: u16, row: u16) -> bool {
        let area = self.output_area.get();
        col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
    }

    /// Scroll the task list by delta lines, clamped to valid range
    fn scroll_task_list(&mut self, delta: i32) {
        let total_lines = self.non_task_line_count() + self.tasks.len();
        let area = self.task_list_area.get();
        let visible_height = area.height.saturating_sub(2) as usize;
        let max_scroll = total_lines.saturating_sub(visible_height);

        if delta < 0 {
            self.task_list_scroll = self.task_list_scroll.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.task_list_scroll = (self.task_list_scroll + delta as usize).min(max_scroll);
        }
    }

    fn draw(&mut self) -> io::Result<()> {
        // Collect all data we need before borrowing terminal mutably
        let palette_open = self.palette.open;
        let filtered_tasks: Vec<RegisteredTask> = self.palette.filter_tasks(&self.registered_tasks)
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
            if self.output_scroll.auto_scroll {
                self.output_scroll.offset = max_scroll as u16;
            }

            // Clamp scroll_offset to valid range
            self.output_scroll.offset = (self.output_scroll.offset as usize).min(max_scroll) as u16;

            // Re-enable auto_scroll if we're at the bottom
            if self.output_scroll.offset as usize >= max_scroll && max_scroll > 0 {
                self.output_scroll.auto_scroll = true;
            }
        }

        let tasks = &self.tasks;
        let task_name = &self.task_name;
        let focused_task = self.focused_task;
        let output_scroll_offset = self.output_scroll.offset;
        let task_list_scroll = self.task_list_scroll;
        let collapsed_stages = &self.collapsed_stages;
        let current_stage = &self.current_stage;
        let task_list_area_rc = self.task_list_area.clone();
        let output_area_rc = self.output_area.clone();
        let task_list_offset_rc = self.task_list_offset.clone();
        let palette = &self.palette;

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
            header::draw_header(frame, chunks[0], task_name, tasks, collapsed_stages);

            // Tasks area
            if tasks.is_empty() && collapsed_stages.is_empty() {
                let waiting = Paragraph::new("Waiting for tasks...")
                    .style(Style::default().fg(Color::DarkGray))
                    .block(Block::default().borders(Borders::ALL).title(" Tasks "));
                frame.render_widget(waiting, chunks[1]);
            } else {
                // Split area: task list on left, output on right
                let task_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(40.min(chunks[1].width / 3)),
                        Constraint::Min(20),
                    ])
                    .split(chunks[1]);

                // Store areas for mouse hit-testing
                task_list_area_rc.set(task_chunks[0]);
                output_area_rc.set(task_chunks[1]);

                // Task list with collapsed stages
                let offset = task_list::draw_task_list(frame, task_chunks[0], tasks, collapsed_stages, current_stage.as_deref(), focused_task, pinned_task, task_list_scroll);
                task_list_offset_rc.set(offset);

                // Right panel: pinned mode vs stacked mode
                if let Some(pin_idx) = pinned_task {
                    if pin_idx < tasks.len() {
                        task_output::draw_task_output(frame, task_chunks[1], &tasks[pin_idx], output_scroll_offset);
                    }
                } else {
                    task_output::draw_stacked_outputs(frame, task_chunks[1], tasks);
                }
            }

            // Help line
            help::draw_help(frame, chunks[2], has_registered_tasks, auto_quit);

            // Command Palette overlay
            if palette_open {
                command_palette::draw_command_palette(frame, area, palette, &filtered_tasks);
            }
        })?;

        Ok(())
    }

    fn handle_input(&mut self) -> io::Result<bool> {
        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if self.palette.open {
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
                    self.palette.open();
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
            KeyCode::Up => {
                if self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(-1);
                }
            }
            KeyCode::Down => {
                if self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(1);
                }
            }
            KeyCode::PageUp => {
                if self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(-10);
                }
            }
            KeyCode::PageDown => {
                if self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(10);
                }
            }
            KeyCode::End => {
                if self.pinned_task.is_some() {
                    self.output_scroll.jump_to_end();
                }
            }
            KeyCode::Home => {
                if self.pinned_task.is_some() {
                    self.output_scroll.jump_to_start();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(task_idx) = self.task_index_from_click(col, row) {
                    self.focus_task(task_idx);
                    self.toggle_pin(task_idx);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_over_task_list(col, row) {
                    self.scroll_task_list(-3);
                } else if self.is_over_output(col, row) && self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_over_task_list(col, row) {
                    self.scroll_task_list(3);
                } else if self.is_over_output(col, row) && self.pinned_task.is_some() {
                    self.output_scroll.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    /// Map a mouse click position to a task index, if it falls on a task row in the task list.
    fn task_index_from_click(&self, col: u16, row: u16) -> Option<usize> {
        if !self.is_over_task_list(col, row) {
            return None;
        }
        let area = self.task_list_area.get();
        // Row within the inner area (after border), plus scroll offset
        let row_in_list = row.saturating_sub(area.y + 1) as usize + self.task_list_scroll;
        let offset = self.task_list_offset.get();
        if row_in_list < offset {
            return None;
        }
        let task_idx = row_in_list - offset;
        if task_idx < self.tasks.len() { Some(task_idx) } else { None }
    }

    fn handle_command_palette_input(&mut self, key: KeyCode) -> io::Result<bool> {
        let filtered_count = self.palette.filter_tasks(&self.registered_tasks).len();

        match key {
            KeyCode::Esc => {
                self.palette.close();
            }
            KeyCode::Enter => {
                let filtered_tasks = self.palette.filter_tasks(&self.registered_tasks);
                if let Some(task) = filtered_tasks.get(self.palette.selected) {
                    self.triggered_task = Some(task.name.clone());
                }
                self.palette.close();
            }
            KeyCode::Up => {
                if filtered_count > 0 {
                    self.palette.selected = self.palette.selected.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if filtered_count > 0 {
                    self.palette.selected = (self.palette.selected + 1)
                        .min(filtered_count.saturating_sub(1));
                }
            }
            KeyCode::Backspace => {
                self.palette.input.pop();
                self.palette.selected = 0;
            }
            KeyCode::Char(c) => {
                self.palette.input.push(c);
                self.palette.selected = 0;
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

    fn on_parallel_begin(&mut self, _count: usize) {}

    fn on_parallel_end(&mut self) {}

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
            self.task_list_scroll = 0;
        }

        // Set new stage name
        self.current_stage = Some(name.to_string());
    }

    fn on_task_registered(&mut self, name: &str, description: &str) {
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
        if let Some(task) = self.tasks.get(self.focused_task) {
            if let Ok(mut parser) = task.parser.lock() {
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
