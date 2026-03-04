use std::collections::{BTreeMap, HashMap};

use ebdev_toolchain_deno::{FlagConfigField, FlagInfo};
use serde_json::Value;

/// Result of the TUI interaction
pub enum FlagTuiResult {
    /// Run task with these overrides (ad-hoc mode)
    RunTask(HashMap<String, Value>),
    /// Global config saved, no task start
    SavedGlobal,
    /// Cancelled by user
    Cancelled,
}

/// TUI operating mode
pub enum TuiMode {
    /// Overrides for this run only
    AdHoc { task_name: String },
    /// Persistent configuration
    Global,
}

/// A flag entry in the TUI list
pub struct FlagItem {
    pub info: FlagInfo,
    pub enabled: bool,
    pub config_values: Option<BTreeMap<String, String>>,
    pub expanded: bool,
    pub dirty: bool,
}

/// What is currently focused
pub enum FocusTarget {
    /// A flag row
    Flag(usize),
    /// A config field within a flag
    ConfigField { flag: usize, field: usize },
}

/// Edit state for a config field
pub struct EditState {
    pub flag_idx: usize,
    pub field_idx: usize,
    pub input: String,
    pub cursor: usize,
    pub all_completions: Vec<String>,
    pub selected: usize,
}

impl EditState {
    pub fn filtered_completions(&self) -> Vec<&str> {
        if self.input.is_empty() {
            self.all_completions.iter().map(|s| s.as_str()).collect()
        } else {
            let lower = self.input.to_lowercase();
            self.all_completions
                .iter()
                .filter(|c| c.to_lowercase().contains(&lower))
                .map(|s| s.as_str())
                .collect()
        }
    }
}

/// Full TUI state
pub struct FlagTuiState {
    pub mode: TuiMode,
    pub flags: Vec<FlagItem>,
    pub focus: FocusTarget,
    pub edit: Option<EditState>,
}

impl FlagTuiState {
    pub fn new(
        flags: Vec<FlagInfo>,
        saved: &serde_json::Map<String, Value>,
        mode: TuiMode,
    ) -> Self {
        let items: Vec<FlagItem> = flags
            .into_iter()
            .map(|info| {
                let active = saved.get(&info.name).cloned().unwrap_or(info.default.clone());
                let (enabled, config_values) = match &active {
                    Value::Bool(false) => (false, None),
                    Value::Object(obj) => {
                        let vals: BTreeMap<String, String> = obj
                            .iter()
                            .map(|(k, v)| {
                                let s = match v {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                (k.clone(), s)
                            })
                            .collect();
                        (true, Some(vals))
                    }
                    _ => (true, None),
                };
                let has_config = info.config.is_some();
                FlagItem {
                    info,
                    enabled,
                    config_values,
                    expanded: has_config && enabled,
                    dirty: false,
                }
            })
            .collect();

        FlagTuiState {
            mode,
            flags: items,
            focus: FocusTarget::Flag(0),
            edit: None,
        }
    }

    /// Toggle a flag and resolve dependencies.
    /// Returns info strings about cascading changes.
    pub fn toggle_flag(&mut self, idx: usize) -> Vec<String> {
        let mut messages = Vec::new();
        if idx >= self.flags.len() {
            return messages;
        }

        let was_enabled = self.flags[idx].enabled;
        self.flags[idx].enabled = !was_enabled;
        self.flags[idx].dirty = true;

        if !was_enabled {
            // Turning ON → expand config if applicable
            if self.flags[idx].info.config.is_some() {
                self.flags[idx].expanded = true;
                // Initialize config_values from defaults if not set
                if self.flags[idx].config_values.is_none() {
                    if let Some(config) = &self.flags[idx].info.config {
                        let vals: BTreeMap<String, String> = config
                            .iter()
                            .filter_map(|f| {
                                f.default.as_ref().map(|d| {
                                    let s = match d {
                                        Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    };
                                    (f.name.clone(), s)
                                })
                            })
                            .collect();
                        self.flags[idx].config_values = Some(vals);
                    }
                }
            }

            // Enable required dependencies
            let requires = self.flags[idx].info.requires.clone();
            for dep_name in &requires {
                if let Some(dep_idx) = self.flags.iter().position(|f| f.info.name == *dep_name) {
                    if !self.flags[dep_idx].enabled {
                        self.flags[dep_idx].enabled = true;
                        self.flags[dep_idx].dirty = true;
                        if self.flags[dep_idx].info.config.is_some() {
                            self.flags[dep_idx].expanded = true;
                            if self.flags[dep_idx].config_values.is_none() {
                                if let Some(config) = &self.flags[dep_idx].info.config {
                                    let vals: BTreeMap<String, String> = config
                                        .iter()
                                        .filter_map(|f| {
                                            f.default.as_ref().map(|d| {
                                                let s = match d {
                                                    Value::String(s) => s.clone(),
                                                    other => other.to_string(),
                                                };
                                                (f.name.clone(), s)
                                            })
                                        })
                                        .collect();
                                    self.flags[dep_idx].config_values = Some(vals);
                                }
                            }
                        }
                        messages.push(format!(
                            "{} = ON (required by {})",
                            dep_name, self.flags[idx].info.name
                        ));
                    }
                }
            }
        } else {
            // Turning OFF → collapse config
            self.flags[idx].expanded = false;

            // Disable dependents
            let flag_name = self.flags[idx].info.name.clone();
            for i in 0..self.flags.len() {
                if self.flags[i].info.requires.contains(&flag_name) && self.flags[i].enabled {
                    self.flags[i].enabled = false;
                    self.flags[i].expanded = false;
                    self.flags[i].dirty = true;
                    messages.push(format!(
                        "{} = OFF (requires {})",
                        self.flags[i].info.name, flag_name
                    ));
                }
            }
        }

        messages
    }

    /// Build overrides map from dirty flags (for ad-hoc mode)
    pub fn build_overrides(&self) -> HashMap<String, Value> {
        let mut overrides = HashMap::new();
        for flag in &self.flags {
            if !flag.dirty {
                continue;
            }
            if !flag.enabled {
                overrides.insert(flag.info.name.clone(), Value::Bool(false));
            } else if let Some(config_values) = &flag.config_values {
                let mut obj = serde_json::Map::new();
                for (k, v) in config_values {
                    obj.insert(k.clone(), Value::String(v.clone()));
                }
                overrides.insert(flag.info.name.clone(), Value::Object(obj));
            } else {
                overrides.insert(flag.info.name.clone(), Value::Bool(true));
            }
        }
        overrides
    }

    /// Build the full saved map for global mode (remove defaults)
    pub fn build_saved(&self) -> serde_json::Map<String, Value> {
        let mut saved = serde_json::Map::new();
        for flag in &self.flags {
            let value = if !flag.enabled {
                Value::Bool(false)
            } else if let Some(config_values) = &flag.config_values {
                let mut obj = serde_json::Map::new();
                for (k, v) in config_values {
                    obj.insert(k.clone(), Value::String(v.clone()));
                }
                Value::Object(obj)
            } else {
                Value::Bool(true)
            };

            // Only store if different from default
            if value != flag.info.default {
                saved.insert(flag.info.name.clone(), value);
            }
        }
        saved
    }

    /// Apply pre-existing overrides (from --with/--without flags)
    pub fn apply_overrides(&mut self, overrides: &HashMap<String, Value>) {
        for (name, value) in overrides {
            if let Some(idx) = self.flags.iter().position(|f| f.info.name == *name) {
                match value {
                    Value::Bool(false) => {
                        self.flags[idx].enabled = false;
                        self.flags[idx].expanded = false;
                        self.flags[idx].dirty = true;
                    }
                    Value::Bool(true) => {
                        self.flags[idx].enabled = true;
                        self.flags[idx].dirty = true;
                        if self.flags[idx].info.config.is_some() {
                            self.flags[idx].expanded = true;
                        }
                    }
                    Value::Object(obj) => {
                        self.flags[idx].enabled = true;
                        self.flags[idx].expanded = true;
                        self.flags[idx].dirty = true;
                        let vals: BTreeMap<String, String> = obj
                            .iter()
                            .map(|(k, v)| {
                                let s = match v {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                (k.clone(), s)
                            })
                            .collect();
                        // Merge with existing config values
                        if let Some(existing) = &mut self.flags[idx].config_values {
                            for (k, v) in vals {
                                existing.insert(k, v);
                            }
                        } else {
                            self.flags[idx].config_values = Some(vals);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Get the config fields for a flag, if any
    pub fn config_fields(&self, flag_idx: usize) -> &[FlagConfigField] {
        self.flags
            .get(flag_idx)
            .and_then(|f| f.info.config.as_deref())
            .unwrap_or(&[])
    }

    /// Move focus up
    pub fn move_up(&mut self) {
        match self.focus {
            FocusTarget::Flag(0) => {}
            FocusTarget::Flag(idx) => {
                // Check if previous flag has expanded config fields
                let prev_idx = idx - 1;
                if self.flags[prev_idx].expanded {
                    let field_count = self.config_fields(prev_idx).len();
                    if field_count > 0 {
                        self.focus = FocusTarget::ConfigField {
                            flag: prev_idx,
                            field: field_count - 1,
                        };
                        return;
                    }
                }
                self.focus = FocusTarget::Flag(prev_idx);
            }
            FocusTarget::ConfigField { flag, field: 0 } => {
                self.focus = FocusTarget::Flag(flag);
            }
            FocusTarget::ConfigField { flag, field } => {
                self.focus = FocusTarget::ConfigField {
                    flag,
                    field: field - 1,
                };
            }
        }
    }

    /// Move focus down
    pub fn move_down(&mut self) {
        match self.focus {
            FocusTarget::Flag(idx) => {
                if idx >= self.flags.len() {
                    return;
                }
                // If expanded, go to first config field
                if self.flags[idx].expanded {
                    let field_count = self.config_fields(idx).len();
                    if field_count > 0 {
                        self.focus = FocusTarget::ConfigField { flag: idx, field: 0 };
                        return;
                    }
                }
                // Otherwise go to next flag
                if idx + 1 < self.flags.len() {
                    self.focus = FocusTarget::Flag(idx + 1);
                }
            }
            FocusTarget::ConfigField { flag, field } => {
                let field_count = self.config_fields(flag).len();
                if field + 1 < field_count {
                    self.focus = FocusTarget::ConfigField {
                        flag,
                        field: field + 1,
                    };
                } else if flag + 1 < self.flags.len() {
                    self.focus = FocusTarget::Flag(flag + 1);
                }
            }
        }
    }
}
