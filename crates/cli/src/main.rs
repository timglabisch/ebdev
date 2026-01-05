use std::ffi::OsString;
use std::io::IsTerminal;
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
        /// Run in interactive mode with PTY (for vim, htop, etc.)
        #[arg(long, short = 'i')]
        interactive: bool,
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
        if let Err(e) = ebdev_remote::run_bridge().await {
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
        RemoteCommands::Run { container, command, args, workdir, interactive } => {
            let pty_config = if interactive {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("Interactive mode requires a terminal. Use without -i for non-interactive.");
                }
                Some(ebdev_remote::PtyConfig {
                    cols: get_terminal_size()?.0,
                    rows: get_terminal_size()?.1,
                })
            } else {
                None
            };

            // Gemeinsame Ausführungslogik über Executor Trait
            let mut executor = RemoteExecutor::connect(&container).await?;
            run_with_executor(&mut executor, &command, &args, workdir.as_deref(), pty_config, interactive).await
        }
    }
}

/// Führt einen Befehl mit einem beliebigen Executor aus
async fn run_with_executor<E: ebdev_remote::Executor>(
    executor: &mut E,
    program: &str,
    args: &[String],
    workdir: Option<&str>,
    pty: Option<ebdev_remote::PtyConfig>,
    interactive: bool,
) -> anyhow::Result<ExitCode> {
    use ebdev_remote::ExecuteOptions;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);

    let options = ExecuteOptions {
        program: program.to_string(),
        args: args.to_vec(),
        workdir: workdir.map(|s| s.to_string()),
        env: vec![],
        pty,
    };

    let handle = executor
        .execute(options, event_tx)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    if interactive {
        run_interactive_loop(handle, &mut event_rx).await
    } else {
        run_simple_loop(&mut event_rx).await
    }
}

/// Einfache Ausführung: Output streamen bis Exit
async fn run_simple_loop(
    event_rx: &mut tokio::sync::mpsc::Receiver<ebdev_remote::ExecuteEvent>,
) -> anyhow::Result<ExitCode> {
    use ebdev_remote::{ExecuteEvent, OutputStream};
    use tokio::io::AsyncWriteExt;

    let mut exit_code = None;

    while let Some(event) = event_rx.recv().await {
        match event {
            ExecuteEvent::Output { stream, data } => {
                match stream {
                    OutputStream::Stdout => tokio::io::stdout().write_all(&data).await?,
                    OutputStream::Stderr => tokio::io::stderr().write_all(&data).await?,
                }
            }
            ExecuteEvent::Exit { code } => {
                exit_code = code;
                break;
            }
        }
    }

    Ok(ExitCode::from(exit_code.unwrap_or(1) as u8))
}

/// Interaktive Ausführung: stdin/stdout/resize multiplexen
async fn run_interactive_loop(
    handle: ebdev_remote::ExecuteHandle,
    event_rx: &mut tokio::sync::mpsc::Receiver<ebdev_remote::ExecuteEvent>,
) -> anyhow::Result<ExitCode> {
    use ebdev_remote::ExecuteEvent;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let orig_termios = set_raw_mode()?;
    let _guard = RawModeGuard { orig: orig_termios };

    let mut host_stdin = tokio::io::stdin();
    let mut host_stdout = tokio::io::stdout();
    let mut stdin_buf = [0u8; 4096];
    let mut sigwinch = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    let mut exit_code = None;

    loop {
        tokio::select! {
            biased;

            _ = sigwinch.recv() => {
                if let (Ok((cols, rows)), Some(ref resize_tx)) = (get_terminal_size(), &handle.resize_tx) {
                    let _ = resize_tx.send((cols, rows)).await;
                }
            }

            result = host_stdin.read(&mut stdin_buf) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = handle.stdin_tx.send(stdin_buf[..n].to_vec()).await;
                    }
                }
            }

            event = event_rx.recv() => {
                match event {
                    Some(ExecuteEvent::Output { data, .. }) => {
                        host_stdout.write_all(&data).await?;
                        host_stdout.flush().await?;
                    }
                    Some(ExecuteEvent::Exit { code }) => {
                        exit_code = code;
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(ExitCode::from(exit_code.unwrap_or(0) as u8))
}

/// Remote Executor - führt Befehle über Docker Bridge aus
struct RemoteExecutor {
    #[allow(dead_code)]
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
    buffer: Vec<u8>,
    session_counter: u32,
}

impl RemoteExecutor {
    async fn connect(container: &str) -> anyhow::Result<Self> {
        use tokio::process::Command;
        use ebdev_remote::PROTOCOL_VERSION;

        let container_binary_path = "/tmp/ebdev";

        // Try to start existing binary first
        let existing_result = Self::try_start_bridge(container, container_binary_path).await;

        match existing_result {
            Ok((child, stdin, stdout, buffer, version)) if version == PROTOCOL_VERSION => {
                // Binary exists and has correct version
                return Ok(Self { child, stdin: Some(stdin), stdout: Some(stdout), buffer, session_counter: 0 });
            }
            Ok((mut child, _, _, _, version)) => {
                // Binary exists but wrong version - kill it and copy new one
                let _ = child.kill().await;
                eprintln!("Bridge version mismatch (got {}, need {}), updating...", version, PROTOCOL_VERSION);
            }
            Err(_) => {
                // Binary doesn't exist or failed to start
            }
        }

        // Copy binary to container
        let binary_path = find_linux_binary()?;
        let cp_status = Command::new("docker")
            .args(["cp", &binary_path.to_string_lossy(), &format!("{}:{}", container, container_binary_path)])
            .status()
            .await?;
        if !cp_status.success() {
            anyhow::bail!("Failed to copy binary to container");
        }

        // Make executable
        let chmod_status = Command::new("docker")
            .args(["exec", container, "chmod", "+x", container_binary_path])
            .status()
            .await?;
        if !chmod_status.success() {
            anyhow::bail!("Failed to make binary executable");
        }

        // Start bridge
        let (child, stdin, stdout, buffer, version) = Self::try_start_bridge(container, container_binary_path).await?;

        if version != PROTOCOL_VERSION {
            anyhow::bail!("Bridge protocol version mismatch after copy (got {}, need {})", version, PROTOCOL_VERSION);
        }

        Ok(Self { child, stdin: Some(stdin), stdout: Some(stdout), buffer, session_counter: 0 })
    }

    /// Try to start the bridge and get version, returns (child, stdin, stdout, buffer, protocol_version)
    async fn try_start_bridge(
        container: &str,
        binary_path: &str,
    ) -> anyhow::Result<(tokio::process::Child, tokio::process::ChildStdin, tokio::process::ChildStdout, Vec<u8>, u32)> {
        use std::process::Stdio;
        use tokio::process::Command;
        use tokio::io::AsyncReadExt;
        use ebdev_remote::{Response, MAGIC, decode_message};

        let mut child = Command::new("docker")
            .args(["exec", "-i", container, binary_path])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Suppress stderr for version check
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = child.stdout.take().expect("stdout");

        // Wait for MAGIC with timeout
        let mut magic_buf = [0u8; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stdout.read_exact(&mut magic_buf)
        ).await.map_err(|_| anyhow::anyhow!("Timeout waiting for bridge magic"))??;

        if &magic_buf != MAGIC {
            anyhow::bail!("Invalid magic bytes from bridge");
        }

        let mut buffer = Vec::new();
        let mut read_buf = [0u8; 4096];

        // Wait for Ready with timeout
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("Timeout waiting for bridge ready");
            }

            let n = tokio::time::timeout(remaining, stdout.read(&mut read_buf))
                .await
                .map_err(|_| anyhow::anyhow!("Timeout reading from bridge"))??;

            if n == 0 {
                anyhow::bail!("Bridge closed unexpectedly");
            }
            buffer.extend_from_slice(&read_buf[..n]);

            if let Some((response, consumed)) = decode_message::<Response>(&buffer)? {
                buffer.drain(..consumed);
                match response {
                    Response::Ready { protocol_version } => {
                        return Ok((child, stdin, stdout, buffer, protocol_version));
                    }
                    _ => anyhow::bail!("Expected Ready, got {:?}", response),
                }
            }
        }
    }
}

/// Request-Typ für den Bridge-Writer
enum BridgeRequest {
    Stdin { session_id: u32, data: Vec<u8> },
    Resize { session_id: u32, cols: u16, rows: u16 },
}

impl ebdev_remote::Executor for RemoteExecutor {
    async fn execute(
        &mut self,
        options: ebdev_remote::ExecuteOptions,
        event_tx: tokio::sync::mpsc::Sender<ebdev_remote::ExecuteEvent>,
    ) -> Result<ebdev_remote::ExecuteHandle, ebdev_remote::ExecutorError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use ebdev_remote::{Request, Response, ExecuteEvent, ExecuteHandle, ExecutorError, encode_message, decode_message};

        self.session_counter += 1;
        let session_id = self.session_counter;

        // Send Execute request
        let request = Request::Execute {
            session_id,
            program: options.program,
            args: options.args,
            working_dir: options.workdir,
            env: options.env,
            pty: options.pty,
        };
        let msg = encode_message(&request).map_err(|e| ExecutorError::Protocol(e.to_string()))?;
        let stdin = self.stdin.as_mut().ok_or_else(|| ExecutorError::Protocol("stdin already taken".into()))?;
        stdin.write_all(&msg).await?;
        stdin.flush().await?;

        // Wait for Started
        let stdout = self.stdout.as_mut().ok_or_else(|| ExecutorError::Protocol("stdout already taken".into()))?;
        let mut read_buf = [0u8; 4096];

        loop {
            let n = stdout.read(&mut read_buf).await?;
            if n == 0 {
                return Err(ExecutorError::Protocol("Bridge closed unexpectedly".into()));
            }
            self.buffer.extend_from_slice(&read_buf[..n]);

            if let Some((response, consumed)) = decode_message::<Response>(&self.buffer)
                .map_err(|e| ExecutorError::Protocol(e.to_string()))?
            {
                self.buffer.drain(..consumed);
                match response {
                    Response::Started { session_id: sid } if sid == session_id => break,
                    Response::Error { message, .. } => return Err(ExecutorError::Spawn(message)),
                    _ => return Err(ExecutorError::Protocol(format!("Expected Started, got {:?}", response))),
                }
            }
        }

        // Channels for user-facing handle
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(4);

        // Internal channel for bridge requests
        let (bridge_tx, mut bridge_rx) = tokio::sync::mpsc::channel::<BridgeRequest>(32);

        // Forward stdin to bridge channel
        let bridge_tx_stdin = bridge_tx.clone();
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if bridge_tx_stdin.send(BridgeRequest::Stdin { session_id, data }).await.is_err() {
                    break;
                }
            }
        });

        // Forward resize to bridge channel
        let bridge_tx_resize = bridge_tx;
        tokio::spawn(async move {
            while let Some((cols, rows)) = resize_rx.recv().await {
                if bridge_tx_resize.send(BridgeRequest::Resize { session_id, cols, rows }).await.is_err() {
                    break;
                }
            }
        });

        // Take stdin and stdout for the I/O tasks
        let mut stdin = self.stdin.take().ok_or_else(|| ExecutorError::Protocol("stdin already taken".into()))?;
        let mut stdout = self.stdout.take().ok_or_else(|| ExecutorError::Protocol("stdout already taken".into()))?;
        let mut buffer = std::mem::take(&mut self.buffer);

        // Single writer task for all bridge requests
        tokio::spawn(async move {
            while let Some(req) = bridge_rx.recv().await {
                let request = match req {
                    BridgeRequest::Stdin { session_id, data } => Request::Stdin { session_id, data },
                    BridgeRequest::Resize { session_id, cols, rows } => Request::Resize { session_id, cols, rows },
                };
                if let Ok(msg) = encode_message(&request) {
                    if stdin.write_all(&msg).await.is_err() { break; }
                    if stdin.flush().await.is_err() { break; }
                }
            }
        });

        // Response reader task
        tokio::spawn(async move {
            let mut read_buf = [0u8; 4096];
            loop {
                match stdout.read(&mut read_buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buffer.extend_from_slice(&read_buf[..n]);

                        while let Ok(Some((response, consumed))) = decode_message::<Response>(&buffer) {
                            buffer.drain(..consumed);

                            let event = match response {
                                Response::Output { session_id: sid, stream, data } if sid == session_id => {
                                    Some(ExecuteEvent::Output { stream, data })
                                }
                                Response::Exit { session_id: sid, code } if sid == session_id => {
                                    Some(ExecuteEvent::Exit { code })
                                }
                                _ => None,
                            };

                            if let Some(evt) = event {
                                let is_exit = matches!(evt, ExecuteEvent::Exit { .. });
                                if event_tx.send(evt).await.is_err() { return; }
                                if is_exit { return; }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(ExecuteHandle {
            stdin_tx,
            resize_tx: Some(resize_tx),
            kill_tx: None, // Kill not implemented for remote - would need to send Request::Kill
        })
    }
}

/// RAII guard for raw terminal mode
struct RawModeGuard {
    orig: libc::termios,
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal_mode(&self.orig);
    }
}

/// Get terminal size
fn get_terminal_size() -> anyhow::Result<(u16, u16)> {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) != 0 {
            anyhow::bail!("Failed to get terminal size");
        }
        Ok((size.ws_col, size.ws_row))
    }
}

/// Set terminal to raw mode, returns original termios for restoration
fn set_raw_mode() -> anyhow::Result<libc::termios> {
    unsafe {
        let mut orig: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut orig) != 0 {
            anyhow::bail!("Failed to get terminal attributes");
        }

        let mut raw = orig;
        libc::cfmakeraw(&mut raw);

        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
            anyhow::bail!("Failed to set raw mode");
        }

        Ok(orig)
    }
}

/// Restore terminal mode
fn restore_terminal_mode(orig: &libc::termios) {
    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, orig);
    }
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
