use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

pub fn draw_help(frame: &mut Frame, area: Rect, has_registered_tasks: bool, auto_quit: bool) {
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
        "j/k: navigate | Enter: expand/pin | ↑↓: scroll | /: run task | q: quit"
    } else {
        "j/k: navigate | Enter: expand/pin | ↑↓: scroll | q: quit"
    };
    spans.push(Span::styled(help_text, Style::default().fg(Color::DarkGray)));

    let help = Paragraph::new(Line::from(spans));
    frame.render_widget(help, area);
}
