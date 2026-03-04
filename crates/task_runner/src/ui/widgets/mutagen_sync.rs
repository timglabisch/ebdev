use crate::command::{MutagenSessionProgress, MutagenSyncPhase};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

const BAR_WIDTH: usize = 10;

fn progress_bar(percent: u8, style: Style) -> Vec<Span<'static>> {
    let filled = (percent as usize * BAR_WIDTH / 100).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    vec![
        Span::styled("█".repeat(filled), style),
        Span::styled("░".repeat(empty), Style::default().fg(Color::DarkGray)),
    ]
}

pub fn draw_mutagen_sync(frame: &mut Frame, area: Rect, sessions: &[MutagenSessionProgress]) {
    if sessions.is_empty() || area.height < 3 {
        return;
    }

    // Gesamt-Prozent = Durchschnitt
    let total_percent = sessions.iter().map(|s| s.percent as u32).sum::<u32>() / sessions.len() as u32;
    let title = format!(" Mutagen Sync ─── {}% ", total_percent);

    let any_halted = sessions.iter().any(|s| matches!(s.phase, MutagenSyncPhase::Halted(_)));
    let all_ready = sessions.iter().all(|s| s.phase == MutagenSyncPhase::Ready);
    let border_style = if any_halted {
        Style::default().fg(Color::Red)
    } else if all_ready {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    };

    let max_visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = sessions.iter().take(max_visible).map(|s| {
        let (icon, icon_style) = match &s.phase {
            MutagenSyncPhase::Ready => ("✓", Style::default().fg(Color::Green)),
            MutagenSyncPhase::Active => ("●", Style::default().fg(Color::Yellow)),
            MutagenSyncPhase::Pending => ("◌", Style::default().fg(Color::DarkGray)),
            MutagenSyncPhase::Halted(_) => ("✗", Style::default().fg(Color::Red)),
        };
        let name_style = match &s.phase {
            MutagenSyncPhase::Pending => Style::default().fg(Color::DarkGray),
            MutagenSyncPhase::Halted(_) => Style::default().fg(Color::Red),
            _ => Style::default(),
        };
        let bar_color = match &s.phase {
            MutagenSyncPhase::Ready => Style::default().fg(Color::Green),
            MutagenSyncPhase::Active => Style::default().fg(Color::Yellow),
            MutagenSyncPhase::Halted(_) => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::DarkGray),
        };

        let mut spans = vec![
            Span::raw("  "),
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(format!("{:<12}", s.name), name_style),
            Span::styled(format!("{:<18}", s.status_label), Style::default().fg(Color::DarkGray)),
        ];
        spans.extend(progress_bar(s.percent, bar_color));
        Line::from(spans)
    }).collect();

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );
    frame.render_widget(widget, area);
}
