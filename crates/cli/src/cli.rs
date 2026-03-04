use clap::{Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, ArgValueCompleter};

use crate::completions::{complete_task_args, complete_task_names, complete_flag_names, complete_flag_value, complete_with_flags};

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
        #[arg(add = ArgValueCandidates::new(complete_task_names))]
        name: String,
        /// Disable TUI (use headless mode). TUI is enabled by default when running in a terminal.
        #[arg(long)]
        no_tui: bool,
        /// Log all executor communication to file (JSON format)
        #[arg(long)]
        debug_log: Option<std::path::PathBuf>,
        /// Activate flags for this run (e.g. --with search --with search:engine=meilisearch). Use --with without values for interactive flag selection.
        #[arg(long = "with", num_args = 0.., add = ArgValueCandidates::new(complete_with_flags))]
        with_flags: Vec<String>,
        /// Deactivate flags for this run
        #[arg(long = "without", add = ArgValueCandidates::new(complete_with_flags))]
        without_flags: Vec<String>,
        /// Arguments to pass to the task (after --)
        #[arg(last = true, add = ArgValueCompleter::new(complete_task_args))]
        task_args: Vec<String>,
    },
    /// List all available tasks from .ebdev.ts
    Tasks {
        /// Output task info as JSON (for tooling/completion)
        #[arg(long)]
        json: bool,
    },
    /// List and configure feature flags
    #[command(alias = "features")]
    Flags {
        /// Output as JSON (non-interactive)
        #[arg(long)]
        json: bool,
    },
    /// Set a feature flag value
    Flag {
        /// Flag name or flag.field (e.g. "search" or "search.engine")
        #[arg(add = ArgValueCandidates::new(complete_flag_names))]
        name: String,
        /// Value: on/off for boolean, or string value for config fields
        #[arg(add = ArgValueCompleter::new(complete_flag_value))]
        value: Option<String>,
    },
    /// Run commands in Docker containers via bridge
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for (zsh, bash, fish). Omit for setup instructions.
        shell: Option<String>,
    },
    /// Internal: Run as remote bridge inside a container (used by remote run)
    #[command(hide = true)]
    RemoteBridge,
    /// Internal: Complete arg values for a task (used by shell completion)
    #[command(hide = true)]
    CompleteArg {
        /// Task name (export name in .ebdev.ts)
        task: String,
        /// Arg name (camelCase field name)
        arg: String,
    },
    /// Internal: Complete flag config field values (used by shell completion)
    #[command(hide = true)]
    CompleteFlag {
        /// Flag name
        flag: String,
        /// Field name (camelCase)
        field: String,
    },
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
