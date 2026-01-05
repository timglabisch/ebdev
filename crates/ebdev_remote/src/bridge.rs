//! Bridge-Implementierung mit tokio und PTY-Support

use crate::{decode_message, encode_message, OutputStream, PtyConfig, Request, Response, MAGIC};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Fehlertyp für Bridge-Operationen
#[derive(Debug)]
pub enum BridgeError {
    Io(std::io::Error),
    Bincode(bincode::Error),
    Pty(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Io(e) => write!(f, "IO error: {}", e),
            BridgeError::Bincode(e) => write!(f, "Bincode error: {}", e),
            BridgeError::Pty(e) => write!(f, "PTY error: {}", e),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self {
        BridgeError::Io(e)
    }
}

impl From<bincode::Error> for BridgeError {
    fn from(e: bincode::Error) -> Self {
        BridgeError::Bincode(e)
    }
}

/// PTY-Paar (master und slave file descriptors)
struct Pty {
    master: OwnedFd,
    slave: OwnedFd,
}

impl Pty {
    /// Erstellt ein neues PTY-Paar
    fn new() -> Result<Self, BridgeError> {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            if master < 0 {
                return Err(BridgeError::Pty(format!(
                    "posix_openpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            if libc::grantpt(master) != 0 {
                libc::close(master);
                return Err(BridgeError::Pty(format!(
                    "grantpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            if libc::unlockpt(master) != 0 {
                libc::close(master);
                return Err(BridgeError::Pty(format!(
                    "unlockpt failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let slave_name = libc::ptsname(master);
            if slave_name.is_null() {
                libc::close(master);
                return Err(BridgeError::Pty(format!(
                    "ptsname failed: {}",
                    std::io::Error::last_os_error()
                )));
            }

            let slave = libc::open(slave_name, libc::O_RDWR | libc::O_NOCTTY);
            if slave < 0 {
                libc::close(master);
                return Err(BridgeError::Pty(format!(
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

    /// Setzt die Terminal-Größe
    fn set_size(&self, cols: u16, rows: u16) -> Result<(), BridgeError> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        unsafe {
            #[cfg(target_os = "macos")]
            let ret = libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as libc::c_ulong, &size);
            #[cfg(target_os = "linux")]
            let ret = libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as libc::c_int, &size);
            if ret != 0 {
                return Err(BridgeError::Pty(format!(
                    "ioctl TIOCSWINSZ failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }

        Ok(())
    }
}

/// Handle für einen laufenden Prozess
struct ProcessHandle {
    /// Empfänger für Prozess-Output (stdout/stderr/exit)
    output_rx: mpsc::Receiver<Response>,
    /// Sender für stdin-Daten
    stdin_tx: mpsc::Sender<Vec<u8>>,
    /// Sender für resize-Events (nur bei PTY)
    resize_tx: Option<mpsc::Sender<(u16, u16)>>,
    /// Prozess-ID für Signal-Handling
    pid: Option<libc::pid_t>,
}

impl ProcessHandle {
    /// Sendet ein Signal an den Prozess
    fn send_signal(&self, signal: i32) {
        if let Some(pid) = self.pid {
            unsafe {
                libc::kill(pid, signal);
            }
        }
    }

    /// Beendet den Prozess sauber (SIGTERM, dann SIGKILL)
    fn kill(&mut self) {
        if let Some(pid) = self.pid.take() {
            kill_process(pid);
        }
    }
}

/// Beendet einen Prozess sauber
fn kill_process(pid: libc::pid_t) {
    unsafe {
        // Prüfen ob Prozess noch existiert
        if libc::kill(pid, 0) != 0 {
            return; // Prozess existiert nicht mehr
        }

        // Erst SIGTERM für sauberes Beenden
        libc::kill(pid, libc::SIGTERM);

        // Kurz warten
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Prüfen ob noch am Leben, dann SIGKILL
        if libc::kill(pid, 0) == 0 {
            libc::kill(pid, libc::SIGKILL);
        }

        // Zombie aufräumen
        libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // Automatisch Prozess beenden wenn Handle gedroppt wird
        if let Some(pid) = self.pid.take() {
            kill_process(pid);
        }
    }
}

/// Startet den Bridge-Modus (async mit tokio)
pub async fn run_bridge() -> Result<(), BridgeError> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    // Sende Magic-Bytes und Ready-Signal
    stdout.write_all(MAGIC).await?;
    let ready_msg = encode_message(&Response::Ready)?;
    stdout.write_all(&ready_msg).await?;
    stdout.flush().await?;

    let mut buffer = Vec::new();
    let mut read_buf = [0u8; 4096];

    // Aktueller Prozess-Handle
    let mut process_handle: Option<ProcessHandle> = None;

    loop {
        tokio::select! {
            biased;

            // Handle Prozess-Output (höhere Priorität)
            Some(response) = async {
                if let Some(ref mut handle) = process_handle {
                    handle.output_rx.recv().await
                } else {
                    std::future::pending::<Option<Response>>().await
                }
            } => {
                let is_exit = matches!(response, Response::Exit { .. });
                let msg = encode_message(&response)?;
                stdout.write_all(&msg).await?;
                stdout.flush().await?;

                if is_exit {
                    process_handle = None;
                }
            }

            // Lese von Host-stdin (Requests)
            result = stdin.read(&mut read_buf) => {
                let n = result?;
                if n == 0 {
                    // EOF - Beende
                    break;
                }
                buffer.extend_from_slice(&read_buf[..n]);

                // Verarbeite alle vollständigen Nachrichten
                while let Some((request, consumed)) = decode_message::<Request>(&buffer)? {
                    buffer.drain(..consumed);

                    match request {
                        Request::Execute {
                            program,
                            args,
                            working_dir,
                            env,
                            pty,
                        } => {
                            // Starte neuen Prozess
                            match start_process(
                                &program,
                                &args,
                                working_dir.as_deref(),
                                &env,
                                pty,
                            ).await {
                                Ok(h) => process_handle = Some(h),
                                Err(e) => {
                                    let error_msg = encode_message(&Response::Error {
                                        message: format!("Failed to start process: {}", e),
                                    })?;
                                    stdout.write_all(&error_msg).await?;
                                    stdout.flush().await?;
                                }
                            }
                        }
                        Request::Stdin { data } => {
                            if let Some(ref handle) = process_handle {
                                let _ = handle.stdin_tx.send(data).await;
                            }
                        }
                        Request::Resize { cols, rows } => {
                            if let Some(ref handle) = process_handle {
                                if let Some(ref resize_tx) = handle.resize_tx {
                                    let _ = resize_tx.send((cols, rows)).await;
                                }
                            }
                        }
                        Request::Signal { signal } => {
                            if let Some(ref handle) = process_handle {
                                handle.send_signal(signal);
                            }
                        }
                        Request::Shutdown => {
                            // Sauber beenden: erst SIGTERM, dann warten, dann SIGKILL
                            if let Some(mut handle) = process_handle.take() {
                                handle.kill();
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Startet einen Prozess (mit oder ohne PTY)
async fn start_process(
    program: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
    pty_config: Option<PtyConfig>,
) -> Result<ProcessHandle, BridgeError> {
    if let Some(config) = pty_config {
        start_pty_process(program, args, working_dir, env, config).await
    } else {
        start_simple_process(program, args, working_dir, env).await
    }
}

/// Startet einen einfachen Prozess ohne PTY
async fn start_simple_process(
    program: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
) -> Result<ProcessHandle, BridgeError> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    for (key, value) in env {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn()?;

    // PID für Signal-Handling erfassen
    let pid = child.id().map(|id| id as libc::pid_t);

    let child_stdin = child.stdin.take();
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    let (output_tx, output_rx) = mpsc::channel::<Response>(64);
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(16);

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
        let tx = output_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx
                            .send(Response::Output {
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
        let tx = output_tx.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx
                            .send(Response::Output {
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

    // Wait Task
    tokio::spawn(async move {
        let status = child.wait().await;
        let code = status.ok().and_then(|s| s.code());
        let _ = output_tx.send(Response::Exit { code }).await;
    });

    Ok(ProcessHandle {
        output_rx,
        stdin_tx,
        resize_tx: None,
        pid,
    })
}

/// Startet einen Prozess mit PTY
async fn start_pty_process(
    program: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
    config: PtyConfig,
) -> Result<ProcessHandle, BridgeError> {
    let pty = Pty::new()?;
    pty.set_size(config.cols, config.rows)?;

    let program = program.to_string();
    let args: Vec<String> = args.to_vec();
    let working_dir = working_dir.map(|s| s.to_string());
    let env: Vec<(String, String)> = env.to_vec();

    let master_fd = pty.master.as_raw_fd();
    let slave_fd = pty.slave.as_raw_fd();

    let child_pid = unsafe {
        let pid = libc::fork();

        if pid < 0 {
            return Err(BridgeError::Pty(format!(
                "fork failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        if pid == 0 {
            // Child process

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
            if let Some(ref dir) = working_dir {
                let c_dir = std::ffi::CString::new(dir.as_str()).unwrap();
                libc::chdir(c_dir.as_ptr());
            }

            // Environment setzen
            for (key, value) in &env {
                std::env::set_var(key, value);
            }

            // TERM setzen falls nicht vorhanden
            if std::env::var("TERM").is_err() {
                std::env::set_var("TERM", "xterm-256color");
            }

            // Programm ausführen
            let c_program = std::ffi::CString::new(program.as_str()).unwrap();
            let c_args: Vec<std::ffi::CString> = std::iter::once(c_program.clone())
                .chain(args.iter().map(|a| std::ffi::CString::new(a.as_str()).unwrap()))
                .collect();
            let c_args_ptrs: Vec<*const libc::c_char> = c_args
                .iter()
                .map(|a| a.as_ptr())
                .chain(std::iter::once(std::ptr::null()))
                .collect();

            libc::execvp(c_program.as_ptr(), c_args_ptrs.as_ptr());

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

    let (output_tx, output_rx) = mpsc::channel::<Response>(64);
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(16);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(4);

    let master_fd_owned = pty.master;

    // PTY I/O Task
    let output_tx_clone = output_tx.clone();
    tokio::spawn(async move {
        use tokio::io::unix::AsyncFd;

        let async_fd = match AsyncFd::new(master_fd_owned) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = output_tx_clone
                    .send(Response::Error {
                        message: format!("Failed to create AsyncFd: {}", e),
                    })
                    .await;
                return;
            }
        };

        let mut buf = [0u8; 4096];

        loop {
            tokio::select! {
                biased;

                // Schreibe stdin zum PTY (höhere Priorität)
                Some(data) = stdin_rx.recv() => {
                    let fd = async_fd.as_raw_fd();
                    let mut written = 0;
                    while written < data.len() {
                        // Warte bis writeable
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
                                let _ = output_tx_clone
                                    .send(Response::Output {
                                        stream: OutputStream::Stdout,
                                        data: buf[..n as usize].to_vec(),
                                    })
                                    .await;
                                guard.clear_ready();
                            } else if n == 0 {
                                // EOF
                                break;
                            } else {
                                let err = std::io::Error::last_os_error();
                                if err.kind() == std::io::ErrorKind::WouldBlock {
                                    guard.clear_ready();
                                } else if err.raw_os_error() == Some(libc::EIO) {
                                    // PTY closed (child exited)
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

    // Wait für Child-Prozess
    tokio::spawn(async move {
        loop {
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };

            if result == child_pid {
                let code = if libc::WIFEXITED(status) {
                    Some(libc::WEXITSTATUS(status))
                } else {
                    None
                };
                let _ = output_tx.send(Response::Exit { code }).await;
                break;
            } else if result < 0 {
                let _ = output_tx.send(Response::Exit { code: None }).await;
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    });

    Ok(ProcessHandle {
        output_rx,
        stdin_tx,
        resize_tx: Some(resize_tx),
        pid: Some(child_pid),
    })
}
