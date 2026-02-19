use crate::ui::types::{CollapsedStage, TaskInfo, TaskState};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

pub fn draw_header(frame: &mut Frame, area: Rect, task_name: &str, tasks: &[TaskInfo], collapsed_stages: &[CollapsedStage]) {
    // Count from current tasks
    let completed = tasks.iter().filter(|t| t.state.is_success()).count();
    let failed = tasks.iter().filter(|t| t.state.is_failed()).count();
    let running = tasks.iter().filter(|t| matches!(t.state, TaskState::Running)).count();

    // Add counts from collapsed stages
    let collapsed_completed: usize = collapsed_stages.iter().map(|s| s.task_count - s.failed_count).sum();
    let collapsed_failed: usize = collapsed_stages.iter().map(|s| s.failed_count).sum();

    let total_completed = completed + collapsed_completed;
    let total_failed = failed + collapsed_failed;
    let total = tasks.len() + collapsed_stages.iter().map(|s| s.task_count).sum::<usize>();

    let status = if total_failed > 0 {
        format!(" Task: {} | {}/{} completed, {} failed, {} running ", task_name, total_completed, total, total_failed, running)
    } else {
        format!(" Task: {} | {}/{} completed, {} running ", task_name, total_completed, total, running)
    };

    let all_done = running == 0;
    let any_failed = total_failed > 0;
    let header_style = if any_failed {
        Style::default().fg(Color::Red)
    } else if all_done && !tasks.is_empty() {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Cyan)
    };

    let header = Paragraph::new(status)
        .style(header_style)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, area);
}
