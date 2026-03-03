use std::process::ExitCode;
use clap_complete::engine::CompletionCandidate;

/// Handle `ebdev completions [shell]`.
/// With shell arg: generate a completion script that uses `./ebdev`.
/// Without: show setup instructions.
pub fn handle_completions(shell: Option<&str>) -> anyhow::Result<ExitCode> {
    let Some(shell) = shell else {
        println!("Usage: ebdev completions <shell>");
        println!();
        println!("Generate shell completions that work with ./ebdev wrapper scripts.");
        println!();
        println!("  ebdev completions zsh    # add output to .zshrc");
        println!("  ebdev completions bash   # add output to .bashrc");
        println!("  ebdev completions fish   # add output to config.fish");
        return Ok(ExitCode::SUCCESS);
    };

    let exe = std::env::current_exe()?;
    let output = std::process::Command::new(&exe)
        .env("COMPLETE", shell)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to generate completions: {}", stderr);
    }

    let script = String::from_utf8(output.stdout)?;
    let exe_str = exe.to_string_lossy();

    // Replace the hardcoded absolute binary path with ./ebdev
    let script = script.replace(&*exe_str, "./ebdev");

    print!("{}", script);

    // For zsh: also register the completer for ./ebdev (clap only registers for "ebdev")
    if shell == "zsh" {
        println!("compdef _clap_dynamic_completer_ebdev ./ebdev");
    }

    Ok(ExitCode::SUCCESS)
}

/// Complete task names by running `ebdev tasks --json`.
/// Used with `ArgValueCandidates` (no args — clap filters by prefix).
pub fn complete_task_names() -> Vec<CompletionCandidate> {
    let Some(tasks) = run_tasks_json() else {
        return Vec::new();
    };
    tasks
        .iter()
        .map(|t| {
            let mut c = CompletionCandidate::new(&t.name);
            if let Some(desc) = &t.description {
                c = c.help(Some(desc.into()));
            }
            c
        })
        .collect()
}

/// Complete task args (after `--`) by finding the task name in the CLI tokens
/// and returning its declared args as `--cli-name` candidates.
/// Used with `ArgValueCompleter` (receives current prefix).
pub fn complete_task_args(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some(task_name) = find_task_name_in_args() else {
        return Vec::new();
    };
    let Some(tasks) = run_tasks_json() else {
        return Vec::new();
    };
    let Some(task) = tasks.iter().find(|t| t.name == task_name) else {
        return Vec::new();
    };
    let Some(args) = &task.args else {
        return Vec::new();
    };
    let prefix = current.to_str().unwrap_or("");

    // Detect if we're completing a flag value rather than a flag name.
    // Case 1: --flag=partial  (prefix contains '=')
    //   → return "--flag=value" candidates (replaces entire token)
    if let Some(eq_pos) = prefix.find('=') {
        let flag_part = &prefix[..eq_pos]; // e.g. "--target"
        let value_prefix = &prefix[eq_pos + 1..]; // e.g. "st"
        let cli_name = flag_part.strip_prefix("--").unwrap_or(flag_part);
        if let Some(arg) = args.iter().find(|a| a.cli_name == cli_name) {
            return complete_flag_values(&task_name, arg, Some(flag_part), value_prefix);
        }
        return Vec::new();
    }

    // Case 2: --flag <cursor>  (previous token is a non-boolean flag)
    //   → return bare "value" candidates (value is a separate token)
    if prefix.is_empty() || !prefix.starts_with('-') {
        if let Some(arg) = detect_prev_flag_needs_value(args) {
            return complete_flag_values(&task_name, &arg, None, prefix);
        }
    }

    // Default: complete flag names
    args.iter()
        .flat_map(|arg| {
            let flag = format!("--{}", arg.cli_name);
            if !flag.starts_with(prefix) {
                return Vec::new();
            }
            let mut candidates = vec![];
            let mut c = CompletionCandidate::new(&flag);
            c = c.help(Some(arg.description.clone().into()));
            candidates.push(c);
            if let Some(choices) = &arg.choices {
                for choice in choices {
                    let value = format!("{}={}", flag, choice);
                    if value.starts_with(prefix) {
                        let mut vc = CompletionCandidate::new(&value);
                        vc = vc.help(Some(format!("{} ({})", arg.description, choice).into()));
                        candidates.push(vc);
                    }
                }
            }
            candidates
        })
        .collect()
}

/// Check if the previous CLI token (before cursor) is a non-boolean flag
/// that expects a value. If so, return the matching ArgInfo.
fn detect_prev_flag_needs_value(args: &[ArgInfo]) -> Option<ArgInfo> {
    let cli_args: Vec<String> = std::env::args().collect();
    // Find tokens after "--"
    let after_dd: Vec<&str> = cli_args.iter()
        .skip(1)
        .skip_while(|a| *a != "--")
        .skip(1) // skip the "--" itself
        .map(|s| s.as_str())
        .collect();

    // The last token in after_dd is the current prefix (being completed).
    // The second-to-last is what we're interested in.
    if after_dd.len() < 2 {
        return None;
    }
    let prev = after_dd[after_dd.len() - 2];
    if !prev.starts_with("--") {
        return None;
    }
    let cli_name = prev.strip_prefix("--").unwrap_or(prev);
    args.iter()
        .find(|a| a.cli_name == cli_name && a.arg_type != "boolean")
        .cloned()
}

/// Collect completion values for a flag (static choices + dynamic completeFn).
///
/// `eq_flag`: If `Some("--name")`, we're in equals mode (`--name=<TAB>`)
///            and return `--name=value` candidates (full token replacement).
///            If `None`, we're in space mode (`--name <TAB>`)
///            and return bare `value` candidates.
fn complete_flag_values(task_name: &str, arg: &ArgInfo, eq_flag: Option<&str>, value_prefix: &str) -> Vec<CompletionCandidate> {
    let mut values: Vec<String> = Vec::new();

    // Static choices
    if let Some(choices) = &arg.choices {
        values.extend(choices.iter().cloned());
    }

    // Dynamic completions
    if arg.completable {
        if let Some(dynamic) = run_complete_arg(task_name, &arg.name) {
            for v in dynamic {
                if !values.contains(&v) {
                    values.push(v);
                }
            }
        }
    }

    values.iter()
        .filter(|v| v.starts_with(value_prefix))
        .map(|v| {
            let candidate_value = match eq_flag {
                Some(flag) => format!("{}={}", flag, v),
                None => v.clone(),
            };
            let mut c = CompletionCandidate::new(&candidate_value);
            c = c.help(Some(format!("{} ({})", arg.description, v).into()));
            c
        })
        .collect()
}

/// Run `ebdev complete-arg <task> <arg>` as subprocess and parse JSON result.
fn run_complete_arg(task_name: &str, arg_name: &str) -> Option<Vec<String>> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new(exe)
        .args(["complete-arg", task_name, arg_name])
        .env_remove("COMPLETE")
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Extract the task name from the current CLI args (env::args).
/// Looks for `task <name>` pattern before `--`.
fn find_task_name_in_args() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "task" {
            for next in iter.by_ref() {
                if next == "--" {
                    return None;
                }
                if !next.starts_with('-') {
                    return Some(next.clone());
                }
            }
            return None;
        }
    }
    None
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct TaskInfo {
    name: String,
    description: Option<String>,
    args: Option<Vec<ArgInfo>>,
    #[serde(rename = "pickedFlags", default)]
    picked_flags: Option<Vec<String>>,
}

#[derive(Clone, serde::Deserialize)]
struct ArgInfo {
    name: String,
    #[serde(rename = "cliName")]
    cli_name: String,
    #[serde(rename = "type", default)]
    arg_type: String,
    description: String,
    choices: Option<Vec<String>>,
    #[serde(default)]
    completable: bool,
}

/// Run `ebdev tasks --json` as a subprocess and parse the result.
/// Removes the COMPLETE env var so the subprocess doesn't enter completion mode.
fn run_tasks_json() -> Option<Vec<TaskInfo>> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new(exe)
        .args(["tasks", "--json"])
        .env_remove("COMPLETE")
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

// =============================================================================
// Flag Completions
// =============================================================================

#[derive(serde::Deserialize)]
struct FlagInfo {
    name: String,
    description: String,
    config: Option<Vec<FlagConfigFieldInfo>>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct FlagConfigFieldInfo {
    name: String,
    #[serde(rename = "cliName")]
    cli_name: String,
    description: String,
    choices: Option<Vec<String>>,
    #[serde(default)]
    completable: bool,
}

/// Complete flag names for `ebdev flag <TAB>`.
/// Returns flag names and flag.field variants.
pub fn complete_flag_names() -> Vec<CompletionCandidate> {
    let Some(flags) = run_flags_json() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for f in &flags {
        let mut c = CompletionCandidate::new(&f.name);
        c = c.help(Some(f.description.clone().into()));
        candidates.push(c);
        if let Some(config) = &f.config {
            for field in config {
                let dotted = format!("{}.{}", f.name, field.name);
                let mut fc = CompletionCandidate::new(&dotted);
                fc = fc.help(Some(field.description.clone().into()));
                candidates.push(fc);
            }
        }
    }
    candidates
}

/// Complete flag names for `--with`/`--without` on task command.
pub fn complete_with_flags() -> Vec<CompletionCandidate> {
    let Some(flags) = run_flags_json() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for f in &flags {
        let mut c = CompletionCandidate::new(&f.name);
        c = c.help(Some(f.description.clone().into()));
        candidates.push(c);
    }
    candidates
}

/// Complete flag values for `ebdev flag <name> <TAB>`.
/// Boolean flags → on/off, config field → static choices + dynamic completions.
pub fn complete_flag_value(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some(flag_name) = find_flag_name_in_args() else {
        return Vec::new();
    };
    let Some(flags) = run_flags_json() else {
        return Vec::new();
    };
    let prefix = current.to_str().unwrap_or("");

    if let Some(dot_pos) = flag_name.find('.') {
        // Config field: "search.engine" → choices + dynamic
        let base = &flag_name[..dot_pos];
        let field = &flag_name[dot_pos + 1..];
        let Some(flag) = flags.iter().find(|f| f.name == base) else {
            return Vec::new();
        };
        let Some(config) = &flag.config else {
            return Vec::new();
        };
        let Some(field_info) = config.iter().find(|f| f.name == field) else {
            return Vec::new();
        };
        let mut values: Vec<String> = Vec::new();
        if let Some(choices) = &field_info.choices {
            values.extend(choices.iter().cloned());
        }
        if field_info.completable {
            if let Some(dynamic) = run_complete_flag(base, field) {
                for v in dynamic {
                    if !values.contains(&v) {
                        values.push(v);
                    }
                }
            }
        }
        values.iter()
            .filter(|v| v.starts_with(prefix))
            .map(|v| {
                let mut c = CompletionCandidate::new(v.as_str());
                c = c.help(Some(field_info.description.clone().into()));
                c
            })
            .collect()
    } else {
        // Boolean or whole config flag → on/off
        ["on", "off"].iter()
            .filter(|v| v.starts_with(prefix))
            .map(|v| CompletionCandidate::new(*v))
            .collect()
    }
}

/// Extract the flag name from CLI args for value completion.
/// Looks for `flag <name>` pattern.
fn find_flag_name_in_args() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "flag" {
            for next in iter.by_ref() {
                if !next.starts_with('-') {
                    return Some(next.clone());
                }
            }
            return None;
        }
    }
    None
}

/// Run `ebdev complete-flag <flag> <field>` as subprocess and parse JSON result.
fn run_complete_flag(flag_name: &str, field_name: &str) -> Option<Vec<String>> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new(exe)
        .args(["complete-flag", flag_name, field_name])
        .env_remove("COMPLETE")
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Run `ebdev flags --json` as a subprocess and parse the result.
fn run_flags_json() -> Option<Vec<FlagInfo>> {
    let exe = std::env::current_exe().ok()?;
    let output = std::process::Command::new(exe)
        .args(["flags", "--json"])
        .env_remove("COMPLETE")
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}
