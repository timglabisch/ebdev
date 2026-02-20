use super::TaskRunnerUI;
use super::types::{CompletedStage, FocusTarget, PinTarget, TaskInfo, TaskState, format_bytes};
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
    focus: FocusTarget,
    /// Scroll state for the output panel (pinned mode)
    output_scroll: ScrollState,
    /// Scroll offset for the task list panel
    task_list_scroll: usize,
    should_quit: bool,
    rows: u16,
    cols: u16,
    /// Completed stages from previous stage transitions (tasks preserved)
    completed_stages: Vec<CompletedStage>,
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
    /// Pinned task: None = stacked mode (default), Some = show only that task's output
    pinned_task: Option<PinTarget>,
    /// Stored geometry of the left task list panel (for mouse hit-testing)
    task_list_area: Rc<Cell<Rect>>,
    /// Stored geometry of the output panel (for mouse hit-testing)
    output_area: Rc<Cell<Rect>>,
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
            focus: FocusTarget::CurrentTask(0),
            output_scroll: ScrollState::new(),
            task_list_scroll: 0,
            should_quit: false,
            rows,
            cols,
            completed_stages: Vec::new(),
            current_stage: None,
            registered_tasks: Vec::new(),
            palette: CommandPaletteState::new(),
            triggered_task: None,
            auto_quit: true,
            pinned_task: None,
            task_list_area: Rc::new(Cell::new(Rect::default())),
            output_area: Rc::new(Cell::new(Rect::default())),
        })
    }

    /// Build a visual row map: each entry is one rendered line in the task list.
    /// `Some(target)` = focusable row, `None` = non-focusable (separator, stage header).
    fn build_visual_rows(&self) -> Vec<Option<FocusTarget>> {
        let mut rows = Vec::new();
        for (si, stage) in self.completed_stages.iter().enumerate() {
            rows.push(Some(FocusTarget::CompletedStage(si)));
            if stage.expanded {
                for ti in 0..stage.tasks.len() {
                    rows.push(Some(FocusTarget::CompletedTask { stage: si, task: ti }));
                }
            }
        }
        if self.current_stage.is_some() {
            if !self.completed_stages.is_empty() {
                rows.push(None); // separator
            }
            rows.push(None); // header
            rows.push(None); // empty line
        }
        for ti in 0..self.tasks.len() {
            rows.push(Some(FocusTarget::CurrentTask(ti)));
        }
        rows
    }

    /// Move focus by delta steps through focusable items
    fn move_focus(&mut self, delta: i32) {
        let focusable: Vec<FocusTarget> = self.build_visual_rows().into_iter().flatten().collect();
        if focusable.is_empty() {
            return;
        }
        let current = focusable.iter().position(|i| *i == self.focus).unwrap_or(0);
        let new_idx = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (current + delta as usize).min(focusable.len() - 1)
        };
        self.focus = focusable[new_idx];
        self.output_scroll.reset();
        self.ensure_focused_visible();
    }

    /// Toggle pin on a target. If already pinned to the same target, unpin.
    fn toggle_pin(&mut self, pin: PinTarget) {
        if self.pinned_task == Some(pin) {
            self.pinned_task = None;
        } else {
            self.pinned_task = Some(pin);
        }
        self.output_scroll.reset();
    }

    /// Handle Enter key: toggle expand on stage headers, toggle pin on tasks
    fn handle_enter(&mut self) {
        match self.focus {
            FocusTarget::CompletedStage(idx) => {
                if let Some(stage) = self.completed_stages.get_mut(idx) {
                    stage.toggle_expanded();
                }
            }
            FocusTarget::CompletedTask { stage, task } => {
                self.toggle_pin(PinTarget::CompletedTask { stage, task });
            }
            FocusTarget::CurrentTask(idx) => {
                self.toggle_pin(PinTarget::CurrentTask(idx));
            }
        }
    }

    /// Ensure the focused item is visible in the task list by adjusting scroll
    fn ensure_focused_visible(&mut self) {
        let area = self.task_list_area.get();
        let visible_height = area.height.saturating_sub(2) as usize; // borders
        if visible_height == 0 {
            return;
        }

        let focused_line = self.build_visual_rows().iter()
            .position(|r| *r == Some(self.focus))
            .unwrap_or(0);

        if focused_line < self.task_list_scroll {
            self.task_list_scroll = focused_line;
        }
        if focused_line >= self.task_list_scroll + visible_height {
            self.task_list_scroll = focused_line - visible_height + 1;
        }
    }

    /// Map a mouse click position to a FocusTarget
    fn focus_target_from_click(&self, col: u16, row: u16) -> Option<FocusTarget> {
        if !self.is_over_task_list(col, row) {
            return None;
        }
        let area = self.task_list_area.get();
        let row_in_list = row.saturating_sub(area.y + 1) as usize + self.task_list_scroll;
        self.build_visual_rows().get(row_in_list).copied().flatten()
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
        let total_lines = self.build_visual_rows().len();
        let area = self.task_list_area.get();
        let visible_height = area.height.saturating_sub(2) as usize;
        let max_scroll = total_lines.saturating_sub(visible_height);

        if delta < 0 {
            self.task_list_scroll = self.task_list_scroll.saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.task_list_scroll = (self.task_list_scroll + delta as usize).min(max_scroll);
        }
    }

    /// Resolve the task shown in the output panel: pinned > focused completed task > None
    /// (Current tasks use stacked mode unless pinned)
    fn resolve_output_task(&self) -> Option<&TaskInfo> {
        if let Some(ref pin) = self.pinned_task {
            return pin.resolve_task(&self.completed_stages, &self.tasks);
        }
        if let FocusTarget::CompletedTask { stage, task } = self.focus {
            return self.completed_stages.get(stage).and_then(|s| s.tasks.get(task));
        }
        None
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

        // Compute auto-scroll when showing single task output (pinned or focused)
        let output_content_height = self.resolve_output_task()
            .map(|t| t.screen_text().lines.len());
        if let Some(content_height) = output_content_height {
            let terminal = self.terminal.as_mut().unwrap();
            let visible_height = terminal.size()?.height.saturating_sub(10) as usize;
            let max_scroll = content_height.saturating_sub(visible_height);

            if self.output_scroll.auto_scroll {
                self.output_scroll.offset = max_scroll as u16;
            }

            self.output_scroll.offset = (self.output_scroll.offset as usize).min(max_scroll) as u16;

            if self.output_scroll.offset as usize >= max_scroll && max_scroll > 0 {
                self.output_scroll.auto_scroll = true;
            }
        }

        let tasks = &self.tasks;
        let task_name = &self.task_name;
        let focus = self.focus;
        let output_scroll_offset = self.output_scroll.offset;
        let task_list_scroll = self.task_list_scroll;
        let completed_stages = &self.completed_stages;
        let current_stage = &self.current_stage;
        let task_list_area_rc = self.task_list_area.clone();
        let output_area_rc = self.output_area.clone();
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
            header::draw_header(frame, chunks[0], task_name, tasks, completed_stages);

            // Tasks area
            if tasks.is_empty() && completed_stages.is_empty() {
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

                // Task list with completed stages
                task_list::draw_task_list(frame, task_chunks[0], tasks, completed_stages, current_stage.as_deref(), focus, pinned_task, task_list_scroll);

                // Right panel: pinned > focused completed task > stacked mode
                let output_task: Option<&TaskInfo> = if let Some(ref pin) = pinned_task {
                    pin.resolve_task(completed_stages, tasks)
                } else if let FocusTarget::CompletedTask { stage: si, task: ti } = focus {
                    completed_stages.get(si).and_then(|s| s.tasks.get(ti))
                } else {
                    None
                };

                if let Some(task) = output_task {
                    task_output::draw_task_output(frame, task_chunks[1], task, output_scroll_offset);
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
            // j / Tab: next item
            KeyCode::Char('j') | KeyCode::Tab => {
                self.move_focus(1);
            }
            // k / Shift+Tab: previous item
            KeyCode::Char('k') | KeyCode::BackTab => {
                self.move_focus(-1);
            }
            // Enter: expand/collapse stage or toggle pin on task
            KeyCode::Enter => {
                self.handle_enter();
            }
            KeyCode::Up => {
                if self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(-1);
                }
            }
            KeyCode::Down => {
                if self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(1);
                }
            }
            KeyCode::PageUp => {
                if self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(-10);
                }
            }
            KeyCode::PageDown => {
                if self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(10);
                }
            }
            KeyCode::End => {
                if self.resolve_output_task().is_some() {
                    self.output_scroll.jump_to_end();
                }
            }
            KeyCode::Home => {
                if self.resolve_output_task().is_some() {
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
                if let Some(target) = self.focus_target_from_click(col, row) {
                    self.focus = target;
                    self.handle_enter();
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_over_task_list(col, row) {
                    self.scroll_task_list(-3);
                } else if self.is_over_output(col, row) && self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_over_task_list(col, row) {
                    self.scroll_task_list(3);
                } else if self.is_over_output(col, row) && self.resolve_output_task().is_some() {
                    self.output_scroll.scroll_by(3);
                }
            }
            _ => {}
        }
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
            self.focus = FocusTarget::CurrentTask(idx);
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
        // Move current tasks into a completed stage (preserving task data)
        if !self.tasks.is_empty() {
            let stage_name = self.current_stage.take().unwrap_or_else(|| "Default".to_string());
            let tasks = std::mem::take(&mut self.tasks);
            self.completed_stages.push(CompletedStage::from_tasks(stage_name, tasks));
            self.task_map.clear();

            // Convert pin: CurrentTask → CompletedTask in the new completed stage
            if let Some(PinTarget::CurrentTask(idx)) = self.pinned_task {
                let stage_idx = self.completed_stages.len() - 1;
                self.pinned_task = Some(PinTarget::CompletedTask { stage: stage_idx, task: idx });
            }
        }

        self.current_stage = Some(name.to_string());
        self.task_list_scroll = 0;
        self.focus = FocusTarget::CurrentTask(0);
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
        // Route logs to the focused current task (not completed tasks)
        if let FocusTarget::CurrentTask(idx) = self.focus {
            if let Some(task) = self.tasks.get(idx) {
                if let Ok(mut parser) = task.parser.lock() {
                    let formatted = format!("{}\r\n", message);
                    parser.process(formatted.as_bytes());
                }
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

        // Auto-focus on running task only when not pinned and focus is on current tasks
        // (don't override user navigation to completed stages)
        if self.pinned_task.is_none() && matches!(self.focus, FocusTarget::CurrentTask(_)) {
            if let Some(idx) = self.tasks.iter().position(|t| t.state == TaskState::Running) {
                self.focus = FocusTarget::CurrentTask(idx);
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

        // Print output of failed tasks from all stages + current tasks
        let all_tasks = self.completed_stages.iter()
            .flat_map(|s| s.tasks.iter())
            .chain(self.tasks.iter());

        for task in all_tasks {
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
