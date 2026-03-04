use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use super::state::{FlagTuiState, FocusTarget, TuiMode};

/// Widget that renders the entire flag TUI
pub struct FlagTuiWidget<'a> {
    pub state: &'a FlagTuiState,
}

impl<'a> Widget for FlagTuiWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let width = area.width as usize;
        let mut y = area.y;

        // Header line
        if y < area.y + area.height {
            let mode_label = match &self.state.mode {
                TuiMode::AdHoc { task_name } => format!("Ad-hoc: {}", task_name),
                TuiMode::Global => "Global".to_string(),
            };
            let title = "Feature Flags";
            let pad = width.saturating_sub(title.len() + mode_label.len());
            let header = Line::from(vec![
                Span::styled(
                    format!(" {}", title),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(pad.saturating_sub(2))),
                Span::styled(
                    format!("{} ", mode_label),
                    Style::default().fg(Color::Yellow),
                ),
            ]);
            buf.set_line(area.x, y, &header, area.width);
            y += 1;
        }

        // Separator
        if y < area.y + area.height {
            let sep = "─".repeat(width);
            buf.set_string(
                area.x,
                y,
                &sep,
                Style::default().fg(Color::DarkGray),
            );
            y += 1;
        }

        // Flag rows
        for (flag_idx, flag) in self.state.flags.iter().enumerate() {
            if y >= area.y + area.height {
                break;
            }

            let is_focused = matches!(self.state.focus, FocusTarget::Flag(i) if i == flag_idx);
            let marker = if flag.enabled { "●" } else { " " };
            let marker_style = if flag.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let name_style = if is_focused {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if flag.dirty {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let desc_style = Style::default().fg(Color::DarkGray);

            // Build line: " [●] name              description"
            let name_part = format!("{:20}", flag.info.name);
            let desc_part = &flag.info.description;

            let focus_indicator = if is_focused && self.state.edit.is_none() {
                ">"
            } else {
                " "
            };

            let line = Line::from(vec![
                Span::styled(format!(" {} ", focus_indicator), name_style),
                Span::raw("["),
                Span::styled(marker, marker_style),
                Span::raw("] "),
                Span::styled(name_part, name_style),
                Span::styled(desc_part.to_string(), desc_style),
            ]);
            buf.set_line(area.x, y, &line, area.width);
            y += 1;

            // Config fields (if expanded)
            if flag.expanded {
                if let Some(config) = &flag.info.config {
                    for (field_idx, field) in config.iter().enumerate() {
                        if y >= area.y + area.height {
                            break;
                        }

                        let is_field_focused = matches!(
                            self.state.focus,
                            FocusTarget::ConfigField { flag: fi, field: ffi }
                            if fi == flag_idx && ffi == field_idx
                        );

                        let is_editing = self.state.edit.as_ref().map_or(false, |e| {
                            e.flag_idx == flag_idx && e.field_idx == field_idx
                        });

                        let field_value = flag
                            .config_values
                            .as_ref()
                            .and_then(|cv| cv.get(&field.name))
                            .map(|s| s.as_str())
                            .unwrap_or("");

                        let field_focus = if is_field_focused && !is_editing {
                            ">"
                        } else {
                            " "
                        };

                        let field_name_style = if is_field_focused {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };

                        let value_style = if is_editing {
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::UNDERLINED)
                        } else if is_field_focused {
                            Style::default().fg(Color::White)
                        } else {
                            Style::default().fg(Color::Gray)
                        };

                        if is_editing {
                            let edit = self.state.edit.as_ref().unwrap();
                            // Show edit input with cursor
                            let (before_cursor, after_cursor) = edit.input.split_at(
                                edit.cursor.min(edit.input.len()),
                            );
                            let cursor_char = after_cursor.chars().next().unwrap_or(' ');
                            let rest = if after_cursor.len() > cursor_char.len_utf8() {
                                &after_cursor[cursor_char.len_utf8()..]
                            } else {
                                ""
                            };

                            let line = Line::from(vec![
                                Span::raw("       "),
                                Span::styled(
                                    format!("{:15}", field.name),
                                    field_name_style,
                                ),
                                Span::styled(before_cursor.to_string(), value_style),
                                Span::styled(
                                    cursor_char.to_string(),
                                    Style::default()
                                        .fg(Color::Black)
                                        .bg(Color::White),
                                ),
                                Span::styled(rest.to_string(), value_style),
                            ]);
                            buf.set_line(area.x, y, &line, area.width);
                            y += 1;

                            // Completion dropdown
                            let completions = edit.filtered_completions();
                            let max_show = 5.min(completions.len());
                            for (ci, comp) in completions.iter().take(max_show).enumerate() {
                                if y >= area.y + area.height {
                                    break;
                                }
                                let is_selected = ci == edit.selected;
                                let prefix = if is_selected { " → " } else { "   " };
                                let comp_style = if is_selected {
                                    Style::default()
                                        .fg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                };
                                let line = Line::from(vec![
                                    Span::raw(" ".repeat(22)),
                                    Span::styled(format!("{}{}", prefix, comp), comp_style),
                                ]);
                                buf.set_line(area.x, y, &line, area.width);
                                y += 1;
                            }
                        } else {
                            let line = Line::from(vec![
                                Span::styled(format!("   {}   ", field_focus), field_name_style),
                                Span::styled(
                                    format!("{:15}", field.name),
                                    field_name_style,
                                ),
                                Span::styled(field_value.to_string(), value_style),
                            ]);
                            buf.set_line(area.x, y, &line, area.width);
                            y += 1;
                        }
                    }
                }
            }
        }

        // Bottom separator
        if y < area.y + area.height {
            let sep = "─".repeat(width);
            buf.set_string(
                area.x,
                y,
                &sep,
                Style::default().fg(Color::DarkGray),
            );
            y += 1;
        }

        // Help line
        if y < area.y + area.height {
            let help = if self.state.edit.is_some() {
                Line::from(vec![
                    Span::styled(" Type", Style::default().fg(Color::DarkGray)),
                    Span::styled(" to filter  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Enter", Style::default().fg(Color::Gray)),
                    Span::styled(" confirm  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Tab", Style::default().fg(Color::Gray)),
                    Span::styled(" cycle  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Esc", Style::default().fg(Color::Gray)),
                    Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(" ↑↓", Style::default().fg(Color::Gray)),
                    Span::styled(" navigate  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Space", Style::default().fg(Color::Gray)),
                    Span::styled(" toggle  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Enter", Style::default().fg(Color::Gray)),
                    Span::styled(" edit/confirm  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Tab", Style::default().fg(Color::Gray)),
                    Span::styled(" mode  ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Esc", Style::default().fg(Color::Gray)),
                    Span::styled(" quit", Style::default().fg(Color::DarkGray)),
                ])
            };
            buf.set_line(area.x, y, &help, area.width);
        }
    }
}

/// Calculate the maximum height needed for the TUI
pub fn calculate_max_height(state: &FlagTuiState) -> u16 {
    let mut h: u16 = 4; // header + top separator + bottom separator + help
    for flag in &state.flags {
        h += 1; // flag row
        if let Some(config) = &flag.info.config {
            // Always account for expanded state in max height
            h += config.len() as u16;
            // Space for completions per field
            h += 5;
        }
    }
    // Cap at terminal height - 2
    let (_, term_h) = crossterm::terminal::size().unwrap_or((80, 24));
    h.min(term_h.saturating_sub(2))
}
