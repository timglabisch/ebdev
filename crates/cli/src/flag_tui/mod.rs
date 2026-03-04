mod render;
mod state;

pub use state::{FlagTuiResult, TuiMode};

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crossterm::event::{self, KeyCode, KeyEventKind};
use crossterm::terminal;
use ebdev_toolchain_deno::FlagInfo;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::Value;

use render::{calculate_max_height, FlagTuiWidget};
use state::{EditState, FlagTuiState, FocusTarget};

pub struct FlagTui {
    state: FlagTuiState,
    completions: HashMap<(String, String), Vec<String>>,
}

impl FlagTui {
    pub fn new(
        flags: Vec<FlagInfo>,
        saved: &serde_json::Map<String, Value>,
        completions: HashMap<(String, String), Vec<String>>,
        mode: TuiMode,
    ) -> Self {
        let state = FlagTuiState::new(flags, saved, mode);
        FlagTui { state, completions }
    }

    pub fn apply_overrides(&mut self, overrides: &HashMap<String, Value>) {
        self.state.apply_overrides(overrides);
    }

    pub fn run(mut self) -> io::Result<FlagTuiResult> {
        if self.state.flags.is_empty() {
            eprintln!("No feature flags defined.");
            return Ok(FlagTuiResult::Cancelled);
        }

        terminal::enable_raw_mode()?;
        let backend = CrosstermBackend::new(io::stdout());
        let height = calculate_max_height(&self.state);
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;

        let result = loop {
            terminal.draw(|frame| {
                let area = frame.area();
                frame.render_widget(FlagTuiWidget { state: &self.state }, area);
            })?;

            if event::poll(Duration::from_millis(50))? {
                if let event::Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        if let Some(result) = self.handle_key(key.code)? {
                            break result;
                        }
                    }
                }
            }
        };

        terminal::disable_raw_mode()?;
        println!();

        Ok(result)
    }

    fn handle_key(&mut self, code: KeyCode) -> io::Result<Option<FlagTuiResult>> {
        // Edit mode key handling
        if self.state.edit.is_some() {
            return self.handle_edit_key(code);
        }

        // Normal mode key handling
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.move_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.move_down();
            }
            KeyCode::Char(' ') => {
                match self.state.focus {
                    FocusTarget::Flag(idx) => {
                        self.state.toggle_flag(idx);
                    }
                    FocusTarget::ConfigField { .. } => {
                        // Space on config field does nothing (use Enter to edit)
                    }
                }
            }
            KeyCode::Enter => {
                match self.state.focus {
                    FocusTarget::ConfigField { flag, field } => {
                        // Open edit mode
                        self.open_edit(flag, field);
                    }
                    FocusTarget::Flag(_) => {
                        // Confirm
                        return Ok(Some(self.confirm()));
                    }
                }
            }
            KeyCode::Tab => {
                // Toggle mode
                self.state.mode = match self.state.mode {
                    TuiMode::AdHoc { ref task_name } => {
                        let _name = task_name.clone();
                        TuiMode::Global
                    }
                    TuiMode::Global => {
                        // Can't switch to AdHoc from standalone Global mode
                        // just stay Global
                        TuiMode::Global
                    }
                };
                // Note: in AdHoc mode, Tab toggles to Global.
                // We need to store the original task name to toggle back.
                // For simplicity, Tab from AdHoc goes to Global (one-way for now).
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                return Ok(Some(FlagTuiResult::Cancelled));
            }
            _ => {}
        }

        Ok(None)
    }

    fn handle_edit_key(&mut self, code: KeyCode) -> io::Result<Option<FlagTuiResult>> {
        let edit = self.state.edit.as_mut().unwrap();

        match code {
            KeyCode::Esc => {
                // Cancel edit
                self.state.edit = None;
            }
            KeyCode::Enter => {
                // Confirm edit
                let flag_idx = edit.flag_idx;
                let field_idx = edit.field_idx;
                let completions = edit.filtered_completions();

                let value = if !completions.is_empty() && edit.selected < completions.len() {
                    completions[edit.selected].to_string()
                } else {
                    edit.input.clone()
                };

                // Apply value
                let field_name = self.state.flags[flag_idx]
                    .info
                    .config
                    .as_ref()
                    .and_then(|c| c.get(field_idx))
                    .map(|f| f.name.clone());

                if let Some(field_name) = field_name {
                    if let Some(config_values) = &mut self.state.flags[flag_idx].config_values {
                        config_values.insert(field_name, value);
                    }
                    self.state.flags[flag_idx].dirty = true;
                }

                self.state.edit = None;
            }
            KeyCode::Tab => {
                // Cycle through completions
                let count = edit.filtered_completions().len();
                if count > 0 {
                    edit.selected = (edit.selected + 1) % count;
                    let value = edit.filtered_completions()[edit.selected].to_string();
                    edit.input = value;
                    edit.cursor = edit.input.len();
                }
            }
            KeyCode::Up => {
                let count = edit.filtered_completions().len();
                if count > 0 {
                    if edit.selected == 0 {
                        edit.selected = count - 1;
                    } else {
                        edit.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                let count = edit.filtered_completions().len();
                if count > 0 {
                    edit.selected = (edit.selected + 1) % count;
                }
            }
            KeyCode::Backspace => {
                if edit.cursor > 0 {
                    edit.input.remove(edit.cursor - 1);
                    edit.cursor -= 1;
                    edit.selected = 0;
                }
            }
            KeyCode::Left => {
                if edit.cursor > 0 {
                    edit.cursor -= 1;
                }
            }
            KeyCode::Right => {
                if edit.cursor < edit.input.len() {
                    edit.cursor += 1;
                }
            }
            KeyCode::Char(c) => {
                edit.input.insert(edit.cursor, c);
                edit.cursor += 1;
                edit.selected = 0;
            }
            _ => {}
        }

        Ok(None)
    }

    fn open_edit(&mut self, flag_idx: usize, field_idx: usize) {
        let current_value = self.state.flags[flag_idx]
            .config_values
            .as_ref()
            .and_then(|cv| {
                let field_name = self.state.flags[flag_idx]
                    .info
                    .config
                    .as_ref()
                    .and_then(|c| c.get(field_idx))
                    .map(|f| &f.name);
                field_name.and_then(|n| cv.get(n))
            })
            .cloned()
            .unwrap_or_default();

        let flag_name = &self.state.flags[flag_idx].info.name;
        let field_name = self.state.flags[flag_idx]
            .info
            .config
            .as_ref()
            .and_then(|c| c.get(field_idx))
            .map(|f| f.name.clone())
            .unwrap_or_default();

        let all_completions = self
            .completions
            .get(&(flag_name.clone(), field_name))
            .cloned()
            .unwrap_or_default();

        self.state.edit = Some(EditState {
            flag_idx,
            field_idx,
            input: current_value,
            cursor: 0,
            all_completions,
            selected: 0,
        });

        // Place cursor at end
        if let Some(edit) = &mut self.state.edit {
            edit.cursor = edit.input.len();
        }
    }

    fn confirm(&self) -> FlagTuiResult {
        match &self.state.mode {
            TuiMode::AdHoc { .. } => FlagTuiResult::RunTask(self.state.build_overrides()),
            TuiMode::Global => {
                // Save to .ebdev/flags.json
                let saved = self.state.build_saved();
                if let Err(e) = save_flags_json(&saved) {
                    eprintln!("Error saving flags: {}", e);
                    return FlagTuiResult::Cancelled;
                }
                FlagTuiResult::SavedGlobal
            }
        }
    }
}

fn save_flags_json(saved: &serde_json::Map<String, Value>) -> io::Result<()> {
    std::fs::create_dir_all(".ebdev")?;
    let json = serde_json::to_string_pretty(saved)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    std::fs::write(".ebdev/flags.json", json)?;
    Ok(())
}
