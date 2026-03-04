use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ebdev_toolchain_deno::FlagInfo;

use crate::flag_tui;

/// Load saved flag state from `.ebdev/flags.json`.
/// Returns an empty map if the file doesn't exist or is invalid.
pub fn load_saved_flags() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(".ebdev/flags.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save flag state to `.ebdev/flags.json`, stripping entries that match defaults.
pub fn save_flags_json(
    saved: &serde_json::Map<String, serde_json::Value>,
    flags: &[FlagInfo],
) -> anyhow::Result<()> {
    let mut clean = serde_json::Map::new();
    for (k, v) in saved {
        let flag = flags.iter().find(|f| f.name == *k);
        if let Some(flag) = flag {
            if v != &flag.default {
                clean.insert(k.clone(), v.clone());
            }
        } else {
            clean.insert(k.clone(), v.clone());
        }
    }

    std::fs::create_dir_all(".ebdev")?;
    let json = serde_json::to_string_pretty(&clean)?;
    std::fs::write(".ebdev/flags.json", json)?;
    Ok(())
}

/// Pre-fetch dynamic completions for all completable config fields.
pub async fn prefetch_completions(
    config_path: &Path,
    flags: &[FlagInfo],
) -> std::collections::HashMap<(String, String), Vec<String>> {
    let mut map = std::collections::HashMap::new();
    for flag in flags {
        if let Some(config) = &flag.config {
            for field in config {
                let mut values: Vec<String> = field.choices.clone().unwrap_or_default();

                if field.completable {
                    if let Ok(dynamic) =
                        ebdev_toolchain_deno::complete_flag_value(config_path, &flag.name, &field.name).await
                    {
                        for v in dynamic {
                            if !values.contains(&v) {
                                values.push(v);
                            }
                        }
                    }
                }

                if !values.is_empty() {
                    map.insert((flag.name.clone(), field.name.clone()), values);
                }
            }
        }
    }
    map
}

/// Handle `ebdev flags` / `ebdev features` command.
pub async fn handle_flags(json: bool) -> anyhow::Result<ExitCode> {
    let config_path = PathBuf::from(".ebdev.ts");
    if !config_path.exists() {
        eprintln!("No .ebdev.ts found in current directory");
        return Ok(ExitCode::FAILURE);
    }

    let flags = ebdev_toolchain_deno::list_flags(&config_path).await?;
    let saved = load_saved_flags();

    // Interactive TUI by default (when not --json and stdout is a tty)
    if !json && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        let completions = prefetch_completions(&config_path, &flags).await;
        let tui = flag_tui::FlagTui::new(
            flags,
            &saved,
            completions,
            flag_tui::TuiMode::Global,
        );
        match tui.run()? {
            flag_tui::FlagTuiResult::SavedGlobal => {
                println!("Flags saved.");
            }
            flag_tui::FlagTuiResult::Cancelled => {}
            _ => {}
        }
        return Ok(ExitCode::SUCCESS);
    }

    let saved_value = serde_json::Value::Object(saved);

    if json {
        #[derive(serde::Serialize)]
        struct FlagOutput {
            #[serde(flatten)]
            info: FlagInfo,
            active: serde_json::Value,
        }
        let output: Vec<FlagOutput> = flags.into_iter().map(|f| {
            let active = saved_value.get(&f.name).cloned().unwrap_or(f.default.clone());
            FlagOutput { info: f, active }
        }).collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if flags.is_empty() {
        println!("No feature flags defined in .ebdev.ts");
    } else {
        println!("Feature flags:\n");
        for f in &flags {
            let active = saved_value.get(&f.name).cloned().unwrap_or(f.default.clone());
            let status = match &active {
                serde_json::Value::Bool(true) => "ON".to_string(),
                serde_json::Value::Bool(false) => "OFF".to_string(),
                serde_json::Value::Object(_) => "ON".to_string(),
                _ => "?".to_string(),
            };
            let default_marker = if saved_value.get(&f.name).is_none() { " (default)" } else { "" };
            println!("  {:20} {:4} {}{}", f.name, status, f.description, default_marker);

            if let Some(config_fields) = &f.config {
                for field in config_fields {
                    let field_val = active.as_object()
                        .and_then(|o| o.get(&field.name))
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_else(|| field.default.as_ref().map(|d| match d {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        }).unwrap_or_default());
                    let choices = field.choices.as_ref()
                        .map(|c| format!(" [{}]", c.join(", ")))
                        .unwrap_or_default();
                    println!("    .{:17} = {}{}", field.name, field_val, choices);
                }
            }

            if !f.requires.is_empty() {
                println!("    requires: {}", f.requires.join(", "));
            }
        }
        println!();
        println!("Set a flag:  ebdev flag <name> on/off");
        println!("Set config:  ebdev flag <name>.<field> <value>");
    }

    Ok(ExitCode::SUCCESS)
}

/// Handle `ebdev flag <name> [value]` command.
pub async fn handle_flag(name: &str, value: Option<&str>) -> anyhow::Result<ExitCode> {
    let config_path = PathBuf::from(".ebdev.ts");
    if !config_path.exists() {
        eprintln!("No .ebdev.ts found in current directory");
        return Ok(ExitCode::FAILURE);
    }

    let flags = ebdev_toolchain_deno::list_flags(&config_path).await?;

    // Parse name: "search" vs "search.engine"
    let (flag_name, field_name) = if let Some(dot) = name.find('.') {
        (&name[..dot], Some(&name[dot + 1..]))
    } else {
        (name, None)
    };

    let flag_info = match flags.iter().find(|f| f.name == flag_name) {
        Some(f) => f,
        None => {
            eprintln!("Unknown flag '{}'. Available flags: {}", flag_name,
                flags.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
            return Ok(ExitCode::FAILURE);
        }
    };

    let mut saved = load_saved_flags();

    if let Some(field) = field_name {
        // Setting a config field value
        let config_fields = match &flag_info.config {
            Some(c) => c,
            None => {
                eprintln!("Flag '{}' is a boolean flag and has no config fields", flag_name);
                return Ok(ExitCode::FAILURE);
            }
        };
        let field_info = match config_fields.iter().find(|f| f.name == field || f.cli_name == field) {
            Some(f) => f,
            None => {
                eprintln!("Unknown field '{}' on flag '{}'. Available fields: {}",
                    field, flag_name,
                    config_fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
                return Ok(ExitCode::FAILURE);
            }
        };

        let val = match value {
            Some(v) => v.to_string(),
            None => {
                eprintln!("Usage: ebdev flag {}.{} <value>", flag_name, field);
                return Ok(ExitCode::FAILURE);
            }
        };

        // Validate choices
        if let Some(choices) = &field_info.choices {
            if !choices.contains(&val) {
                eprintln!("Invalid value '{}'. Must be one of: {}", val, choices.join(", "));
                return Ok(ExitCode::FAILURE);
            }
        }

        // Get or create config object
        let config_obj = saved.entry(flag_name.to_string())
            .or_insert_with(|| {
                let mut obj = serde_json::Map::new();
                for cf in config_fields {
                    if let Some(d) = &cf.default {
                        obj.insert(cf.name.clone(), d.clone());
                    }
                }
                serde_json::Value::Object(obj)
            });

        if let serde_json::Value::Object(ref mut obj) = config_obj {
            let json_val: serde_json::Value = match field_info.field_type.as_str() {
                "number" => serde_json::Value::Number(val.parse::<serde_json::Number>()
                    .map_err(|_| anyhow::anyhow!("'{}' is not a valid number", val))?),
                _ => serde_json::Value::String(val.clone()),
            };
            obj.insert(field_info.name.clone(), json_val);
        }

        println!("{}.{} = {}", flag_name, field_info.name, val);
    } else {
        // Setting flag on/off
        let val = match value {
            Some("on") | Some("true") | Some("1") => true,
            Some("off") | Some("false") | Some("0") => false,
            Some(v) => {
                eprintln!("Invalid value '{}'. Use 'on' or 'off'", v);
                return Ok(ExitCode::FAILURE);
            }
            None => {
                // Toggle current state
                let current = saved.get(flag_name)
                    .cloned()
                    .unwrap_or(flag_info.default.clone());
                match current {
                    serde_json::Value::Bool(b) => !b,
                    serde_json::Value::Object(_) => false, // config flag ON → OFF
                    _ => true,
                }
            }
        };

        if val {
            enable_flag(flag_name, flag_info, &flags, &mut saved);
        } else {
            disable_flag(flag_name, &flags, &mut saved);
        }

        println!("{} = {}", flag_name, if val { "ON" } else { "OFF" });
    }

    save_flags_json(&saved, &flags)?;

    Ok(ExitCode::SUCCESS)
}

/// Enable a flag and its dependencies.
fn enable_flag(
    flag_name: &str,
    flag_info: &FlagInfo,
    all_flags: &[FlagInfo],
    saved: &mut serde_json::Map<String, serde_json::Value>,
) {
    if flag_info.config.is_some() {
        if flag_info.default == serde_json::Value::Bool(false) {
            saved.insert(flag_name.to_string(), build_default_config(flag_info));
        } else {
            saved.remove(flag_name);
        }
    } else {
        saved.insert(flag_name.to_string(), serde_json::Value::Bool(true));
    }

    // Enable dependencies
    for dep in &flag_info.requires {
        let dep_info = all_flags.iter().find(|f| f.name == *dep);
        let dep_current = saved.get(dep.as_str())
            .cloned()
            .unwrap_or_else(|| dep_info.map(|d| d.default.clone()).unwrap_or(serde_json::Value::Bool(false)));

        if dep_current == serde_json::Value::Bool(false) {
            if let Some(di) = dep_info {
                if di.config.is_some() {
                    if di.default == serde_json::Value::Bool(false) {
                        saved.insert(dep.to_string(), build_default_config(di));
                    } else {
                        saved.remove(dep.as_str());
                    }
                } else if di.default == serde_json::Value::Bool(true) {
                    saved.remove(dep.as_str());
                } else {
                    saved.insert(dep.to_string(), serde_json::Value::Bool(true));
                }
            } else {
                saved.insert(dep.to_string(), serde_json::Value::Bool(true));
            }
            println!("  {} = ON (required by {})", dep, flag_name);
        }
    }
}

/// Disable a flag and its dependents.
fn disable_flag(
    flag_name: &str,
    all_flags: &[FlagInfo],
    saved: &mut serde_json::Map<String, serde_json::Value>,
) {
    saved.insert(flag_name.to_string(), serde_json::Value::Bool(false));

    for f in all_flags {
        if f.requires.contains(&flag_name.to_string()) {
            let f_current = saved.get(&f.name)
                .cloned()
                .unwrap_or(f.default.clone());
            if f_current != serde_json::Value::Bool(false) {
                saved.insert(f.name.clone(), serde_json::Value::Bool(false));
                println!("  {} = OFF (requires {})", f.name, flag_name);
            }
        }
    }
}

/// Build a config object from field defaults for a flag.
fn build_default_config(flag_info: &FlagInfo) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if let Some(fields) = &flag_info.config {
        for cf in fields {
            if let Some(d) = &cf.default {
                obj.insert(cf.name.clone(), d.clone());
            }
        }
    }
    serde_json::Value::Object(obj)
}
