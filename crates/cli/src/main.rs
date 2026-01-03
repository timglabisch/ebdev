use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use clap::{Parser, Subcommand};
use ebdev_config::Config;
use ebdev_toolchain_node::NodeEnv;
use ebdev_toolchain_mutagen::MutagenEnv;
use ebdev_mutagen_config::{discover_projects, SyncMode};

#[derive(Parser)]
#[command(name = "ebdev", version, about = "easybill development toolchain")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage toolchain installations
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Manage mutagen sync projects
    Mutagen {
        #[command(subcommand)]
        command: MutagenCommands,
    },
    /// Run a command with the configured toolchain environment
    #[command(disable_help_flag = true)]
    Run {
        /// Override node version from config
        #[arg(long)]
        node_version: Option<String>,
        /// Override pnpm version from config
        #[arg(long)]
        pnpm_version: Option<String>,
        /// Override mutagen version from config
        #[arg(long)]
        mutagen_version: Option<String>,
        /// Command to run (e.g. node, npm, pnpm, mutagen)
        command: String,
        /// Arguments passed to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Install all configured toolchains (node, pnpm)
    Install,
}

#[derive(Subcommand)]
enum MutagenCommands {
    /// Show discovered mutagen sync projects (debug)
    Debug,
    /// Start staged mutagen sync
    Sync {
        /// Terminate all sessions and run init stages (0..N-1)
        #[arg(long)]
        init: bool,
        /// Run the final sync stage
        #[arg(long)]
        sync: bool,
        /// Stay in watch mode after sync completes (requires --sync)
        #[arg(long)]
        keep_open: bool,
        /// Terminate all sessions for this project and exit
        #[arg(long)]
        terminate: bool,
    },
}

fn build_path(node_env: &NodeEnv, pnpm_version: Option<&str>, mutagen_env: Option<&MutagenEnv>) -> OsString {
    let mut paths: Vec<PathBuf> = Vec::new();

    // mutagen bin dir first (if configured)
    if let Some(env) = mutagen_env {
        paths.push(env.install_dir().to_path_buf());
    }

    // pnpm bin dir (if configured)
    if let Some(v) = pnpm_version {
        paths.push(node_env.pnpm_bin_dir(v));
    }

    // node bin dir
    paths.push(node_env.bin_dir());

    // existing PATH
    if let Some(existing) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&existing) {
            paths.push(path);
        }
    }

    std::env::join_paths(paths).unwrap_or_default()
}

async fn ensure_toolchain(
    base_path: &PathBuf,
    node_version: &str,
    pnpm_version: Option<&str>,
    mutagen_version: Option<&str>,
) -> anyhow::Result<(NodeEnv, Option<MutagenEnv>)> {
    let node_env = match NodeEnv::new(base_path, node_version) {
        Ok(env) => env,
        Err(_) => {
            ebdev_toolchain_node::install_node(node_version, base_path).await?;
            NodeEnv::new(base_path, node_version)?
        }
    };

    if let Some(pnpm_v) = pnpm_version {
        if !node_env.pnpm_bin_dir(pnpm_v).exists() {
            node_env.install_pnpm(pnpm_v).await?;
        }
    }

    let mutagen_env = if let Some(mutagen_v) = mutagen_version {
        let env = match MutagenEnv::new(base_path, mutagen_v) {
            Ok(env) => env,
            Err(_) => {
                ebdev_toolchain_mutagen::install_mutagen(mutagen_v, base_path).await?;
                MutagenEnv::new(base_path, mutagen_v)?
            }
        };
        Some(env)
    } else {
        None
    };

    Ok((node_env, mutagen_env))
}

#[tokio::main]
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
    let cli = Cli::parse();
    let base_path = PathBuf::from(".");
    let config = Config::load_from_dir(&base_path)?;

    match cli.command {
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Install => {
                let node_version = &config.toolchain.node.version;
                let pnpm_version = config.toolchain.pnpm.as_ref().map(|p| p.version.as_str());
                let mutagen_version = config.toolchain.mutagen.as_ref().map(|m| m.version.as_str());

                ebdev_toolchain_node::install_node(node_version, &base_path).await?;

                if let Some(pnpm_v) = pnpm_version {
                    let env = NodeEnv::new(&base_path, node_version)?;
                    env.install_pnpm(pnpm_v).await?;
                }

                if let Some(mutagen_v) = mutagen_version {
                    ebdev_toolchain_mutagen::install_mutagen(mutagen_v, &base_path).await?;
                }
            }
        },
        Commands::Mutagen { command } => match command {
            MutagenCommands::Debug => {
                let projects = discover_projects(&base_path)?;

                if projects.is_empty() {
                    println!("No mutagen sync projects found.");
                    return Ok(ExitCode::SUCCESS);
                }

                // Sort by stage
                let mut projects = projects;
                projects.sort_by_key(|p| p.project.stage);

                println!("Discovered {} mutagen sync project(s):\n", projects.len());

                for (i, p) in projects.iter().enumerate() {
                    println!("{}. {}", i + 1, p.project.name);
                    println!("   Config:    {}", p.config_path.display());
                    println!("   Directory: {}", p.resolved_directory.display());
                    println!("   Target:    {}", p.project.target);
                    println!("   Mode:      {}", match p.project.mode {
                        SyncMode::TwoWay => "two-way",
                        SyncMode::OneWayCreate => "one-way-create",
                        SyncMode::OneWayReplica => "one-way-replica",
                    });
                    println!("   Stage:     {}", p.project.stage);
                    if p.project.polling.enabled {
                        println!("   Polling:   enabled ({}s)", p.project.polling.interval);
                    }
                    if !p.project.ignore.is_empty() {
                        println!("   Ignore:    {}", p.project.ignore.join(", "));
                    }
                    println!();
                }
            }
            MutagenCommands::Sync { init, sync, keep_open, terminate } => {
                let mutagen_version = config.toolchain.mutagen
                    .as_ref()
                    .map(|m| m.version.as_str())
                    .ok_or_else(|| anyhow::anyhow!("No mutagen version configured in .ebdev.toml"))?;

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

                // Handle --terminate: terminate all mutagen sessions and exit
                if terminate {
                    println!("Terminating all mutagen sessions...");
                    ebdev_mutagen_runner::terminate_all_sessions(&mutagen_bin).await?;
                    println!("Done.");
                    return Ok(ExitCode::SUCCESS);
                }

                let projects = discover_projects(&base_path)?;

                // Default behavior: --sync only (just final stage)
                let (run_init, run_sync) = match (init, sync) {
                    (false, false) => (false, true),  // Default: only sync
                    (true, false) => (true, false),   // --init only
                    (false, true) => (false, true),   // --sync only
                    (true, true) => (true, true),     // --init --sync: both
                };

                // Terminate all sessions if --init flag is set
                if run_init {
                    println!("Terminating all mutagen sessions...");
                    ebdev_mutagen_runner::terminate_all_sessions(&mutagen_bin).await?;
                }

                let options = ebdev_mutagen_runner::SyncOptions {
                    run_init_stages: run_init,
                    run_final_stage: run_sync,
                    keep_open,
                };

                ebdev_mutagen_runner::run_staged_sync_with_options(
                    &mutagen_bin,
                    &base_path,
                    projects,
                    options,
                ).await?;
            }
        },
        Commands::Run { node_version, pnpm_version, mutagen_version, command, args } => {
            let node_v = node_version.as_deref()
                .unwrap_or(&config.toolchain.node.version);
            let pnpm_v = pnpm_version.as_deref()
                .or(config.toolchain.pnpm.as_ref().map(|p| p.version.as_str()));
            let mutagen_v = mutagen_version.as_deref()
                .or(config.toolchain.mutagen.as_ref().map(|m| m.version.as_str()));

            let (node_env, mutagen_env) = ensure_toolchain(&base_path, node_v, pnpm_v, mutagen_v).await?;
            let path = build_path(&node_env, pnpm_v, mutagen_env.as_ref());

            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            let status = node_env.run(&command, &args_ref, &path).await?;

            return Ok(ExitCode::from(status.code().unwrap_or(1) as u8));
        }
    }

    Ok(ExitCode::SUCCESS)
}
