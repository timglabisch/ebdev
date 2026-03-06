use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Output,
    Tasks,
    Flags,
}

pub fn draw_tab_bar(
    frame: &mut Frame,
    area: Rect,
    active: ActiveTab,
    task_count: usize,
    flag_count: usize,
    tab_areas: &[Rc<Cell<Rect>>; 3],
) {
    let output_label = " Output ";
    let tasks_label = if task_count > 0 {
        format!(" Tasks ({}) ", task_count)
    } else {
        " Tasks ".to_string()
    };
    let flags_label = if flag_count > 0 {
        format!(" Flags ({}) ", flag_count)
    } else {
        " Flags ".to_string()
    };

    let active_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(Color::DarkGray);
    let disabled_style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM);
    let sep_style = Style::default().fg(Color::DarkGray);

    let output_style = if active == ActiveTab::Output { active_style } else { normal_style };
    let tasks_style = if active == ActiveTab::Tasks {
        active_style
    } else if task_count > 0 {
        normal_style
    } else {
        disabled_style
    };
    let flags_style = if active == ActiveTab::Flags {
        active_style
    } else if flag_count > 0 {
        normal_style
    } else {
        disabled_style
    };

    let spans = vec![
        Span::styled(output_label, output_style),
        Span::styled("│", sep_style),
        Span::styled(&tasks_label, tasks_style),
        Span::styled("│", sep_style),
        Span::styled(&flags_label, flags_style),
    ];

    // Store clickable areas for mouse hit-testing
    let x = area.x;
    tab_areas[0].set(Rect::new(x, area.y, output_label.len() as u16, 1));
    let tasks_x = x + output_label.len() as u16 + 1; // +1 for separator
    tab_areas[1].set(Rect::new(tasks_x, area.y, tasks_label.len() as u16, 1));
    let flags_x = tasks_x + tasks_label.len() as u16 + 1; // +1 for separator
    tab_areas[2].set(Rect::new(flags_x, area.y, flags_label.len() as u16, 1));

    let line = Paragraph::new(Line::from(spans));
    frame.render_widget(line, area);
}
