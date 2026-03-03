mod cli;
mod completions;
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
        let config_path = PathBuf::from(".ebdev.ts");
        if !config_path.exists() {
            eprintln!("No .ebdev.ts found in current directory");
            return Ok(ExitCode::FAILURE);
        }

        let flags = ebdev_toolchain_deno::list_flags(&config_path).await?;

        // Read saved state
        let saved: serde_json::Value = std::fs::read_to_string(".ebdev/flags.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        if *json {
            #[derive(serde::Serialize)]
            struct FlagOutput {
                #[serde(flatten)]
                info: ebdev_toolchain_deno::FlagInfo,
                active: serde_json::Value,
            }
            let output: Vec<FlagOutput> = flags.into_iter().map(|f| {
                let active = if let Some(v) = saved.get(&f.name) {
                    v.clone()
                } else {
                    f.default.clone()
                };
                FlagOutput { info: f, active }
            }).collect();
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else if flags.is_empty() {
            println!("No feature flags defined in .ebdev.ts");
        } else {
            println!("Feature flags:\n");
            for f in &flags {
                let active = if let Some(v) = saved.get(&f.name) {
                    v.clone()
                } else {
                    f.default.clone()
                };
                let status = match &active {
                    serde_json::Value::Bool(true) => "ON".to_string(),
                    serde_json::Value::Bool(false) => "OFF".to_string(),
                    serde_json::Value::Object(_) => "ON".to_string(),
                    _ => "?".to_string(),
                };
                let default_marker = if saved.get(&f.name).is_none() { " (default)" } else { "" };
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
        return Ok(ExitCode::SUCCESS);
    }

    if let Commands::Flag { name, value } = &cli.command {
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
            (name.as_str(), None)
        };

        let flag_info = flags.iter().find(|f| f.name == flag_name);
        if flag_info.is_none() {
            eprintln!("Unknown flag '{}'. Available flags: {}", flag_name,
                flags.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
            return Ok(ExitCode::FAILURE);
        }
        let flag_info = flag_info.unwrap();

        // Load existing saved state
        let mut saved: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(".ebdev/flags.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if let Some(field) = field_name {
            // Setting a config field value
            let config_fields = match &flag_info.config {
                Some(c) => c,
                None => {
                    eprintln!("Flag '{}' is a boolean flag and has no config fields", flag_name);
                    return Ok(ExitCode::FAILURE);
                }
            };
            let field_info = config_fields.iter().find(|f| f.name == field || f.cli_name == field);
            if field_info.is_none() {
                eprintln!("Unknown field '{}' on flag '{}'. Available fields: {}",
                    field, flag_name,
                    config_fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
                return Ok(ExitCode::FAILURE);
            }
            let field_info = field_info.unwrap();

            let val = match value {
                Some(v) => v.clone(),
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
                    // Build default config object
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
            let val = match value.as_deref() {
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
                if flag_info.config.is_some() {
                    if flag_info.default == serde_json::Value::Bool(false) {
                        // Config flag with default=false: build config object from field defaults
                        let mut obj = serde_json::Map::new();
                        if let Some(fields) = &flag_info.config {
                            for cf in fields {
                                if let Some(d) = &cf.default {
                                    obj.insert(cf.name.clone(), d.clone());
                                }
                            }
                        }
                        saved.insert(flag_name.to_string(), serde_json::Value::Object(obj));
                    } else {
                        // Default is ON, remove from saved to use defaults
                        saved.remove(flag_name);
                    }
                } else {
                    saved.insert(flag_name.to_string(), serde_json::Value::Bool(true));
                }

                // Enable dependencies
                for dep in &flag_info.requires {
                    let dep_info = flags.iter().find(|f| f.name == *dep);
                    let dep_current = saved.get(dep.as_str())
                        .cloned()
                        .unwrap_or_else(|| dep_info.map(|d| d.default.clone()).unwrap_or(serde_json::Value::Bool(false)));
                    if dep_current == serde_json::Value::Bool(false) {
                        // Explicitly enable the dependency
                        if let Some(di) = dep_info {
                            if di.config.is_some() {
                                // Config flag: remove from saved to use defaults (ON with default config)
                                // but only if the default is truthy; otherwise explicitly set it
                                if di.default == serde_json::Value::Bool(false) {
                                    // Default is OFF, so we must explicitly build a config object
                                    let mut obj = serde_json::Map::new();
                                    if let Some(fields) = &di.config {
                                        for cf in fields {
                                            if let Some(d) = &cf.default {
                                                obj.insert(cf.name.clone(), d.clone());
                                            }
                                        }
                                    }
                                    saved.insert(dep.to_string(), serde_json::Value::Object(obj));
                                } else {
                                    saved.remove(dep.as_str());
                                }
                            } else {
                                // Boolean flag
                                if di.default == serde_json::Value::Bool(true) {
                                    saved.remove(dep.as_str()); // use default (ON)
                                } else {
                                    saved.insert(dep.to_string(), serde_json::Value::Bool(true));
                                }
                            }
                        } else {
                            saved.insert(dep.to_string(), serde_json::Value::Bool(true));
                        }
                        println!("  {} = ON (required by {})", dep, flag_name);
                    }
                }
            } else {
                saved.insert(flag_name.to_string(), serde_json::Value::Bool(false));

                // Disable dependents
                for f in &flags {
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

            println!("{} = {}", flag_name, if val { "ON" } else { "OFF" });
        }

        // Remove entries that match defaults (keep file minimal)
        let mut clean = serde_json::Map::new();
        for (k, v) in &saved {
            let flag = flags.iter().find(|f| f.name == *k);
            if let Some(flag) = flag {
                if v != &flag.default {
                    clean.insert(k.clone(), v.clone());
                }
            } else {
                clean.insert(k.clone(), v.clone());
            }
        }

        // Ensure .ebdev directory exists
        std::fs::create_dir_all(".ebdev")?;

        // Write flags.json
        let json = serde_json::to_string_pretty(&clean)?;
        std::fs::write(".ebdev/flags.json", json)?;

        return Ok(ExitCode::SUCCESS);
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
    let config = Config::load_from_dir(&base_path).await?;

    // Self-update check (skip if env var set to prevent loop)
    if std::env::var("EBDEV_SKIP_SELF_UPDATE").is_err() {
        let desired = &config.toolchain.ebdev.version;
        let current = option_env!("EBDEV_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        // Strip leading 'v' prefix for comparison (git describe produces "v0.0.5")
        let current = current.strip_prefix('v').unwrap_or(current);
        if desired != current {
            let new_binary = ebdev_toolchain_ebdev::self_update(desired, current).await
                .map_err(|e| anyhow::anyhow!(
                    "Self-update failed (have v{current}, want v{desired}): {e}"
                ))?;
            // Re-exec with same args
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
        Commands::Task { name, tui, debug_log, with_flags, without_flags, task_args } => {
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

            // Parse --with/--without into flag overrides
            let mut flag_overrides = std::collections::HashMap::new();
            for spec in &with_flags {
                parse_flag_override(spec, true, &mut flag_overrides);
            }
            for spec in &without_flags {
                parse_flag_override(spec, false, &mut flag_overrides);
            }

            if tui {
                return task::run_task_with_tui(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args, flag_overrides).await;
            } else {
                return task::run_task_headless(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args, flag_overrides).await;
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
