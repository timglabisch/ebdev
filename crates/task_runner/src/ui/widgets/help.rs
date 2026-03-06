use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::rc::Rc;

pub fn draw_help(frame: &mut Frame, area: Rect, has_registered_tasks: bool, auto_quit: bool, compact_mode: bool, compact_area: &Rc<Cell<Rect>>) {
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
        "j/k: navigate | Enter: expand/pin | ↑↓: scroll | /: run task | "
    } else {
        "j/k: navigate | Enter: expand/pin | ↑↓: scroll | "
    };
    spans.push(Span::styled(help_text, Style::default().fg(Color::DarkGray)));

    // Calculate x position for compact toggle
    let x_before_compact: u16 = spans.iter().map(|s| s.width() as u16).sum::<u16>() + area.x;

    // Compact mode toggle (clickable)
    let compact_label = if compact_mode { "c: sidebar" } else { "c: compact" };
    spans.push(Span::styled(compact_label, Style::default().fg(Color::DarkGray)));

    spans.push(Span::styled(" | x: kill", Style::default().fg(Color::DarkGray)));

    // Store the area of the compact toggle for mouse hit-testing
    compact_area.set(Rect::new(x_before_compact, area.y, compact_label.len() as u16, 1));

    spans.push(Span::styled(" | q: quit", Style::default().fg(Color::DarkGray)));

    let help = Paragraph::new(Line::from(spans));
    frame.render_widget(help, area);
}
