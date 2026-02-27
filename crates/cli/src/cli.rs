use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ebdev", version = option_env!("EBDEV_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")), about = "Development environment tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
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
        /// Override rust version from config
        #[arg(long)]
        rust_version: Option<String>,
        /// Command to run (e.g. node, npm, pnpm, mutagen)
        command: String,
        /// Arguments passed to the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run a task defined in .ebdev.ts
    Task {
        /// Task name to run
        name: String,
        /// Run with TUI visualization
        #[arg(long)]
        tui: bool,
        /// Log all executor communication to file (JSON format)
        #[arg(long)]
        debug_log: Option<std::path::PathBuf>,
    },
    /// List all available tasks from .ebdev.ts
    Tasks,
    /// Run commands in Docker containers via bridge
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    /// Internal: Run as remote bridge inside a container (used by remote run)
    #[command(hide = true)]
    RemoteBridge,
}

#[derive(Subcommand)]
pub enum RemoteCommands {
    /// Run a command inside a Docker container
    Run {
        /// Docker container name or ID
        container: String,
        /// Command to run
        command: String,
        /// Arguments for the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        /// Working directory inside the container
        #[arg(long, short = 'w')]
        workdir: Option<String>,
        /// Run in interactive mode with PTY (for vim, htop, etc.)
        #[arg(long, short = 'i')]
        interactive: bool,
    },
}

#[derive(Subcommand)]
pub enum ToolchainCommands {
    /// Install all configured toolchains (node, pnpm)
    Install,
    /// Show loaded configuration info
    Info,
}

#[derive(Subcommand)]
pub enum MutagenCommands {
    /// Show current mutagen sessions
    Status,
    /// Terminate all mutagen sessions for this project
    Terminate,
}
