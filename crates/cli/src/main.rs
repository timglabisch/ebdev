mod cli;
mod completions;
mod flag_tui;
mod flags;
mod remote;
mod task;
mod toolchain;

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;
use clap::{CommandFactory, Parser};
use ebdev_config::Config;
use ebdev_toolchain_node::NodeEnv;
use ebdev_toolchain_mutagen::MutagenEnv;

use cli::{Cli, Commands, MutagenCommands, ToolchainCommands};
use toolchain::{build_path, ensure_toolchain};

/// Embedded Linux bridge binary (built via `make build-linux`)
/// Empty if binary wasn't available at compile time
const EMBEDDED_LINUX_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ebdev-bridge-linux"));

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<ExitCode> {
    // Ensure our own binary is in PATH so child processes can find `ebdev`
    // (e.g. when running ./bin/ebdev or from a non-standard location)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let mut paths = vec![exe_dir.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(new_path) = std::env::join_paths(&paths) {
                std::env::set_var("PATH", &new_path);
            }
        }
    }

    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();

    // Completions braucht keine Config
    if let Commands::Completions { shell } = &cli.command {
        return completions::handle_completions(shell.as_deref());
    }

    // CompleteArg braucht keine Config — direkt .ebdev.ts laden
    if let Commands::CompleteArg { task, arg } = &cli.command {
        let config_path = PathBuf::from(".ebdev.ts");
        if config_path.exists() {
            match ebdev_toolchain_deno::complete_arg(&config_path, task, arg).await {
                Ok(values) => {
                    println!("{}", serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string()));
                }
                Err(_) => {
                    println!("[]");
                }
            }
        } else {
            println!("[]");
        }
        return Ok(ExitCode::SUCCESS);
    }

    // CompleteFlag braucht keine Config — direkt .ebdev.ts laden
    if let Commands::CompleteFlag { flag, field } = &cli.command {
        let config_path = PathBuf::from(".ebdev.ts");
        if config_path.exists() {
            match ebdev_toolchain_deno::complete_flag_value(&config_path, flag, field).await {
                Ok(values) => {
                    println!("{}", serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string()));
                }
                Err(_) => {
                    println!("[]");
                }
            }
        } else {
            println!("[]");
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Flags/Flag commands brauchen keine Config — direkt .ebdev.ts laden
    if let Commands::Flags { json } = &cli.command {
        return flags::handle_flags(*json).await;
    }

    if let Commands::Flag { name, value } = &cli.command {
        return flags::handle_flag(name, value.as_deref()).await;
    }

    // RemoteBridge und Remote brauchen keine Config - direkt ausführen
    if matches!(cli.command, Commands::RemoteBridge) {
        if let Err(e) = ebdev_remote::run_bridge().await {
            eprintln!("Remote bridge error: {}", e);
            return Ok(ExitCode::FAILURE);
        }
        return Ok(ExitCode::SUCCESS);
    }

    if let Commands::Remote { command } = cli.command {
        return remote::handle_remote_command(command, EMBEDDED_LINUX_BINARY).await;
    }

    let base_path = PathBuf::from(".");

    // Self-update: try early version extraction from raw source text first.
    // This handles the case where a newer .ebdev.ts uses API features that
    // the current binary's embedded runtime doesn't support. By extracting
    // just the version string without evaluating TypeScript, we can still
    // self-update to the correct version, which then loads the config properly.
    if std::env::var("EBDEV_SKIP_SELF_UPDATE").is_err() {
        let current = option_env!("EBDEV_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let current = current.strip_prefix('v').unwrap_or(current);

        let ts_path = base_path.join(".ebdev.ts");
        if let Ok(source) = std::fs::read_to_string(&ts_path) {
            if let Some(desired) = ebdev_config::extract_ebdev_version(&source) {
                if desired != current {
                    self_update_and_reexec(&desired, current).await?;
                }
            }
        }
    }

    let config = Config::load_from_dir(&base_path).await?;

    // Fallback self-update check with fully evaluated config.
    // Catches cases where the regex extraction missed the version
    // (e.g., version stored in a variable or computed dynamically).
    if std::env::var("EBDEV_SKIP_SELF_UPDATE").is_err() {
        let desired = &config.toolchain.ebdev.version;
        let current = option_env!("EBDEV_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let current = current.strip_prefix('v').unwrap_or(current);
        if desired != current {
            self_update_and_reexec(desired, current).await?;
        }
    }

    // From here on, all child processes should skip self-update
    // (the parent already handled it above)
    std::env::set_var("EBDEV_SKIP_SELF_UPDATE", "1");

    match cli.command {
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Info => {
                println!("Config: .ebdev.ts");
                println!();
                println!("Toolchain:");
                println!("  ebdev:   {}", config.toolchain.ebdev.version);
                println!("  Node:    {}", config.toolchain.node.version);
                if let Some(pnpm) = &config.toolchain.pnpm {
                    println!("  pnpm:    {}", pnpm.version);
                }
                if let Some(mutagen) = &config.toolchain.mutagen {
                    println!("  Mutagen: {}", mutagen.version);
                }
                if let Some(rust) = &config.toolchain.rust {
                    println!("  Rust:    {}", rust.version);
                }
                if let Some(gh) = &config.toolchain.gh {
                    println!("  gh:      {}", gh.version);
                }
                if let Some(binaries) = &config.toolchain.binary {
                    for (name, cfg) in binaries {
                        println!("  {name}: {} (binary)", cfg.version);
                    }
                }
            }
            ToolchainCommands::Install => {
                let tc = &config.toolchain;

                ebdev_toolchain_node::install_node(&tc.node.version, &base_path).await?;

                if let Some(pnpm) = &tc.pnpm {
                    let env = NodeEnv::new(&base_path, &tc.node.version)?;
                    env.install_pnpm(&pnpm.version).await?;
                }

                if let Some(mutagen) = &tc.mutagen {
                    ebdev_toolchain_mutagen::install_mutagen(&mutagen.version, &base_path).await?;
                }

                if let Some(rust) = &tc.rust {
                    ebdev_toolchain_rust::install_rust(&rust.version, &base_path).await?;
                }

                if let Some(gh) = &tc.gh {
                    ebdev_toolchain_binary::install_gh(&gh.version, &base_path).await?;
                }

                let gh_version = tc.gh.as_ref().map(|g| g.version.as_str());
                if let Some(binaries) = &tc.binary {
                    for (name, cfg) in binaries {
                        ebdev_toolchain_binary::install_binary(
                            &ebdev_toolchain_binary::InstallBinaryOptions {
                                name,
                                version: &cfg.version,
                                url_template: &cfg.url,
                                binary_path: cfg.binary.as_deref(),
                                base_path: &base_path,
                                gh_version,
                            },
                        ).await?;
                    }
                }
            }
        },
        Commands::Mutagen { command } => {
            let mutagen_version = config.toolchain.mutagen
                .as_ref()
                .map(|m| m.version.as_str())
                .ok_or_else(|| anyhow::anyhow!("No mutagen version configured in toolchain config"))?;

            // Ensure mutagen is installed
            let mutagen_env = match MutagenEnv::new(&base_path, mutagen_version) {
                Ok(env) => env,
                Err(_) => {
                    println!("Installing mutagen {}...", mutagen_version);
                    ebdev_toolchain_mutagen::install_mutagen(mutagen_version, &base_path).await?;
                    MutagenEnv::new(&base_path, mutagen_version)?
                }
            };

            let mutagen_bin = mutagen_env.bin_path();

            // Compute project CRC32 from config path
            let config_path = base_path.canonicalize()?.join(".ebdev.ts");
            let project_crc32 = crc32fast::hash(config_path.to_string_lossy().as_bytes());
            let project_suffix = format!("{:08x}", project_crc32);

            match command {
                MutagenCommands::Status => {
                    // List all mutagen sessions, highlight ones belonging to this project
                    let output = tokio::process::Command::new(&mutagen_bin)
                        .args(["sync", "list"])
                        .output()
                        .await?;

                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if stderr.contains("no sessions") {
                            println!("No mutagen sessions.");
                            return Ok(ExitCode::SUCCESS);
                        }
                        eprintln!("Failed to list sessions: {}", stderr);
                        return Ok(ExitCode::FAILURE);
                    }

                    let stdout = String::from_utf8_lossy(&output.stdout);
                    println!("Project CRC: {}\n", project_suffix);

                    // Show raw output with project sessions marked
                    for line in stdout.lines() {
                        if line.contains(&project_suffix) {
                            println!("► {}", line);
                        } else {
                            println!("  {}", line);
                        }
                    }
                }
                MutagenCommands::Terminate => {
                    println!("Terminating sessions for project {}...", project_suffix);

                    // Get session list as JSON to find sessions for this project
                    let output = tokio::process::Command::new(&mutagen_bin)
                        .args(["sync", "list", "--template", "{{json .}}"])
                        .output()
                        .await?;

                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if let Ok(sessions) = serde_json::from_str::<Vec<serde_json::Value>>(&stdout) {
                            let mut terminated = 0;
                            for session in sessions {
                                if let (Some(name), Some(id)) = (
                                    session.get("name").and_then(|v| v.as_str()),
                                    session.get("identifier").and_then(|v| v.as_str()),
                                ) {
                                    if name.ends_with(&project_suffix) {
                                        let _ = tokio::process::Command::new(&mutagen_bin)
                                            .args(["sync", "terminate", id])
                                            .output()
                                            .await;
                                        println!("  Terminated: {}", name);
                                        terminated += 1;
                                    }
                                }
                            }
                            println!("\nTerminated {} session(s).", terminated);
                        }
                    } else {
                        println!("No sessions to terminate.");
                    }
                }
            }
        },
        Commands::Run { node_version, pnpm_version, mutagen_version, rust_version, command, args } => {
            let mut tc = config.toolchain.clone();
            if let Some(v) = node_version {
                tc.node.version = v;
            }
            if let Some(v) = pnpm_version {
                tc.pnpm = Some(ebdev_config::PnpmConfig { version: v });
            }
            if let Some(v) = mutagen_version {
                tc.mutagen = Some(ebdev_config::MutagenConfig { version: v });
            }
            if let Some(v) = rust_version {
                tc.rust = Some(ebdev_config::RustConfig { version: v });
            }

            let (node_env, mutagen_env, rust_env, binary_envs) = ensure_toolchain(&base_path, &tc).await?;
            let pnpm_v = tc.pnpm.as_ref().map(|p| p.version.as_str());
            let path = build_path(&node_env, pnpm_v, mutagen_env.as_ref(), rust_env.as_ref(), &binary_envs);

            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            let mut cmd = tokio::process::Command::new(&command);
            cmd.args(&args_ref)
                .env("PATH", &path)
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit());

            if let Some(ref env) = rust_env {
                cmd.env("RUSTUP_HOME", env.rustup_home());
                cmd.env("CARGO_HOME", env.cargo_home());
                cmd.env("RUSTUP_TOOLCHAIN", env.version());
            }

            let status = cmd.status().await
                .map_err(|e| {
                    let hint = if e.kind() == std::io::ErrorKind::NotFound {
                        format!(
                            "\n  hint: '{}' was not found in PATH. Did you mean 'ebdev task {}'?",
                            command, command
                        )
                    } else {
                        String::new()
                    };
                    anyhow::anyhow!("Failed to run '{}': {}{}", command, e, hint)
                })?;

            return Ok(ExitCode::from(status.code().unwrap_or(1) as u8));
        }
        Commands::Tasks { json } => {
            let config_path = base_path.join(".ebdev.ts");
            if !config_path.exists() {
                eprintln!("No .ebdev.ts found in current directory");
                return Ok(ExitCode::FAILURE);
            }

            let tasks = ebdev_toolchain_deno::list_tasks(&config_path).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else if tasks.is_empty() {
                println!("No tasks found in .ebdev.ts");
                println!();
                println!("Define tasks as exported async functions:");
                println!();
                println!("  export async function build() {{");
                println!("    await exec([\"npm\", \"run\", \"build\"]);");
                println!("  }}");
            } else {
                println!("Available tasks:\n");
                for t in &tasks {
                    if let Some(desc) = &t.description {
                        println!("  {:20} {}", t.name, desc);
                    } else {
                        println!("  {}", t.name);
                    }
                }
                println!();
                println!("Run a task with: ebdev task <name>");
            }
        }
        Commands::Task { name, no_tui, debug_log, with_flags, without_flags, task_args } => {
            let config_path = base_path.join(".ebdev.ts");
            if !config_path.exists() {
                eprintln!("No .ebdev.ts found in current directory");
                return Ok(ExitCode::FAILURE);
            }

            let (node_env, mutagen_env, rust_env, binary_envs) =
                ensure_toolchain(&base_path, &config.toolchain).await?;

            let pnpm_v = config.toolchain.pnpm.as_ref().map(|p| p.version.as_str());
            let path = build_path(&node_env, pnpm_v, mutagen_env.as_ref(), rust_env.as_ref(), &binary_envs);

            let mutagen_path = mutagen_env.map(|e| e.bin_path());

            let mut task_env = std::collections::HashMap::new();
            task_env.insert("PATH".to_string(), path.to_string_lossy().to_string());

            if let Some(ref env) = rust_env {
                task_env.insert("RUSTUP_HOME".to_string(), env.rustup_home().to_string_lossy().to_string());
                task_env.insert("CARGO_HOME".to_string(), env.cargo_home().to_string_lossy().to_string());
                task_env.insert("RUSTUP_TOOLCHAIN".to_string(), env.version().to_string());
            }

            // Detect bare --with (no values) → interactive flag selection
            let with_interactive = with_flags.is_empty() && {
                let args_raw: Vec<String> = std::env::args().collect();
                let sep = args_raw.iter().position(|a| a == "--");
                args_raw[..sep.unwrap_or(args_raw.len())]
                    .iter()
                    .any(|a| a == "--with")
            };

            // Parse --with/--without into flag overrides
            let mut flag_overrides = std::collections::HashMap::new();
            for spec in &with_flags {
                parse_flag_override(spec, true, &mut flag_overrides);
            }
            for spec in &without_flags {
                parse_flag_override(spec, false, &mut flag_overrides);
            }

            if with_interactive {
                let flags_meta = ebdev_toolchain_deno::list_flags(&config_path).await?;
                let saved = flags::load_saved_flags();
                let completions = flags::prefetch_completions(&config_path, &flags_meta).await;

                let mut tui_instance = flag_tui::FlagTui::new(
                    flags_meta,
                    &saved,
                    completions,
                    flag_tui::TuiMode::AdHoc { task_name: name.clone() },
                );
                tui_instance.apply_overrides(&flag_overrides);

                match tui_instance.run()? {
                    flag_tui::FlagTuiResult::RunTask(overrides) => {
                        flag_overrides = overrides;
                    }
                    flag_tui::FlagTuiResult::SavedGlobal => {
                        println!("Flags saved globally.");
                        return Ok(ExitCode::SUCCESS);
                    }
                    flag_tui::FlagTuiResult::Cancelled => {
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            }

            if no_tui {
                return task::run_task_headless(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args, flag_overrides).await;
            } else {
                return task::run_task_with_tui(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args, flag_overrides).await;
            }
        }
        // Handled earlier before config load
        Commands::Completions { .. } => unreachable!(),
        Commands::RemoteBridge => unreachable!(),
        Commands::Remote { .. } => unreachable!(),
        Commands::CompleteArg { .. } => unreachable!(),
        Commands::CompleteFlag { .. } => unreachable!(),
        Commands::Flags { .. } => unreachable!(),
        Commands::Flag { .. } => unreachable!(),
    }

    Ok(ExitCode::SUCCESS)
}

/// Parse a --with/--without flag spec into overrides.
///
/// Supported formats:
/// - "search"                              → search = true/false
/// - "search.engine=meilisearch"           → search = { engine: "meilisearch" }
/// - "search:engine=meilisearch,index=products" → search = { engine: "meilisearch", index: "products" }
fn parse_flag_override(spec: &str, enable: bool, overrides: &mut std::collections::HashMap<String, serde_json::Value>) {
    if !enable {
        // --without always disables the flag
        let flag_name = spec.split(&['.', ':'][..]).next().unwrap_or(spec);
        overrides.insert(flag_name.to_string(), serde_json::Value::Bool(false));
        return;
    }

    // Check for colon syntax: flag:key=val,key=val
    if let Some(colon) = spec.find(':') {
        let flag_name = &spec[..colon];
        let fields_str = &spec[colon + 1..];
        let mut obj = match overrides.get(flag_name) {
            Some(serde_json::Value::Object(existing)) => existing.clone(),
            _ => serde_json::Map::new(),
        };
        for pair in fields_str.split(',') {
            if let Some(eq) = pair.find('=') {
                let k = &pair[..eq];
                let v = &pair[eq + 1..];
                obj.insert(k.to_string(), serde_json::Value::String(v.to_string()));
            }
        }
        overrides.insert(flag_name.to_string(), serde_json::Value::Object(obj));
        return;
    }

    // Check for dot syntax: flag.field=value
    if let Some(dot) = spec.find('.') {
        let flag_name = &spec[..dot];
        let rest = &spec[dot + 1..];
        if let Some(eq) = rest.find('=') {
            let field = &rest[..eq];
            let value = &rest[eq + 1..];
            let mut obj = match overrides.get(flag_name) {
                Some(serde_json::Value::Object(existing)) => existing.clone(),
                _ => serde_json::Map::new(),
            };
            obj.insert(field.to_string(), serde_json::Value::String(value.to_string()));
            overrides.insert(flag_name.to_string(), serde_json::Value::Object(obj));
        }
        return;
    }

    // Simple flag name → enable
    overrides.insert(spec.to_string(), serde_json::Value::Bool(true));
}

/// Download a new ebdev binary and re-exec with the same arguments.
/// This function does not return on success (exec replaces the process).
async fn self_update_and_reexec(desired: &str, current: &str) -> anyhow::Result<()> {
    let new_binary = ebdev_toolchain_ebdev::self_update(desired, current).await
        .map_err(|e| anyhow::anyhow!(
            "Self-update failed (have v{current}, want v{desired}): {e}"
        ))?;
    let args: Vec<String> = std::env::args().collect();
    eprintln!("ebdev self-update: re-executing as v{desired} ...");
    eprintln!("  {} {}", new_binary.display(), args[1..].join(" "));
    eprintln!("---");
    let err = std::process::Command::new(&new_binary)
        .args(&args[1..])
        .env("EBDEV_SKIP_SELF_UPDATE", "1")
        .exec();
    // exec() only returns on error
    anyhow::bail!(
        "Failed to re-exec updated binary at {}: {}\n  args: {:?}",
        new_binary.display(),
        err,
        &args[1..]
    );
}

