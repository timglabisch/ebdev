use crate::command::FlagDisplay;
use crate::ui::types::truncate_string;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::cell::Cell;
use std::rc::Rc;

const NAME_LEN: usize = 20;
const DESC_LEN: usize = 50;

pub fn draw_flag_browser(
    frame: &mut Frame,
    area: Rect,
    flags: &[FlagDisplay],
    selected: usize,
    browser_area: &Rc<Cell<(Rect, usize)>>,
) {
    // Store area + flag count for mouse hit-testing
    browser_area.set((area, flags.len()));

    let lines: Vec<Line> = if flags.is_empty() {
        vec![Line::from(Span::styled(
            "  No flags defined",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        flags.iter().enumerate().map(|(i, flag)| {
            let is_selected = i == selected;
            let prefix = if is_selected { " > " } else { "   " };

            let (icon, icon_style) = if flag.enabled {
                ("●", Style::default().fg(Color::Green))
            } else {
                ("○", Style::default().fg(Color::DarkGray))
            };

            let name_style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let name = truncate_string(&flag.name, NAME_LEN);
            let desc = truncate_string(&flag.description, DESC_LEN);

            Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(icon, icon_style),
                Span::raw("  "),
                Span::styled(name, name_style),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(Color::DarkGray)),
            ])
        }).collect()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Flags ");

    let list = Paragraph::new(lines).block(block);
    frame.render_widget(list, area);
}

/// Toggle a flag at the given index, handling dependency cascades.
/// Returns false if idx is out of bounds.
pub fn toggle_flag(flags: &mut [FlagDisplay], idx: usize) -> bool {
    if idx >= flags.len() {
        return false;
    }

    let new_enabled = !flags[idx].enabled;

    if new_enabled {
        // Enable: also enable required flags
        flags[idx].enabled = true;
        let requires = flags[idx].requires.clone();
        for dep in &requires {
            if let Some(dep_flag) = flags.iter_mut().find(|f| f.name == *dep) {
                dep_flag.enabled = true;
            }
        }
    } else {
        // Disable: also disable flags that depend on this one
        let flag_name = flags[idx].name.clone();
        flags[idx].enabled = false;
        for f in flags.iter_mut() {
            if f.requires.contains(&flag_name) {
                f.enabled = false;
            }
        }
    }

    true
}

/// Save current flag state to `.ebdev/flags.json`.
/// Only non-default values are written; untouched flags preserve their original saved_value.
pub fn save_flags(flags: &[FlagDisplay]) {
    let mut map = serde_json::Map::new();
    for f in flags {
        if f.enabled != f.default_enabled {
            // Changed from default: write the new boolean value
            map.insert(f.name.clone(), serde_json::Value::Bool(f.enabled));
        } else if let Some(ref saved) = f.saved_value {
            // Unchanged but had a saved value (e.g. config object): preserve it
            map.insert(f.name.clone(), saved.clone());
        }
        // Otherwise: matches default, no entry needed
    }
    let _ = std::fs::create_dir_all(".ebdev");
    let _ = std::fs::write(
        ".ebdev/flags.json",
        serde_json::to_string_pretty(&map).unwrap_or_default(),
    );
}
