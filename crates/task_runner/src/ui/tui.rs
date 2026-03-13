use super::TaskRunnerUI;
use super::types::{CompletedStage, FocusTarget, PinTarget, TaskInfo, TaskState, format_bytes, row_from_click};
use super::widgets::command_palette::{self, CommandPaletteState};
use super::widgets::tab_bar::{self, ActiveTab};
use super::widgets::{flag_browser, help, task_browser, task_list, task_output};
use crate::command::{CommandId, CommandResult, FlagDisplay, RegisteredTask};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::cursor;
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
    h_offset: u16,
    auto_scroll: bool,
}

impl ScrollState {
    fn new() -> Self {
        Self { offset: 0, h_offset: 0, auto_scroll: true }
    }

    fn scroll_by(&mut self, delta: i32) {
        if delta < 0 {
            self.offset = self.offset.saturating_sub(delta.unsigned_abs() as u16);
            self.auto_scroll = false;
        } else {
            self.offset = self.offset.saturating_add(delta as u16);
        }
    }

    fn scroll_h_by(&mut self, delta: i32) {
        if delta < 0 {
            self.h_offset = self.h_offset.saturating_sub(delta.unsigned_abs() as u16);
        } else {
            self.h_offset = self.h_offset.saturating_add(delta as u16);
        }
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.h_offset = 0;
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
    /// Stateful mutagen sync widget
    mutagen_widget: super::widgets::mutagen_sync::MutagenSyncWidget,
    /// Compact mode: hide sidebar, output uses full width
    compact_mode: bool,
    /// Stored geometry of the compact toggle area in help line (for mouse hit-testing)
    help_compact_area: Rc<Cell<Rect>>,
    /// Pending kill request from 'x' key
    kill_request: Option<CommandId>,
    /// True after user navigated with j/k — suppresses auto-focus until new task starts
    user_navigated: bool,
    /// Active tab (Output, Tasks, or Flags)
    active_tab: ActiveTab,
    /// Stored geometry of tab bar labels (for mouse hit-testing)
    tab_areas: [Rc<Cell<Rect>>; 3],
    /// Selected index in the task browser
    task_browser_selected: usize,
    /// Stored geometry + count for task browser rows (for mouse hit-testing)
    task_browser_area: Rc<Cell<(Rect, usize)>>,
    /// Feature flags for the Flags tab
    flags: Vec<FlagDisplay>,
    /// Selected index in the flag browser
    flag_browser_selected: usize,
    /// Stored geometry + count for flag browser rows (for mouse hit-testing)
    flag_browser_area: Rc<Cell<(Rect, usize)>>,
    /// Tick counter for brand animation
    brand_tick: usize,
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
            mutagen_widget: super::widgets::mutagen_sync::MutagenSyncWidget::new(),
            compact_mode: false,
            help_compact_area: Rc::new(Cell::new(Rect::default())),
            kill_request: None,
            user_navigated: false,
            active_tab: ActiveTab::Output,
            tab_areas: [Rc::new(Cell::new(Rect::default())), Rc::new(Cell::new(Rect::default())), Rc::new(Cell::new(Rect::default()))],
            task_browser_selected: 0,
            task_browser_area: Rc::new(Cell::new((Rect::default(), 0))),
            flags: Vec::new(),
            flag_browser_selected: 0,
            flag_browser_area: Rc::new(Cell::new((Rect::default(), 0))),
            brand_tick: 0,
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
        self.user_navigated = true;
        self.output_scroll.reset();
        self.ensure_focused_visible();
    }

    /// Clear all completed stages and reset related state
    fn clear_completed(&mut self) {
        self.completed_stages.clear();
        // If pinned to a completed task, unpin
        if matches!(self.pinned_task, Some(PinTarget::CompletedTask { .. })) {
            self.pinned_task = None;
        }
        self.task_list_scroll = 0;
        self.focus = FocusTarget::CurrentTask(0);
        self.user_navigated = false;
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
        self.task_list_area.get().contains(Position::new(col, row))
    }

    /// Check if a mouse position is over the output panel
    fn is_over_output(&self, col: u16, row: u16) -> bool {
        self.output_area.get().contains(Position::new(col, row))
    }

    /// Check if a mouse position is over the compact toggle in the help line
    fn is_over_help_compact(&self, col: u16, row: u16) -> bool {
        self.help_compact_area.get().contains(Position::new(col, row))
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
        self.brand_tick = self.brand_tick.wrapping_add(1);

        // Collect all data we need before borrowing terminal mutably
        let palette_open = self.palette.open;
        let filtered_tasks: Vec<RegisteredTask> = self.palette.filter_tasks(&self.registered_tasks)
            .into_iter()
            .cloned()
            .collect();
        let has_registered_tasks = !self.registered_tasks.is_empty();
        let is_idle = self.tasks.is_empty()
            || self.tasks.iter().all(|t| t.state != TaskState::Running);
        let auto_quit = self.auto_quit;
        let pinned_task = self.pinned_task;
        let compact_mode = self.compact_mode;

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
        let help_compact_area_rc = self.help_compact_area.clone();
        let active_tab = self.active_tab;
        let task_count = self.registered_tasks.len();
        let flag_count = self.flags.len();
        let tab_areas = self.tab_areas.clone();
        let task_browser_selected = self.task_browser_selected;
        let task_browser_area_rc = self.task_browser_area.clone();
        let registered_tasks = &self.registered_tasks;
        let flags = &self.flags;
        let flag_browser_selected = self.flag_browser_selected;
        let flag_browser_area_rc = self.flag_browser_area.clone();
        let palette = &self.palette;
        let mutagen_widget = &self.mutagen_widget;
        let h_scroll = self.output_scroll.h_offset;
        let brand_tick = self.brand_tick;
        let terminal = self.terminal.as_mut().unwrap();
        terminal.draw(|frame| {
            let area = frame.area();

            let mutagen_height = mutagen_widget.height()
                .min(area.height / 3);

            // Main layout: tab bar + mutagen + tasks + help
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),              // Tab bar [0]
                    Constraint::Length(mutagen_height), // Mutagen Sync [1]
                    Constraint::Min(5),                // Tasks [2]
                    Constraint::Length(1),              // Help [3]
                ])
                .split(area);

            // Tab bar
            tab_bar::draw_tab_bar(frame, chunks[0], active_tab, task_count, flag_count, &tab_areas);

            // Mutagen Sync widget
            if !mutagen_widget.is_empty() {
                mutagen_widget.draw(frame, chunks[1]);
            }

            match active_tab {
                ActiveTab::Output => {
                    // Tasks area
                    if tasks.is_empty() && completed_stages.is_empty() {
                        let waiting = Paragraph::new("Waiting for tasks...")
                            .style(Style::default().fg(Color::DarkGray))
                            .block(Block::default().borders(Borders::ALL).title(" Tasks "));
                        frame.render_widget(waiting, chunks[2]);
                    } else {
                        // Compute layout: compact = full width, normal = sidebar + output
                        let output_rect = if compact_mode {
                            task_list_area_rc.set(Rect::default());
                            output_area_rc.set(chunks[2]);
                            chunks[2]
                        } else {
                            let task_chunks = Layout::default()
                                .direction(Direction::Horizontal)
                                .constraints([
                                    Constraint::Length(40.min(chunks[2].width / 3)),
                                    Constraint::Min(20),
                                ])
                                .split(chunks[2]);

                            task_list_area_rc.set(task_chunks[0]);
                            output_area_rc.set(task_chunks[1]);

                            task_list::draw_task_list(frame, task_chunks[0], task_name, tasks, completed_stages, current_stage.as_deref(), focus, pinned_task, task_list_scroll);

                            task_chunks[1]
                        };

                        // Resolve output task: pinned > focused completed task > stacked mode
                        let output_task: Option<&TaskInfo> = if let Some(ref pin) = pinned_task {
                            pin.resolve_task(completed_stages, tasks)
                        } else if let FocusTarget::CompletedTask { stage: si, task: ti } = focus {
                            completed_stages.get(si).and_then(|s| s.tasks.get(ti))
                        } else {
                            None
                        };

                        if let Some(task) = output_task {
                            task_output::draw_task_output(frame, output_rect, task, output_scroll_offset, h_scroll, false);
                        } else {
                            let focused_idx = if compact_mode {
                                if let FocusTarget::CurrentTask(idx) = focus { Some(idx) } else { None }
                            } else {
                                None
                            };
                            task_output::draw_stacked_outputs(frame, output_rect, tasks, h_scroll, focused_idx);
                        }
                    }
                }
                ActiveTab::Tasks => {
                    task_browser::draw_task_browser(frame, chunks[2], registered_tasks, task_browser_selected, &task_browser_area_rc);
                }
                ActiveTab::Flags => {
                    flag_browser::draw_flag_browser(frame, chunks[2], flags, flag_browser_selected, &flag_browser_area_rc);
                }
            }

            // Help line
            help::draw_help(frame, chunks[3], has_registered_tasks, auto_quit, compact_mode, active_tab, &help_compact_area_rc, brand_tick, is_idle);

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
                    // Ctrl+C → quit
                    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                        self.should_quit = true;
                        return Ok(false);
                    }
                    if self.palette.open {
                        return self.handle_command_palette_input(key.code);
                    }
                    self.handle_key(key.code, key.modifiers)?;
                }
                Event::Mouse(mouse) => {
                    self.handle_mouse(mouse);
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> io::Result<()> {
        // Global keys (always available outside command palette)
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.active_tab != ActiveTab::Output {
                    self.active_tab = ActiveTab::Output;
                    return Ok(());
                }
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Char('/') => {
                if !self.registered_tasks.is_empty() {
                    self.active_tab = ActiveTab::Output;
                    self.palette.open();
                    self.auto_quit = false;
                }
                return Ok(());
            }
            KeyCode::Char('1') => {
                self.active_tab = ActiveTab::Output;
                return Ok(());
            }
            KeyCode::Char('2') => {
                if !self.registered_tasks.is_empty() {
                    self.active_tab = ActiveTab::Tasks;
                    self.auto_quit = false;
                }
                return Ok(());
            }
            KeyCode::Char('3') => {
                if !self.flags.is_empty() {
                    self.active_tab = ActiveTab::Flags;
                    self.auto_quit = false;
                }
                return Ok(());
            }
            _ => {}
        }

        // Tab-specific keys
        match self.active_tab {
            ActiveTab::Output => {
                match code {
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
                    KeyCode::Char('x') => {
                        // Kill the focused running task
                        if let FocusTarget::CurrentTask(idx) = self.focus {
                            if let Some(task) = self.tasks.get_mut(idx) {
                                if task.state == TaskState::Running {
                                    self.kill_request = Some(task.id);
                                    task.killed = true;
                                }
                            }
                        }
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::SHIFT) => {
                        self.clear_completed();
                    }
                    KeyCode::Char('C') => {
                        self.clear_completed();
                    }
                    KeyCode::Char('c') => {
                        self.compact_mode = !self.compact_mode;
                    }
                    KeyCode::Left => {
                        self.output_scroll.scroll_h_by(-4);
                    }
                    KeyCode::Right => {
                        self.output_scroll.scroll_h_by(4);
                    }
                    _ => {}
                }
            }
            ActiveTab::Tasks => {
                match code {
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                        if !self.registered_tasks.is_empty() {
                            self.task_browser_selected = (self.task_browser_selected + 1)
                                .min(self.registered_tasks.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => {
                        self.task_browser_selected = self.task_browser_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        self.trigger_registered_task(self.task_browser_selected);
                    }
                    _ => {}
                }
            }
            ActiveTab::Flags => {
                match code {
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                        if !self.flags.is_empty() {
                            self.flag_browser_selected = (self.flag_browser_selected + 1)
                                .min(self.flags.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => {
                        self.flag_browser_selected = self.flag_browser_selected.saturating_sub(1);
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        self.toggle_flag(self.flag_browser_selected);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Trigger a registered task by index, switching back to Output tab
    fn trigger_registered_task(&mut self, idx: usize) {
        if let Some(task) = self.registered_tasks.get(idx) {
            self.triggered_task = Some(task.name.clone());
            self.active_tab = ActiveTab::Output;
        }
    }

    /// Check if a mouse position is over a tab bar label
    fn tab_from_click(&self, col: u16, row: u16) -> Option<ActiveTab> {
        let pos = Position::new(col, row);
        if self.tab_areas[0].get().contains(pos) {
            return Some(ActiveTab::Output);
        }
        if self.tab_areas[1].get().contains(pos) {
            return Some(ActiveTab::Tasks);
        }
        if self.tab_areas[2].get().contains(pos) {
            return Some(ActiveTab::Flags);
        }
        None
    }

    /// Check if a mouse position is over the task browser, returns the row index
    fn task_browser_row_from_click(&self, col: u16, row: u16) -> Option<usize> {
        let (area, count) = self.task_browser_area.get();
        row_from_click(area, count, col, row)
    }

    /// Check if a mouse position is over the flag browser, returns the row index
    fn flag_browser_row_from_click(&self, col: u16, row: u16) -> Option<usize> {
        let (area, count) = self.flag_browser_area.get();
        row_from_click(area, count, col, row)
    }

    /// Toggle a flag at the given index, handling dependency cascades and saving
    fn toggle_flag(&mut self, idx: usize) {
        if flag_browser::toggle_flag(&mut self.flags, idx) {
            flag_browser::save_flags(&self.flags);
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Tab bar click
                if let Some(tab) = self.tab_from_click(col, row) {
                    if tab == ActiveTab::Tasks && self.registered_tasks.is_empty() {
                        return;
                    }
                    if tab == ActiveTab::Flags && self.flags.is_empty() {
                        return;
                    }
                    self.active_tab = tab;
                    if tab != ActiveTab::Output {
                        self.auto_quit = false;
                    }
                    return;
                }

                // Task browser click (when Tasks tab active)
                if self.active_tab == ActiveTab::Tasks {
                    if let Some(idx) = self.task_browser_row_from_click(col, row) {
                        self.task_browser_selected = idx;
                        self.trigger_registered_task(idx);
                    }
                    return;
                }

                // Flag browser click (when Flags tab active)
                if self.active_tab == ActiveTab::Flags {
                    if let Some(idx) = self.flag_browser_row_from_click(col, row) {
                        self.flag_browser_selected = idx;
                        self.toggle_flag(idx);
                    }
                    return;
                }

                // Output tab: existing behavior
                if self.is_over_help_compact(col, row) {
                    self.compact_mode = !self.compact_mode;
                } else if let Some(target) = self.focus_target_from_click(col, row) {
                    self.focus = target;
                    self.handle_enter();
                }
            }
            MouseEventKind::ScrollUp => {
                if self.active_tab == ActiveTab::Output {
                    if self.is_over_task_list(col, row) {
                        self.scroll_task_list(-3);
                    } else if self.is_over_output(col, row) && self.resolve_output_task().is_some() {
                        self.output_scroll.scroll_by(-3);
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.active_tab == ActiveTab::Output {
                    if self.is_over_task_list(col, row) {
                        self.scroll_task_list(3);
                    } else if self.is_over_output(col, row) && self.resolve_output_task().is_some() {
                        self.output_scroll.scroll_by(3);
                    }
                }
            }
            MouseEventKind::ScrollLeft => {
                if self.active_tab == ActiveTab::Output && self.is_over_output(col, row) {
                    self.output_scroll.scroll_h_by(-4);
                }
            }
            MouseEventKind::ScrollRight => {
                if self.active_tab == ActiveTab::Output && self.is_over_output(col, row) {
                    self.output_scroll.scroll_h_by(4);
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
            self.user_navigated = false;
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
            // If the completed task was focused, resume auto-focus
            // so the next running task gets tracked
            if self.focus == FocusTarget::CurrentTask(idx) {
                self.user_navigated = false;
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

    fn poll_kill_request(&mut self) -> Option<CommandId> {
        self.kill_request.take()
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

    fn on_mutagen_sync_status(&mut self, sessions: &[crate::command::MutagenSessionProgress]) {
        self.mutagen_widget.update(sessions);
    }

    fn on_mutagen_sync_clear(&mut self) {
        self.mutagen_widget.clear();
    }

    fn on_compact_mode(&mut self, enabled: bool) {
        self.compact_mode = enabled;
    }

    fn on_clear_completed(&mut self) {
        self.clear_completed();
    }

    fn on_flags_set(&mut self, flags: &[FlagDisplay]) {
        self.flags = flags.to_vec();
        if self.flag_browser_selected >= self.flags.len() {
            self.flag_browser_selected = 0;
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

        // Auto-focus on running task only when not pinned, not manually navigated,
        // and focus is on current tasks (don't override user navigation)
        if self.pinned_task.is_none() && !self.user_navigated && matches!(self.focus, FocusTarget::CurrentTask(_)) {
            if let Some(idx) = self.tasks.iter().position(|t| t.state == TaskState::Running) {
                self.focus = FocusTarget::CurrentTask(idx);
                self.ensure_focused_visible();
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
            io::stdout().execute(cursor::Show)?;
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
            io::stdout().execute(cursor::Hide)?;
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
