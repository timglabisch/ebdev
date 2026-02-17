//! LocalExecutor - Führt Prozesse direkt auf dem System aus

use crate::executor::{ExecuteEvent, ExecuteHandle, ExecuteOptions, Executor, ExecutorError};
use crate::{OutputStream, PtyConfig};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Executor der Prozesse lokal ausführt
#[derive(Default)]
pub struct LocalExecutor;

impl LocalExecutor {
    pub fn new() -> Self {
        Self
    }
}

impl Executor for LocalExecutor {
    async fn execute(
        &mut self,
        options: ExecuteOptions,
        event_tx: mpsc::Sender<ExecuteEvent>,
    ) -> Result<ExecuteHandle, ExecutorError> {
        if options.pty.is_some() {
            start_pty_process(options, event_tx).await
        } else {
            start_simple_process(options, event_tx).await
        }
    }
}

/// Startet einen einfachen Prozess ohne PTY
async fn start_simple_process(
    options: ExecuteOptions,
    event_tx: mpsc::Sender<ExecuteEvent>,
) -> Result<ExecuteHandle, ExecutorError> {
    let mut cmd = Command::new(&options.program);
    cmd.args(&options.args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(ref dir) = options.workdir {
        cmd.current_dir(dir);
    }

    for (key, value) in &options.env {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn()?;

    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(16);
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

    // Stdin-Writer Task
    if let Some(mut stdin) = child_stdin {
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });
    }

    // Stdout-Reader Task
    if let Some(mut stdout) = child_stdout {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx
                            .send(ExecuteEvent::Output {
                                stream: OutputStream::Stdout,
                                data: buf[..n].to_vec(),
                            })
                            .await;
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Stderr-Reader Task
    if let Some(mut stderr) = child_stderr {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx
                            .send(ExecuteEvent::Output {
                                stream: OutputStream::Stderr,
                                data: buf[..n].to_vec(),
                            })
                            .await;
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Wait Task with kill support
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                let code = status.ok().and_then(|s| s.code());
                let _ = event_tx.send(ExecuteEvent::Exit { code }).await;
            }
            _ = kill_rx => {
                // Kill signal received
                let _ = child.kill().await;
                let status = child.wait().await;
                let code = status.ok().and_then(|s| s.code());
                let _ = event_tx.send(ExecuteEvent::Exit { code }).await;
            }
        }
    });

    Ok(ExecuteHandle {
        stdin_tx,
        resize_tx: None,
        kill_tx: Some(kill_tx),
    })
}

/// PTY-Paar (master und slave file descriptors)
struct Pty {
    master: OwnedFd,
    slave: OwnedFd,
}

impl Pty {
    fn new() -> Result<Self, ExecutorError> {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            if master < 0 {
                return Err(ExecutorError::Spawn(format!(
                    "posix_openpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            if libc::grantpt(master) != 0 {
                libc::close(master);
                return Err(ExecutorError::Spawn(format!(
                    "grantpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            if libc::unlockpt(master) != 0 {
                libc::close(master);
                return Err(ExecutorError::Spawn(format!(
                    "unlockpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let slave_name = libc::ptsname(master);
            if slave_name.is_null() {
                libc::close(master);
                return Err(ExecutorError::Spawn(format!(
                    "ptsname failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let slave = libc::open(slave_name, libc::O_RDWR | libc::O_NOCTTY);
            if slave < 0 {
                libc::close(master);
                return Err(ExecutorError::Spawn(format!(
                    "open slave failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            Ok(Pty {
                master: OwnedFd::from_raw_fd(master),
                slave: OwnedFd::from_raw_fd(slave),
            })
        }
    }

    fn set_size(&self, cols: u16, rows: u16) -> Result<(), ExecutorError> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        unsafe {
            #[cfg(target_os = "macos")]
            let ret = libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as libc::c_ulong,
                &size,
            );
            #[cfg(target_os = "linux")]
            let ret = libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as libc::c_int,
                &size,
            );
            if ret != 0 {
                return Err(ExecutorError::Spawn(format!(
                    "ioctl TIOCSWINSZ failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }

        Ok(())
    }
}

/// Resolve a program name to its full path by searching PATH.
/// If the program already contains a '/', it's returned as-is.
fn resolve_program_path(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let full = std::path::PathBuf::from(dir).join(program);
            if full.exists() {
                return full.to_string_lossy().to_string();
            }
        }
    }
    // Fallback: return as-is, execve will fail with a clear error
    program.to_string()
}

/// Startet einen Prozess mit PTY
async fn start_pty_process(
    options: ExecuteOptions,
    event_tx: mpsc::Sender<ExecuteEvent>,
) -> Result<ExecuteHandle, ExecutorError> {
    let pty_config = options.pty.unwrap_or(PtyConfig { cols: 80, rows: 24 });
    let pty = Pty::new()?;
    pty.set_size(pty_config.cols, pty_config.rows)?;

    let master_fd = pty.master.as_raw_fd();
    let slave_fd = pty.slave.as_raw_fd();

    // Prepare everything BEFORE fork() — after fork() in a multi-threaded process,
    // only async-signal-safe functions may be called. std::env, CString::new (allocates),
    // etc. can deadlock if another thread holds their internal locks at fork time.

    // Resolve program path via PATH lookup before fork (execve doesn't search PATH)
    let resolved_program = resolve_program_path(&options.program);
    let c_program = std::ffi::CString::new(resolved_program.as_str())
        .map_err(|e| ExecutorError::Spawn(format!("invalid program name: {}", e)))?;

    let c_args: Vec<std::ffi::CString> = std::iter::once(c_program.clone())
        .chain(
            options
                .args
                .iter()
                .filter_map(|a| std::ffi::CString::new(a.as_str()).ok()),
        )
        .collect();
    let c_args_ptrs: Vec<*const libc::c_char> = c_args
        .iter()
        .map(|a| a.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    // Build full environment for execve: inherit current env + apply overrides + ensure TERM
    let mut env_map: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (key, value) in &options.env {
        env_map.insert(key.clone(), value.clone());
    }
    env_map.entry("TERM".to_string()).or_insert_with(|| "xterm-256color".to_string());

    let c_env: Vec<std::ffi::CString> = env_map
        .iter()
        .filter_map(|(k, v)| std::ffi::CString::new(format!("{}={}", k, v)).ok())
        .collect();
    let c_env_ptrs: Vec<*const libc::c_char> = c_env
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    let c_workdir = options.workdir.as_ref()
        .and_then(|d| std::ffi::CString::new(d.as_str()).ok());

    let child_pid = unsafe {
        let pid = libc::fork();

        if pid < 0 {
            return Err(ExecutorError::Spawn(format!(
                "fork failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        if pid == 0 {
            // Child process — only async-signal-safe calls from here!

            // Neue Session starten
            libc::setsid();

            // Slave als controlling terminal setzen
            #[cfg(target_os = "macos")]
            libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0);
            #[cfg(target_os = "linux")]
            libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_int, 0);

            // Stdio auf slave umleiten
            libc::dup2(slave_fd, libc::STDIN_FILENO);
            libc::dup2(slave_fd, libc::STDOUT_FILENO);
            libc::dup2(slave_fd, libc::STDERR_FILENO);

            // Alle anderen FDs schließen
            if slave_fd > 2 {
                libc::close(slave_fd);
            }
            libc::close(master_fd);

            // Working directory setzen
            if let Some(ref c_dir) = c_workdir {
                libc::chdir(c_dir.as_ptr());
            }

            // execve with pre-built environment (no std::env calls after fork!)
            libc::execve(c_program.as_ptr(), c_args_ptrs.as_ptr(), c_env_ptrs.as_ptr());

            // Wenn wir hier ankommen, ist execvp fehlgeschlagen
            libc::_exit(127);
        }

        pid
    };

    // Parent process - slave schließen
    drop(pty.slave);

    // Master FD non-blocking setzen
    unsafe {
        let flags = libc::fcntl(master_fd, libc::F_GETFL);
        libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(16);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(4);
    let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();

    let master_fd_owned = pty.master;

    // PTY I/O Task
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        use tokio::io::unix::AsyncFd;

        let async_fd = match AsyncFd::new(master_fd_owned) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = event_tx_clone.send(ExecuteEvent::Exit { code: Some(1) }).await;
                eprintln!("Failed to create AsyncFd: {}", e);
                return;
            }
        };

        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                biased;

                // Schreibe stdin zum PTY
                Some(data) = stdin_rx.recv() => {
                    let fd = async_fd.as_raw_fd();
                    let mut written = 0;
                    while written < data.len() {
                        if let Ok(mut guard) = async_fd.writable().await {
                            let n = unsafe {
                                libc::write(
                                    fd,
                                    data[written..].as_ptr() as *const libc::c_void,
                                    data.len() - written,
                                )
                            };
                            if n > 0 {
                                written += n as usize;
                                guard.clear_ready();
                            } else if n < 0 {
                                let err = std::io::Error::last_os_error();
                                if err.kind() == std::io::ErrorKind::WouldBlock {
                                    guard.clear_ready();
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }

                // Handle resize
                Some((cols, rows)) = resize_rx.recv() => {
                    let size = libc::winsize {
                        ws_row: rows,
                        ws_col: cols,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    };
                    unsafe {
                        #[cfg(target_os = "macos")]
                        libc::ioctl(async_fd.as_raw_fd(), libc::TIOCSWINSZ as libc::c_ulong, &size);
                        #[cfg(target_os = "linux")]
                        libc::ioctl(async_fd.as_raw_fd(), libc::TIOCSWINSZ as libc::c_int, &size);
                    }
                }

                // Lese vom PTY
                readable = async_fd.readable() => {
                    match readable {
                        Ok(mut guard) => {
                            let fd = async_fd.as_raw_fd();
                            let n = unsafe {
                                libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
                            };

                            if n > 0 {
                                let _ = event_tx_clone
                                    .send(ExecuteEvent::Output {
                                        stream: OutputStream::Stdout,
                                        data: buf[..n as usize].to_vec(),
                                    })
                                    .await;
                                guard.clear_ready();
                            } else if n == 0 {
                                break;
                            } else {
                                let err = std::io::Error::last_os_error();
                                if err.kind() == std::io::ErrorKind::WouldBlock {
                                    guard.clear_ready();
                                } else if err.raw_os_error() == Some(libc::EIO) {
                                    break;
                                } else {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    // Wait für Child-Prozess mit Kill-Support
    tokio::spawn(async move {
        loop {
            // Check for kill signal
            if kill_rx.try_recv().is_ok() {
                // Kill the process
                unsafe {
                    libc::kill(child_pid, libc::SIGKILL);
                }
            }

            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };

            if result == child_pid {
                let code = if libc::WIFEXITED(status) {
                    Some(libc::WEXITSTATUS(status))
                } else {
                    None
                };
                let _ = event_tx.send(ExecuteEvent::Exit { code }).await;
                break;
            } else if result < 0 {
                let _ = event_tx.send(ExecuteEvent::Exit { code: None }).await;
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    });

    Ok(ExecuteHandle {
        stdin_tx,
        resize_tx: Some(resize_tx),
        kill_tx: Some(kill_tx),
    })
}
