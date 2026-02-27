use std::io::IsTerminal;
use std::process::ExitCode;
use ebdev_remote::RemoteExecutor;

use crate::cli::RemoteCommands;

pub async fn handle_remote_command(command: RemoteCommands, embedded_binary: &'static [u8]) -> anyhow::Result<ExitCode> {
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
            // Verwende embedded binary falls verfügbar, sonst file-based fallback
            let mut executor = RemoteExecutor::connect_with_embedded(&container, embedded_binary)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e))?;
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

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

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
    event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ebdev_remote::ExecuteEvent>,
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
    event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ebdev_remote::ExecuteEvent>,
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
