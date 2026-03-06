use crate::command::RegisteredTask;
use crate::ui::types::truncate_string;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::cell::Cell;
use std::rc::Rc;

const NAME_LEN: usize = 20;
const DESC_LEN: usize = 50;

pub fn draw_task_browser(
    frame: &mut Frame,
    area: Rect,
    tasks: &[RegisteredTask],
    selected: usize,
    browser_area: &Rc<Cell<(Rect, usize)>>,
) {
    // Store area + task count for mouse hit-testing
    browser_area.set((area, tasks.len()));

    let lines: Vec<Line> = if tasks.is_empty() {
        vec![Line::from(Span::styled(
            "  No tasks registered",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        tasks.iter().enumerate().map(|(i, task)| {
            let is_selected = i == selected;
            let prefix = if is_selected { " > " } else { "   " };
            let style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let name = truncate_string(&task.name, NAME_LEN);
            let desc = truncate_string(&task.description, DESC_LEN);

            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name, style),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(Color::DarkGray)),
            ])
        }).collect()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Tasks ");

    let list = Paragraph::new(lines).block(block);
    frame.render_widget(list, area);
}
