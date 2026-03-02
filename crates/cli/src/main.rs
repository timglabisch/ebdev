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
        Commands::Task { name, tui, debug_log, task_args } => {
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

            if tui {
                return task::run_task_with_tui(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args).await;
            } else {
                return task::run_task_headless(&config_path, &name, &base_path, debug_log, mutagen_path, task_env, EMBEDDED_LINUX_BINARY, task_args).await;
            }
        }
        // Handled earlier before config load
        Commands::Completions { .. } => unreachable!(),
        Commands::RemoteBridge => unreachable!(),
        Commands::Remote { .. } => unreachable!(),
        Commands::CompleteArg { .. } => unreachable!(),
    }

    Ok(ExitCode::SUCCESS)
}
