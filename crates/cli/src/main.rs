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
enum RemoteCommands {
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
    },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Install all configured toolchains (node, pnpm)
    Install,
    /// Show loaded configuration info
    Info,
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
        /// Run with TUI visualization
        #[arg(long)]
        tui: bool,
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
    let cli = Cli::parse();

    // RemoteBridge und Remote brauchen keine Config - direkt ausführen
    if matches!(cli.command, Commands::RemoteBridge) {
        if let Err(e) = ebdev_remote::run_bridge() {
            eprintln!("Remote bridge error: {}", e);
            return Ok(ExitCode::FAILURE);
        }
        return Ok(ExitCode::SUCCESS);
    }

    if let Commands::Remote { command } = cli.command {
        return handle_remote_command(command).await;
    }

    let base_path = PathBuf::from(".");
    let config = Config::load_from_dir(&base_path).await?;

    match cli.command {
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Info => {
                println!("Config: .ebdev.ts");
                println!();
                println!("Toolchain:");
                println!("  Node:    {}", config.toolchain.node.version);
                if let Some(pnpm) = &config.toolchain.pnpm {
                    println!("  pnpm:    {}", pnpm.version);
                }
                if let Some(mutagen) = &config.toolchain.mutagen {
                    println!("  Mutagen: {}", mutagen.version);
                }
            }
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
                let projects = discover_projects(&base_path).await?;

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
            MutagenCommands::Sync { init, sync, keep_open, terminate, tui } => {
                let mutagen_version = config.toolchain.mutagen
                    .as_ref()
                    .map(|m| m.version.as_str())
                    .ok_or_else(|| anyhow::anyhow!("No mutagen version configured in config file"))?;

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

                let projects = discover_projects(&base_path).await?;

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

                // Use new V2 API with Operator Pattern Controller
                let backend = std::sync::Arc::new(
                    ebdev_mutagen_runner::RealMutagen::new(mutagen_bin.to_path_buf())
                );
                if tui {
                    ebdev_mutagen_runner::run_sync_tui(backend, projects, options).await?;
                } else {
                    ebdev_mutagen_runner::run_sync_headless(backend, projects, options).await?;
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
        Commands::Tasks => {
            let config_path = base_path.join(".ebdev.ts");
            if !config_path.exists() {
                eprintln!("No .ebdev.ts found in current directory");
                return Ok(ExitCode::FAILURE);
            }

            let tasks = ebdev_toolchain_deno::list_tasks(&config_path).await?;

            if tasks.is_empty() {
                println!("No tasks found in .ebdev.ts");
                println!();
                println!("Define tasks as exported async functions:");
                println!();
                println!("  export async function build() {{");
                println!("    await exec([\"npm\", \"run\", \"build\"]);");
                println!("  }}");
            } else {
                println!("Available tasks:\n");
                for task in tasks {
                    println!("  {}", task);
                }
                println!();
                println!("Run a task with: ebdev task <name>");
            }
        }
        Commands::Task { name, tui, debug_log } => {
            let config_path = base_path.join(".ebdev.ts");
            if !config_path.exists() {
                eprintln!("No .ebdev.ts found in current directory");
                return Ok(ExitCode::FAILURE);
            }

            if tui {
                // Run with TUI visualization
                return run_task_with_tui(&config_path, &name, &base_path, debug_log).await;
            } else {
                // Run in headless mode with PTY support
                return run_task_headless(&config_path, &name, &base_path, debug_log).await;
            }
        }
        // Handled earlier before config load
        Commands::RemoteBridge => unreachable!(),
        Commands::Remote { .. } => unreachable!(),
    }

    Ok(ExitCode::SUCCESS)
}

/// Handle remote commands (don't need config)
async fn handle_remote_command(command: RemoteCommands) -> anyhow::Result<ExitCode> {
    match command {
        RemoteCommands::Run { container, command, args, workdir } => {
            remote_run(&container, &command, &args, workdir.as_deref()).await
        }
    }
}

/// Run a command inside a Docker container via the bridge protocol
async fn remote_run(
    container: &str,
    program: &str,
    args: &[String],
    workdir: Option<&str>,
) -> anyhow::Result<ExitCode> {
    use std::process::Stdio;
    use tokio::process::Command;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use ebdev_remote::{Request, Response, OutputStream, MAGIC, encode_message, decode_message};

    // 1. Find the Linux binary path
    let binary_path = find_linux_binary()?;

    // 2. Copy binary to container
    let container_binary_path = "/tmp/ebdev";
    let cp_status = Command::new("docker")
        .args(["cp", &binary_path.to_string_lossy(), &format!("{}:{}", container, container_binary_path)])
        .status()
        .await?;

    if !cp_status.success() {
        anyhow::bail!("Failed to copy binary to container");
    }

    // 3. Make it executable
    let chmod_status = Command::new("docker")
        .args(["exec", container, "chmod", "+x", container_binary_path])
        .status()
        .await?;

    if !chmod_status.success() {
        anyhow::bail!("Failed to make binary executable");
    }

    // 4. Start the bridge process in the container
    let mut child = Command::new("docker")
        .args(["exec", "-i", container, container_binary_path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    // 5. Wait for MAGIC + Ready
    let mut magic_buf = [0u8; 4];
    stdout.read_exact(&mut magic_buf).await?;
    if &magic_buf != MAGIC {
        anyhow::bail!("Invalid magic bytes from bridge");
    }

    let mut buffer = Vec::new();
    let mut read_buf = [0u8; 4096];

    // Read Ready response
    loop {
        let n = stdout.read(&mut read_buf).await?;
        if n == 0 {
            anyhow::bail!("Bridge closed unexpectedly");
        }
        buffer.extend_from_slice(&read_buf[..n]);

        if let Some((response, consumed)) = decode_message::<Response>(&buffer)? {
            buffer.drain(..consumed);
            match response {
                Response::Ready => break,
                _ => anyhow::bail!("Expected Ready, got {:?}", response),
            }
        }
    }

    // 6. Send Execute request
    let request = Request::Execute {
        program: program.to_string(),
        args: args.to_vec(),
        working_dir: workdir.map(|s| s.to_string()),
        env: vec![],
    };
    let msg = encode_message(&request)?;
    stdin.write_all(&msg).await?;
    stdin.flush().await?;

    // 7. Read responses and stream output
    let mut exit_code = None;

    loop {
        let n = stdout.read(&mut read_buf).await?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&read_buf[..n]);

        while let Some((response, consumed)) = decode_message::<Response>(&buffer)? {
            buffer.drain(..consumed);

            match response {
                Response::Output { stream, data } => {
                    match stream {
                        OutputStream::Stdout => {
                            tokio::io::stdout().write_all(&data).await?;
                        }
                        OutputStream::Stderr => {
                            tokio::io::stderr().write_all(&data).await?;
                        }
                    }
                }
                Response::Exit { code } => {
                    exit_code = code;
                }
                Response::Error { message } => {
                    eprintln!("Remote error: {}", message);
                    return Ok(ExitCode::FAILURE);
                }
                Response::Ready => {}
            }
        }

        if exit_code.is_some() {
            break;
        }
    }

    // 8. Send shutdown and wait
    let shutdown = encode_message(&Request::Shutdown)?;
    let _ = stdin.write_all(&shutdown).await;
    let _ = child.wait().await;

    Ok(ExitCode::from(exit_code.unwrap_or(1) as u8))
}

/// Find the Linux bridge binary (built via make build-linux)
fn find_linux_binary() -> anyhow::Result<std::path::PathBuf> {
    // Look in target/linux relative to current dir or executable
    let candidates = [
        std::path::PathBuf::from("target/linux/ebdev-bridge"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../linux/ebdev-bridge")))
            .unwrap_or_default(),
    ];

    for path in candidates {
        if path.exists() {
            return Ok(path);
        }
    }

    anyhow::bail!(
        "Linux bridge binary not found. Run 'make build-linux' first.\n\
         Expected at: target/linux/ebdev-bridge"
    )
}

/// Run a task with TUI visualization
async fn run_task_with_tui(
    config_path: &std::path::Path,
    task_name: &str,
    base_path: &PathBuf,
    debug_log: Option<std::path::PathBuf>,
) -> anyhow::Result<ExitCode> {
    // Start TUI task runner in separate thread
    let (handle, tui_thread) = match ebdev_task_runner::run_with_tui(
        task_name.to_string(),
        Some(base_path.to_string_lossy().to_string()),
        debug_log,
    ) {
        Ok(r) => r,
        Err(ebdev_task_runner::TaskRunnerError::NotATty) => {
            eprintln!("Error: TUI requires an interactive terminal.");
            eprintln!("Run without --tui flag or use an interactive terminal.");
            return Ok(ExitCode::FAILURE);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            return Ok(ExitCode::FAILURE);
        }
    };

    let handle_for_shutdown = handle.clone();
    let config_path = config_path.to_path_buf();
    let task_name = task_name.to_string();

    // Run Deno in main thread
    let deno_result = ebdev_toolchain_deno::run_task(&config_path, &task_name, Some(handle)).await;

    // Signal shutdown to TUI
    if let Err(e) = handle_for_shutdown.shutdown() {
        eprintln!("Warning: Failed to send shutdown signal: {}", e);
    }

    // Wait for TUI thread
    let tui_result = tui_thread.join();

    // Check results
    if let Err(e) = deno_result {
        eprintln!("Task failed: {}", e);
        return Ok(ExitCode::FAILURE);
    }

    if let Err(e) = tui_result {
        eprintln!("TUI thread error: {:?}", e);
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}

/// Run a task in headless mode with PTY support
async fn run_task_headless(
    config_path: &std::path::Path,
    task_name: &str,
    base_path: &PathBuf,
    debug_log: Option<std::path::PathBuf>,
) -> anyhow::Result<ExitCode> {
    // Start headless task runner in separate thread
    let (handle, runner_thread) = ebdev_task_runner::run_headless(
        Some(base_path.to_string_lossy().to_string()),
        debug_log,
    );

    let handle_for_shutdown = handle.clone();
    let config_path = config_path.to_path_buf();
    let task_name = task_name.to_string();

    // Run Deno in main thread
    let deno_result = ebdev_toolchain_deno::run_task(&config_path, &task_name, Some(handle)).await;

    // Signal shutdown
    let _ = handle_for_shutdown.shutdown();

    // Wait for runner thread
    let runner_result = runner_thread.join();

    // Check results
    if let Err(e) = deno_result {
        eprintln!("Task failed: {}", e);
        return Ok(ExitCode::FAILURE);
    }

    if let Err(e) = runner_result {
        eprintln!("Runner thread error: {:?}", e);
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}
