use crate::ui::types::{CompletedStage, FocusTarget, PinTarget, TaskInfo, truncate_string, MAX_STAGE_NAME_LEN, MAX_TASK_NAME_LEN};
use ratatui::layout::Margin;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

/// Draw task list with expandable completed stages and current tasks.
pub fn draw_task_list(
    frame: &mut Frame,
    area: Rect,
    tasks: &[TaskInfo],
    completed_stages: &[CompletedStage],
    current_stage: Option<&str>,
    focus: FocusTarget,
    pinned_task: Option<PinTarget>,
    scroll_offset: usize,
) {
    let mut lines: Vec<Line> = Vec::new();

    // Draw completed stages
    for (si, stage) in completed_stages.iter().enumerate() {
        let is_focused = focus == FocusTarget::CompletedStage(si);
        let (status_icon, status_style) = if stage.success {
            ("✓", Style::default().fg(Color::Green))
        } else {
            ("✗", Style::default().fg(Color::Red))
        };

        let chevron = if stage.expanded { "▼" } else { "▶" };
        let stage_name = truncate_string(&stage.name, MAX_STAGE_NAME_LEN);
        let task_info = if stage.failed_count > 0 {
            format!(" ({} tasks, {} failed, {:.1}s)", stage.task_count(), stage.failed_count, stage.total_duration.as_secs_f32())
        } else {
            format!(" ({} tasks, {:.1}s)", stage.task_count(), stage.total_duration.as_secs_f32())
        };

        let focus_prefix = if is_focused { "> " } else { "  " };
        let name_style = if is_focused {
            status_style.add_modifier(Modifier::BOLD)
        } else {
            status_style
        };

        lines.push(Line::from(vec![
            Span::styled(focus_prefix, name_style),
            Span::styled(format!("{} ", chevron), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", status_icon), status_style),
            Span::styled(stage_name, name_style),
            Span::styled(task_info, Style::default().fg(Color::DarkGray)),
        ]));

        // Expanded stage tasks
        if stage.expanded {
            for (ti, task) in stage.tasks.iter().enumerate() {
                let target = FocusTarget::CompletedTask { stage: si, task: ti };
                let pin = PinTarget::CompletedTask { stage: si, task: ti };
                let is_task_focused = focus == target;
                let is_pinned = pinned_task == Some(pin);
                let (icon, style) = task.icon_and_style();
                let duration_str = format!(" ({:.1}s)", task.duration().as_secs_f32());
                let prefix = if is_pinned {
                    "    ▸ "
                } else if is_task_focused {
                    "    > "
                } else {
                    "      "
                };
                let name = truncate_string(&task.name, MAX_TASK_NAME_LEN.saturating_sub(2));

                let line_style = if is_pinned || is_task_focused {
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
        }
    }

    // Draw current stage header if present
    if let Some(stage_name) = current_stage {
        if !completed_stages.is_empty() {
            lines.push(Line::from(""));
        }
        let header = format!("═══ {} ═══", stage_name);
        lines.push(Line::from(Span::styled(
            header,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    // Draw current tasks
    for (i, task) in tasks.iter().enumerate() {
        let target = FocusTarget::CurrentTask(i);
        let pin = PinTarget::CurrentTask(i);
        let is_focused = focus == target;
        let is_pinned = pinned_task == Some(pin);
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

    let content_height = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize; // borders
    let max_scroll = content_height.saturating_sub(visible_height);
    let scroll = scroll_offset.min(max_scroll);

    let task_list = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Tasks "))
        .scroll((scroll as u16, 0));
    frame.render_widget(task_list, area);

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
