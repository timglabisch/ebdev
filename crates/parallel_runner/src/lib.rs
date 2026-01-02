use std::io::{self, Read, Stdout, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParallelRunnerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("PTY error: {0}")]
    Pty(String),
}

/// Definition of a command to run
#[derive(Clone)]
pub struct CommandDef {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
}

impl CommandDef {
    pub fn new(name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
        }
    }

    pub fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }
}

/// A terminal pane running a command
struct TerminalPane {
    name: String,
    parser: Arc<Mutex<vt100::Parser>>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    scroll_offset: u16,
}

impl TerminalPane {
    fn new(cmd: &CommandDef, rows: u16, cols: u16) -> Result<Self, ParallelRunnerError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| ParallelRunnerError::Pty(e.to_string()))?;

        let mut command = CommandBuilder::new(&cmd.program);
        for arg in &cmd.args {
            command.arg(arg);
        }
        if let Some(dir) = &cmd.working_dir {
            command.cwd(dir);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| ParallelRunnerError::Pty(e.to_string()))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 1000))); // 1000 lines scrollback

        // Get writer for sending input
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| ParallelRunnerError::Pty(e.to_string()))?;

        // Spawn reader thread
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| ParallelRunnerError::Pty(e.to_string()))?;
        let parser_clone = Arc::clone(&parser);

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        if let Ok(mut parser) = parser_clone.lock() {
                            parser.process(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            name: cmd.name.clone(),
            parser,
            _child: child,
            writer,
            scroll_offset: 0,
        })
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        // Note: PTY resize not supported after taking writer, but we can resize the parser
        if let Ok(mut parser) = self.parser.lock() {
            parser.set_size(rows, cols);
        }
    }

    fn write_input(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    fn screen_content(&self) -> Vec<Line<'static>> {
        let parser = match self.parser.lock() {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let screen = parser.screen();

        let mut lines = Vec::new();
        let (rows, cols) = screen.size();

        for row in 0..rows {
            let mut spans = Vec::new();
            let mut current_text = String::new();
            let mut current_style = Style::default();

            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    let style = cell_to_style(&cell);

                    if style != current_style && !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = style;
                    current_text.push_str(&cell.contents());
                }
            }

            if !current_text.is_empty() {
                spans.push(Span::styled(current_text, current_style));
            }

            lines.push(Line::from(spans));
        }

        lines
    }

    fn scrollback_size(&self) -> usize {
        self.parser
            .lock()
            .map(|p| p.screen().scrollback())
            .unwrap_or(0)
    }
}

fn cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    // Foreground color
    let fg = cell.fgcolor();
    if !matches!(fg, vt100::Color::Default) {
        style = style.fg(vt100_color_to_ratatui(fg));
    }

    // Background color
    let bg = cell.bgcolor();
    if !matches!(bg, vt100::Color::Default) {
        style = style.bg(vt100_color_to_ratatui(bg));
    }

    // Attributes
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(idx) => Color::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Application state
pub struct App {
    panes: Vec<TerminalPane>,
    focused_pane: usize,
    layout: Layout,
    should_quit: bool,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Layout {
    /// Stacked vertically, full width, showing last lines of output
    #[default]
    Stacked,
    /// Side by side
    Horizontal,
    /// Top to bottom, equal height
    Vertical,
    /// Grid layout for many panes
    Grid,
}

impl App {
    pub fn new(commands: Vec<CommandDef>, layout: Layout) -> Result<Self, ParallelRunnerError> {
        // Initial size - will be adjusted on first render
        let panes: Result<Vec<_>, _> = commands
            .iter()
            .map(|cmd| TerminalPane::new(cmd, 24, 80))
            .collect();

        Ok(Self {
            panes: panes?,
            focused_pane: 0,
            layout,
            should_quit: false,
        })
    }

    fn resize_panes(&mut self, width: u16, height: u16) {
        let pane_count = self.panes.len();
        if pane_count == 0 {
            return;
        }

        let (pane_width, pane_height) = match self.layout {
            Layout::Stacked => (width.saturating_sub(2), height / pane_count as u16),
            Layout::Horizontal => (width / pane_count as u16, height.saturating_sub(2)),
            Layout::Vertical => (width.saturating_sub(2), height / pane_count as u16),
            Layout::Grid => {
                let cols = (pane_count as f64).sqrt().ceil() as u16;
                let rows = ((pane_count as f64) / cols as f64).ceil() as u16;
                (width / cols, height / rows)
            }
        };

        for pane in &mut self.panes {
            pane.resize(pane_height.saturating_sub(2), pane_width.saturating_sub(2));
        }
    }

    fn next_pane(&mut self) {
        if !self.panes.is_empty() {
            self.focused_pane = (self.focused_pane + 1) % self.panes.len();
        }
    }

    fn prev_pane(&mut self) {
        if !self.panes.is_empty() {
            self.focused_pane = (self.focused_pane + self.panes.len() - 1) % self.panes.len();
        }
    }

    fn scroll_up(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane) {
            let max_scroll = pane.scrollback_size() as u16;
            pane.scroll_offset = (pane.scroll_offset + 5).min(max_scroll);
        }
    }

    fn scroll_down(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.focused_pane) {
            pane.scroll_offset = pane.scroll_offset.saturating_sub(5);
        }
    }
}

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn init() -> io::Result<Tui> {
    io::stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

pub fn restore() -> io::Result<()> {
    io::stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

/// Run commands in parallel with TUI visualization
pub fn run_parallel(commands: Vec<CommandDef>, layout: Layout) -> Result<(), ParallelRunnerError> {
    if commands.is_empty() {
        println!("No commands to run.");
        return Ok(());
    }

    let mut terminal = init()?;
    let mut app = App::new(commands, layout)?;

    // Initial resize
    let size = terminal.size()?;
    app.resize_panes(size.width, size.height);

    let result = run_loop(&mut terminal, &mut app);

    restore()?;
    result
}

fn run_loop(terminal: &mut Tui, app: &mut App) -> Result<(), ParallelRunnerError> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('q') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                            app.should_quit = true;
                        }
                        KeyCode::Tab => {
                            app.next_pane();
                        }
                        KeyCode::BackTab => {
                            app.prev_pane();
                        }
                        KeyCode::PageUp => {
                            app.scroll_up();
                        }
                        KeyCode::PageDown => {
                            app.scroll_down();
                        }
                        // Forward other keys to focused pane
                        _ => {
                            if let Some(pane) = app.panes.get_mut(app.focused_pane) {
                                let bytes = key_to_bytes(key.code, key.modifiers);
                                if !bytes.is_empty() {
                                    pane.write_input(&bytes);
                                }
                            }
                        }
                    }
                }
                Event::Resize(width, height) => {
                    app.resize_panes(width, height);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn key_to_bytes(code: KeyCode, modifiers: event::KeyModifiers) -> Vec<u8> {
    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(event::KeyModifiers::CONTROL) {
                // Ctrl+letter sends byte 1-26
                if c.is_ascii_lowercase() {
                    vec![(c as u8) - b'a' + 1]
                } else if c.is_ascii_uppercase() {
                    vec![(c.to_ascii_lowercase() as u8) - b'a' + 1]
                } else {
                    vec![]
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![27],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => vec![],
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let pane_count = app.panes.len();

    if pane_count == 0 {
        let msg = Paragraph::new("No commands running")
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, area);
        return;
    }

    match app.layout {
        Layout::Stacked => draw_stacked(frame, app, area),
        Layout::Horizontal => draw_split(frame, app, area, Direction::Horizontal),
        Layout::Vertical => draw_split(frame, app, area, Direction::Vertical),
        Layout::Grid => draw_grid(frame, app, area),
    }
}

/// Stacked layout: each pane gets a header line + output lines, full width
fn draw_stacked(frame: &mut Frame, app: &App, area: Rect) {
    let pane_count = app.panes.len();

    // Calculate height per pane - minimum 4 lines (1 header + 2 content + 1 border)
    let available_height = area.height.saturating_sub(1); // Reserve 1 for help line
    let height_per_pane = (available_height / pane_count as u16).max(4);

    // Create constraints for each pane
    let mut constraints: Vec<Constraint> = app.panes.iter().enumerate().map(|(i, _)| {
        if i == app.focused_pane {
            // Focused pane gets more space
            Constraint::Min(height_per_pane + 2)
        } else {
            Constraint::Length(height_per_pane)
        }
    }).collect();

    // Add help line at bottom
    constraints.push(Constraint::Length(1));

    let chunks = ratatui::layout::Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Render each pane
    for (i, pane) in app.panes.iter().enumerate() {
        let is_focused = i == app.focused_pane;
        let pane_area = chunks[i];

        // Style based on focus
        let (border_style, title_style) = if is_focused {
            (
                Style::default().fg(Color::Cyan),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::Gray),
            )
        };

        // Build title with command info
        let focus_indicator = if is_focused { "▶ " } else { "  " };
        let title = format!("{}{}", focus_indicator, pane.name);

        // Get last N lines of output that fit in the pane
        let content_height = pane_area.height.saturating_sub(2) as usize; // -2 for borders
        let all_content = pane.screen_content();
        let skip_count = all_content.len().saturating_sub(content_height);
        let content: Vec<Line> = all_content.into_iter().skip(skip_count).collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(title, title_style));

        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, pane_area);
    }

    // Help line at bottom
    let help_text = " Tab: switch pane | PageUp/Down: scroll | Ctrl+Q: quit ";
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[pane_count]);
}

/// Split layout (horizontal or vertical)
fn draw_split(frame: &mut Frame, app: &App, area: Rect, direction: Direction) {
    let pane_count = app.panes.len();

    let chunks = ratatui::layout::Layout::default()
        .direction(direction)
        .constraints(vec![Constraint::Ratio(1, pane_count as u32); pane_count])
        .split(area);

    for (i, pane) in app.panes.iter().enumerate() {
        if i >= chunks.len() {
            break;
        }
        render_pane(frame, pane, chunks[i], i == app.focused_pane);
    }
}

/// Grid layout
fn draw_grid(frame: &mut Frame, app: &App, area: Rect) {
    let pane_count = app.panes.len();
    let cols = (pane_count as f64).sqrt().ceil() as usize;
    let rows = ((pane_count as f64) / cols as f64).ceil() as usize;

    let row_chunks = ratatui::layout::Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Ratio(1, rows as u32); rows])
        .split(area);

    let mut pane_idx = 0;
    for (row_idx, row_area) in row_chunks.iter().enumerate() {
        let panes_in_row = if row_idx == rows - 1 {
            pane_count - (rows - 1) * cols
        } else {
            cols
        };

        let col_chunks = ratatui::layout::Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, panes_in_row as u32); panes_in_row])
            .split(*row_area);

        for col_area in col_chunks.iter() {
            if pane_idx < app.panes.len() {
                render_pane(frame, &app.panes[pane_idx], *col_area, pane_idx == app.focused_pane);
                pane_idx += 1;
            }
        }
    }
}

/// Render a single pane with border and content
fn render_pane(frame: &mut Frame, pane: &TerminalPane, area: Rect, is_focused: bool) {
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" {} ", pane.name);
    let content = pane.screen_content();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);

    // Scrollbar for focused pane with scrollback
    if is_focused && pane.scrollback_size() > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut state = ScrollbarState::new(pane.scrollback_size())
            .position(pane.scrollback_size().saturating_sub(pane.scroll_offset as usize));

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin::new(0, 1)),
            &mut state,
        );
    }
}
