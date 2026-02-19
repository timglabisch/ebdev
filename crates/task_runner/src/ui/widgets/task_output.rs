use crate::ui::types::{TaskInfo, TaskState};
use ratatui::prelude::*;
use ratatui::layout::Margin;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

pub fn draw_task_output(frame: &mut Frame, area: Rect, task: &TaskInfo, scroll_offset: u16) {
    let text = task.screen_text();

    let border_style = match &task.state {
        TaskState::Running => Style::default().fg(Color::Yellow),
        TaskState::Completed { exit_code, .. } if *exit_code == 0 => Style::default().fg(Color::Green),
        TaskState::Completed { .. } | TaskState::Failed { .. } => Style::default().fg(Color::Red),
    };

    let inner_width = area.width.saturating_sub(2) as usize;
    let title_text = format!(" {} ", task.name);
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

pub fn draw_stacked_outputs(frame: &mut Frame, area: Rect, tasks: &[TaskInfo]) {
    // Collect running tasks (up to 3)
    let mut display_tasks: Vec<&TaskInfo> = tasks
        .iter()
        .filter(|t| matches!(t.state, TaskState::Running))
        .take(3)
        .collect();

    // If no running tasks but tasks exist, show the last task
    if display_tasks.is_empty() {
        if let Some(last) = tasks.last() {
            display_tasks.push(last);
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

    for (i, task) in display_tasks.iter().enumerate() {
        // Auto-scroll to bottom in stacked mode
        draw_task_output(frame, chunks[i], task, u16::MAX);
    }
}
