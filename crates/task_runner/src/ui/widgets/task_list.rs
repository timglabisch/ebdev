use crate::ui::types::{CollapsedStage, TaskInfo, truncate_string, MAX_STAGE_NAME_LEN, MAX_TASK_NAME_LEN};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draw task list. Returns the number of non-task lines at the top (for mouse click mapping).
pub fn draw_task_list(
    frame: &mut Frame,
    area: Rect,
    tasks: &[TaskInfo],
    collapsed_stages: &[CollapsedStage],
    current_stage: Option<&str>,
    focused_task: usize,
    pinned_task: Option<usize>,
) -> usize {
    let mut lines: Vec<Line> = Vec::new();
    let mut non_task_lines: usize = 0;

    // Draw collapsed stages first
    for stage in collapsed_stages {
        let (icon, style) = if stage.success {
            ("✓", Style::default().fg(Color::Green))
        } else {
            ("✗", Style::default().fg(Color::Red))
        };

        let stage_name = truncate_string(&stage.name, MAX_STAGE_NAME_LEN);
        let task_info = if stage.failed_count > 0 {
            format!(" ({} tasks, {} failed, {:.1}s)", stage.task_count, stage.failed_count, stage.total_duration.as_secs_f32())
        } else {
            format!(" ({} tasks, {:.1}s)", stage.task_count, stage.total_duration.as_secs_f32())
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), style),
            Span::styled(stage_name, style.add_modifier(Modifier::BOLD)),
            Span::styled(task_info, Style::default().fg(Color::DarkGray)),
        ]));
        non_task_lines += 1;
    }

    // Draw current stage header if present
    if let Some(stage_name) = current_stage {
        if !collapsed_stages.is_empty() {
            lines.push(Line::from(""));
            non_task_lines += 1;
        }
        let header = format!("═══ {} ═══", stage_name);
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        non_task_lines += 2;
    }

    // Draw current tasks
    for (i, task) in tasks.iter().enumerate() {
        let is_focused = i == focused_task;
        let is_pinned = pinned_task == Some(i);
        let (icon, style) = task.icon_and_style();
        let duration_str = format!(" ({:.1}s)", task.duration().as_secs_f32());
        let prefix = if is_pinned {
            "▸ "
        } else if is_focused {
            "> "
        } else {
            "  "
        };
        let name = truncate_string(&task.name, MAX_TASK_NAME_LEN);

        let line_style = if is_pinned || is_focused {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(format!("{} ", icon), style),
            Span::styled(name, line_style),
            Span::styled(duration_str, Style::default().fg(Color::DarkGray)),
        ]));
    }

    let task_list = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Tasks "));
    frame.render_widget(task_list, area);

    non_task_lines
}
