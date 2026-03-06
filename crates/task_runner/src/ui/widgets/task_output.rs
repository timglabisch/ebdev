use crate::ui::types::{TaskInfo, TaskState};
use ratatui::prelude::*;
use ratatui::layout::Margin;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

pub fn draw_task_output(frame: &mut Frame, area: Rect, task: &TaskInfo, scroll_offset: u16, h_scroll: u16, focused: bool) {
    let text = task.screen_text();

    let border_style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        match &task.state {
            TaskState::Running => Style::default().fg(Color::Yellow),
            TaskState::Completed { exit_code, .. } if *exit_code == 0 => Style::default().fg(Color::Green),
            TaskState::Completed { .. } | TaskState::Failed { .. } => Style::default().fg(Color::Red),
        }
    };

    let inner_width = area.width.saturating_sub(2) as usize;
    let title_text = if focused {
        format!(" ▸ {} ", task.name)
    } else {
        format!(" {} ", task.name)
    };
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
        .map(|line| if h_scroll > 0 { h_scroll_line(line, h_scroll) } else { line })
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

pub fn draw_stacked_outputs(frame: &mut Frame, area: Rect, tasks: &[TaskInfo], h_scroll: u16, focused_idx: Option<usize>) {
    // Collect running tasks (up to 3) with their original index
    let mut display_tasks: Vec<(usize, &TaskInfo)> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| matches!(t.state, TaskState::Running))
        .take(3)
        .collect();

    // If no running tasks but tasks exist, show the last task
    if display_tasks.is_empty() {
        if let Some((idx, last)) = tasks.iter().enumerate().last() {
            display_tasks.push((idx, last));
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

    for (i, (orig_idx, task)) in display_tasks.iter().enumerate() {
        let is_focused = focused_idx == Some(*orig_idx);
        // Auto-scroll to bottom in stacked mode
        draw_task_output(frame, chunks[i], task, u16::MAX, h_scroll, is_focused);
    }
}

/// Horizontally scroll a Line by trimming `offset` characters from the left of its spans.
fn h_scroll_line(line: Line<'_>, offset: u16) -> Line<'_> {
    let mut remaining = offset as usize;
    let mut new_spans = Vec::new();
    for span in line.spans {
        if remaining == 0 {
            new_spans.push(span);
            continue;
        }
        let content_len = span.content.chars().count();
        if remaining >= content_len {
            remaining -= content_len;
            // Skip entire span
        } else {
            // Trim `remaining` chars from the left
            let trimmed: String = span.content.chars().skip(remaining).collect();
            new_spans.push(Span::styled(trimmed, span.style));
            remaining = 0;
        }
    }
    Line::from(new_spans)
}
