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
struct TaskInfo {
    name: String,
    description: Option<String>,
    args: Option<Vec<ArgInfo>>,
}

#[derive(serde::Deserialize)]
struct ArgInfo {
    #[serde(rename = "cliName")]
    cli_name: String,
    description: String,
    choices: Option<Vec<String>>,
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
