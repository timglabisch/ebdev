use std::path::PathBuf;
use clap::{Parser, Subcommand};
use ebdev_config::Config;
use ebdev_toolchain_node::NodeEnv;

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
        /// Command to run (e.g. node, npm, pnpm)
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

async fn ensure_toolchain(base_path: &PathBuf, node_version: &str, pnpm_version: Option<&str>) -> anyhow::Result<NodeEnv> {
    let env = match NodeEnv::new(base_path, node_version) {
        Ok(env) => env,
        Err(_) => {
            ebdev_toolchain_node::install_node(node_version, base_path).await?;
            NodeEnv::new(base_path, node_version)?
        }
    };

    if let Some(pnpm_v) = pnpm_version {
        if !env.pnpm_bin_dir(pnpm_v).exists() {
            env.install_pnpm(pnpm_v).await?;
        }
    }

    Ok(env)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let base_path = PathBuf::from(".");
    let config = Config::load_from_dir(&base_path)?;

    match cli.command {
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Install => {
                let node_version = &config.toolchain.node.version;
                let pnpm_version = config.toolchain.pnpm.as_ref().map(|p| p.version.as_str());

                ebdev_toolchain_node::install_node(node_version, &base_path).await?;

                if let Some(pnpm_v) = pnpm_version {
                    let env = NodeEnv::new(&base_path, node_version)?;
                    env.install_pnpm(pnpm_v).await?;
                }
            }
        },
        Commands::Run { node_version, pnpm_version, command, args } => {
            let node_v = node_version.as_deref()
                .unwrap_or(&config.toolchain.node.version);
            let pnpm_v = pnpm_version.as_deref()
                .or(config.toolchain.pnpm.as_ref().map(|p| p.version.as_str()));

            let env = ensure_toolchain(&base_path, node_v, pnpm_v).await?;
            let path = env.build_path(pnpm_v);

            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            env.run(&command, &args_ref, &path).await?;
        }
    }

    Ok(())
}
