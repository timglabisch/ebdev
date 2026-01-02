use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use clap::{Parser, Subcommand};
use ebdev_config::Config;
use ebdev_toolchain_node::NodeEnv;
use ebdev_toolchain_mutagen::MutagenEnv;

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
