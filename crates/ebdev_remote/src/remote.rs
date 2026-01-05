//! Remote Executor - führt Befehle über Docker Bridge aus
//!
//! Dieser Executor verbindet sich zu einem Docker-Container,
//! kopiert die Bridge-Binary falls nötig, und führt Befehle
//! über das bincode-Protokoll aus.

use crate::{
    decode_message, encode_message, ExecuteEvent, ExecuteHandle, ExecuteOptions, Executor,
    ExecutorError, Request, Response, MAGIC, PROTOCOL_VERSION,
};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

/// Remote Executor - führt Befehle über Docker Bridge aus
pub struct RemoteExecutor {
    #[allow(dead_code)]
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    buffer: Vec<u8>,
    session_counter: u32,
    container: String,
}

/// Request-Typ für den Bridge-Writer
enum BridgeRequest {
    Stdin { session_id: u32, data: Vec<u8> },
    Resize { session_id: u32, cols: u16, rows: u16 },
}

impl RemoteExecutor {
    /// Container-Name für den dieser Executor verbunden ist
    pub fn container(&self) -> &str {
        &self.container
    }

    /// Verbindet sich zu einem Docker-Container
    ///
    /// Kopiert die Bridge-Binary falls nötig und startet sie.
    pub async fn connect(container: &str) -> Result<Self, ExecutorError> {
        let container_binary_path = "/tmp/ebdev";

        // Try to start existing binary first
        let existing_result = Self::try_start_bridge(container, container_binary_path).await;

        match existing_result {
            Ok((child, stdin, stdout, buffer, version)) if version == PROTOCOL_VERSION => {
                // Binary exists and has correct version
                return Ok(Self {
                    child,
                    stdin: Some(stdin),
                    stdout: Some(stdout),
                    buffer,
                    session_counter: 0,
                    container: container.to_string(),
                });
            }
            Ok((mut child, _, _, _, version)) => {
                // Binary exists but wrong version - kill it and copy new one
                let _ = child.kill().await;
                eprintln!(
                    "Bridge version mismatch (got {}, need {}), updating...",
                    version, PROTOCOL_VERSION
                );
            }
            Err(_) => {
                // Binary doesn't exist or failed to start
            }
        }

        // Copy binary to container
        let binary_path = find_linux_binary()?;
        let cp_status = Command::new("docker")
            .args([
                "cp",
                &binary_path.to_string_lossy(),
                &format!("{}:{}", container, container_binary_path),
            ])
            .status()
            .await
            .map_err(|e| ExecutorError::Io(e))?;

        if !cp_status.success() {
            return Err(ExecutorError::Spawn("Failed to copy binary to container".into()));
        }

        // Make executable
        let chmod_status = Command::new("docker")
            .args(["exec", container, "chmod", "+x", container_binary_path])
            .status()
            .await
            .map_err(|e| ExecutorError::Io(e))?;

        if !chmod_status.success() {
            return Err(ExecutorError::Spawn("Failed to make binary executable".into()));
        }

        // Start bridge
        let (child, stdin, stdout, buffer, version) =
            Self::try_start_bridge(container, container_binary_path).await?;

        if version != PROTOCOL_VERSION {
            return Err(ExecutorError::Protocol(format!(
                "Bridge protocol version mismatch after copy (got {}, need {})",
                version, PROTOCOL_VERSION
            )));
        }

        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: Some(stdout),
            buffer,
            session_counter: 0,
            container: container.to_string(),
        })
    }

    /// Try to start the bridge and get version
    async fn try_start_bridge(
        container: &str,
        binary_path: &str,
    ) -> Result<(Child, ChildStdin, ChildStdout, Vec<u8>, u32), ExecutorError> {
        use std::process::Stdio;

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
        tokio::time::timeout(std::time::Duration::from_secs(2), stdout.read_exact(&mut magic_buf))
            .await
            .map_err(|_| ExecutorError::Protocol("Timeout waiting for bridge magic".into()))??;

        if &magic_buf != MAGIC {
            return Err(ExecutorError::Protocol("Invalid magic bytes from bridge".into()));
        }

        let mut buffer = Vec::new();
        let mut read_buf = [0u8; 4096];

        // Wait for Ready with timeout
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(ExecutorError::Protocol("Timeout waiting for bridge ready".into()));
            }

            let n = tokio::time::timeout(remaining, stdout.read(&mut read_buf))
                .await
                .map_err(|_| ExecutorError::Protocol("Timeout reading from bridge".into()))??;

            if n == 0 {
                return Err(ExecutorError::Protocol("Bridge closed unexpectedly".into()));
            }
            buffer.extend_from_slice(&read_buf[..n]);

            if let Some((response, consumed)) =
                decode_message::<Response>(&buffer).map_err(|e| ExecutorError::Protocol(e.to_string()))?
            {
                buffer.drain(..consumed);
                match response {
                    Response::Ready { protocol_version } => {
                        return Ok((child, stdin, stdout, buffer, protocol_version));
                    }
                    _ => {
                        return Err(ExecutorError::Protocol(format!(
                            "Expected Ready, got {:?}",
                            response
                        )));
                    }
                }
            }
        }
    }
}

impl Executor for RemoteExecutor {
    async fn execute(
        &mut self,
        options: ExecuteOptions,
        event_tx: mpsc::Sender<ExecuteEvent>,
    ) -> Result<ExecuteHandle, ExecutorError> {
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
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| ExecutorError::Protocol("stdin already taken".into()))?;
        stdin.write_all(&msg).await?;
        stdin.flush().await?;

        // Wait for Started
        let stdout = self
            .stdout
            .as_mut()
            .ok_or_else(|| ExecutorError::Protocol("stdout already taken".into()))?;
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
                    _ => {
                        return Err(ExecutorError::Protocol(format!(
                            "Expected Started, got {:?}",
                            response
                        )));
                    }
                }
            }
        }

        // Channels for user-facing handle
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(16);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(4);
        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();

        // Internal channel for bridge requests
        let (bridge_tx, mut bridge_rx) = mpsc::channel::<BridgeRequest>(32);

        // Forward stdin to bridge channel
        let bridge_tx_stdin = bridge_tx.clone();
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                if bridge_tx_stdin
                    .send(BridgeRequest::Stdin { session_id, data })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Forward resize to bridge channel
        let bridge_tx_resize = bridge_tx;
        tokio::spawn(async move {
            while let Some((cols, rows)) = resize_rx.recv().await {
                if bridge_tx_resize
                    .send(BridgeRequest::Resize {
                        session_id,
                        cols,
                        rows,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Take stdin and stdout for the I/O tasks
        let mut stdin = self
            .stdin
            .take()
            .ok_or_else(|| ExecutorError::Protocol("stdin already taken".into()))?;
        let mut stdout = self
            .stdout
            .take()
            .ok_or_else(|| ExecutorError::Protocol("stdout already taken".into()))?;
        let mut buffer = std::mem::take(&mut self.buffer);

        // Single writer task for all bridge requests (including kill)
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    req = bridge_rx.recv() => {
                        match req {
                            Some(BridgeRequest::Stdin { session_id, data }) => {
                                let request = Request::Stdin { session_id, data };
                                if let Ok(msg) = encode_message(&request) {
                                    if stdin.write_all(&msg).await.is_err() { break; }
                                    if stdin.flush().await.is_err() { break; }
                                }
                            }
                            Some(BridgeRequest::Resize { session_id, cols, rows }) => {
                                let request = Request::Resize { session_id, cols, rows };
                                if let Ok(msg) = encode_message(&request) {
                                    if stdin.write_all(&msg).await.is_err() { break; }
                                    if stdin.flush().await.is_err() { break; }
                                }
                            }
                            None => break,
                        }
                    }
                    _ = &mut kill_rx => {
                        // Send kill request
                        let request = Request::Kill { session_id };
                        if let Ok(msg) = encode_message(&request) {
                            let _ = stdin.write_all(&msg).await;
                            let _ = stdin.flush().await;
                        }
                        break;
                    }
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

                        while let Ok(Some((response, consumed))) = decode_message::<Response>(&buffer)
                        {
                            buffer.drain(..consumed);

                            let event = match response {
                                Response::Output {
                                    session_id: sid,
                                    stream,
                                    data,
                                } if sid == session_id => Some(ExecuteEvent::Output { stream, data }),
                                Response::Exit {
                                    session_id: sid,
                                    code,
                                } if sid == session_id => Some(ExecuteEvent::Exit { code }),
                                _ => None,
                            };

                            if let Some(evt) = event {
                                let is_exit = matches!(evt, ExecuteEvent::Exit { .. });
                                if event_tx.send(evt).await.is_err() {
                                    return;
                                }
                                if is_exit {
                                    return;
                                }
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
            kill_tx: Some(kill_tx),
        })
    }
}

/// Find the Linux bridge binary (built via make build-linux)
pub fn find_linux_binary() -> Result<PathBuf, ExecutorError> {
    // Look in target/linux relative to current dir or executable
    let candidates = [
        PathBuf::from("target/linux/ebdev-bridge"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../linux/ebdev-bridge")))
            .unwrap_or_default(),
        // Also check relative to workspace root
        PathBuf::from(".rust_apps/ebdev/target/linux/ebdev-bridge"),
    ];

    for path in candidates {
        if path.exists() {
            return Ok(path);
        }
    }

    Err(ExecutorError::Spawn(
        "Linux bridge binary not found. Run 'make build-linux' first.\n\
         Expected at: target/linux/ebdev-bridge"
            .into(),
    ))
}
