use crate::command::RegisteredTask;
use crate::ui::types::truncate_string;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

// Command Palette constants
const PALETTE_MAX_WIDTH: u16 = 60;
const PALETTE_MAX_HEIGHT: u16 = 15;
const PALETTE_MARGIN: u16 = 4;
const PALETTE_NAME_LEN: usize = 20;
const PALETTE_DESC_LEN: usize = 30;

/// State for the command palette overlay
pub struct CommandPaletteState {
    pub open: bool,
    pub input: String,
    pub selected: usize,
}

impl CommandPaletteState {
    pub fn new() -> Self {
        Self {
            open: false,
            input: String::new(),
            selected: 0,
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.input.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.input.clear();
    }

    /// Get filtered tasks matching the current input
    pub fn filter_tasks<'a>(&self, tasks: &'a [RegisteredTask]) -> Vec<&'a RegisteredTask> {
        let input_lower = self.input.to_lowercase();
        tasks
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
}

pub fn draw_command_palette(frame: &mut Frame, area: Rect, state: &CommandPaletteState, filtered_tasks: &[RegisteredTask]) {
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
    let input_text = if state.input.is_empty() {
        Span::styled("Type to search...", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw(&state.input)
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
            let is_selected = i == state.selected;
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
