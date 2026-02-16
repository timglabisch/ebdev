//! Execution Backend - Wrapper um LocalExecutor und RemoteExecutor
//!
//! Dieses Modul bietet eine synchrone Schnittstelle für die Ausführung von Commands,
//! die intern die async Executoren aus ebdev_remote verwendet.

use crate::command::{Command, CommandResult};
use ebdev_remote::{ExecuteEvent, ExecuteOptions, Executor, LocalExecutor, PtyConfig, RemoteExecutor};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

/// Event vom Backend an den Executor
pub enum BackendEvent {
    Output(Vec<u8>),
    Completed(CommandResult),
    Error(String),
}

/// Execution Backend - verwaltet Command-Ausführung
pub struct ExecutionBackend {
    runtime: Runtime,
}

impl ExecutionBackend {
    pub fn new() -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| format!("Failed to create tokio runtime: {}", e))?;
        Ok(Self { runtime })
    }

    /// Führt einen Command aus und sendet Events über den Callback
    ///
    /// Diese Methode blockiert bis der Command beendet ist.
    pub fn execute(
        &mut self,
        command: &Command,
        default_cwd: Option<&str>,
        rows: u16,
        cols: u16,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        match command {
            Command::Exec { cmd, cwd, env, .. } => {
                self.execute_local(
                    &cmd[0],
                    &cmd[1..],
                    cwd.as_deref().or(default_cwd),
                    env.clone(),
                    Some(PtyConfig { cols, rows }),
                    timeout,
                    event_tx,
                )
            }
            Command::Shell { script, cwd, env, .. } => {
                self.execute_local(
                    "sh",
                    &["-c".to_string(), script.clone()],
                    cwd.as_deref().or(default_cwd),
                    env.clone(),
                    Some(PtyConfig { cols, rows }),
                    timeout,
                    event_tx,
                )
            }
            Command::DockerExec {
                container,
                cmd,
                user,
                env,
                ..
            } => {
                self.execute_docker_exec(container, cmd, user.as_deref(), env.clone(), rows, cols, timeout, event_tx)
            }
            Command::DockerRun {
                image,
                cmd,
                volumes,
                workdir,
                network,
                env,
                ..
            } => {
                // DockerRun wird lokal ausgeführt (docker run --rm ...)
                self.execute_docker_run(
                    image,
                    cmd,
                    volumes.as_ref(),
                    workdir.as_deref(),
                    network.as_deref(),
                    env.clone(),
                    rows,
                    cols,
                    timeout,
                    event_tx,
                )
            }
            Command::WasmRemote {
                container,
                module,
                args,
                env,
                cwd,
                ..
            } => {
                self.execute_wasm_remote(container, module, args, env.clone(), cwd.clone(), timeout, event_tx)
            }
            Command::WasmExec {
                module,
                args,
                env,
                cwd,
                ..
            } => {
                self.execute_wasm_local(module, args, env.clone(), cwd.clone(), timeout, event_tx)
            }
        }
    }

    /// Führt einen lokalen Befehl aus
    fn execute_local(
        &mut self,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        env: Option<std::collections::HashMap<String, String>>,
        pty: Option<PtyConfig>,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        let options = ExecuteOptions {
            program: program.to_string(),
            args: args.to_vec(),
            workdir: cwd.map(|s| s.to_string()),
            env: env
                .map(|e| e.into_iter().collect())
                .unwrap_or_default(),
            pty,
        };

        let mut executor = LocalExecutor::new();
        self.run_executor(&mut executor, options, timeout, event_tx)
    }

    /// Führt einen Befehl in einem Docker-Container aus (über Bridge)
    fn execute_docker_exec(
        &mut self,
        container: &str,
        cmd: &[String],
        user: Option<&str>,
        env: Option<std::collections::HashMap<String, String>>,
        rows: u16,
        cols: u16,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        if cmd.is_empty() {
            let _ = event_tx.send(BackendEvent::Error("Empty command".to_string()));
            return Err("Empty command".to_string());
        }

        // Build the command - if user is specified, use su to switch
        // Note: This requires the bridge to run as root (or a user that can su).
        // The original docker exec -u approach is not possible here since the bridge
        // is already running. We use su inside the container instead.
        let (program, args) = if let Some(u) = user {
            // Use su to run as different user: su user -c "command"
            (
                "su".to_string(),
                vec![
                    u.to_string(),
                    "-c".to_string(),
                    cmd.join(" "),
                ],
            )
        } else {
            (cmd[0].clone(), cmd[1..].to_vec())
        };

        let options = ExecuteOptions {
            program,
            args,
            workdir: None,
            env: env
                .map(|e| e.into_iter().collect())
                .unwrap_or_default(),
            pty: Some(PtyConfig { cols, rows }),
        };

        // Connect to container via bridge and execute
        let container = container.to_string();
        self.runtime.block_on(async {
            let mut executor = match RemoteExecutor::connect(&container).await {
                Ok(e) => e,
                Err(e) => {
                    let _ = event_tx.send(BackendEvent::Error(format!("Failed to connect to container: {}", e)));
                    return Err(format!("Failed to connect to container: {}", e));
                }
            };

            let (tx, mut rx) = mpsc::channel::<ExecuteEvent>(64);

            // Start the process
            let handle = match executor.execute(options, tx).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = event_tx.send(BackendEvent::Error(e.to_string()));
                    return Err(e.to_string());
                }
            };

            // Collect events with timeout
            let timeout_at = tokio::time::Instant::now() + timeout;
            let mut exit_code = None;
            let mut timed_out = false;

            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(ExecuteEvent::Output { stream: _, data }) => {
                                let _ = event_tx.send(BackendEvent::Output(data));
                            }
                            Some(ExecuteEvent::Exit { code }) => {
                                exit_code = code;
                                break;
                            }
                            None => {
                                // Channel closed
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(timeout_at) => {
                        timed_out = true;
                        // Kill the process
                        if let Some(kill_tx) = handle.kill_tx {
                            let _ = kill_tx.send(());
                        }
                        break;
                    }
                }
            }

            let result = CommandResult {
                exit_code: exit_code.unwrap_or(-1),
                success: exit_code == Some(0),
                timed_out,
            };

            let _ = event_tx.send(BackendEvent::Completed(result));
            Ok(())
        })
    }

    /// Führt docker run aus
    fn execute_docker_run(
        &mut self,
        image: &str,
        cmd: &[String],
        volumes: Option<&Vec<String>>,
        workdir: Option<&str>,
        network: Option<&str>,
        env: Option<std::collections::HashMap<String, String>>,
        rows: u16,
        cols: u16,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        let mut docker_args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-i".to_string(),
            "-t".to_string(),
        ];

        if let Some(vols) = volumes {
            for v in vols {
                docker_args.push("-v".to_string());
                docker_args.push(v.clone());
            }
        }

        if let Some(w) = workdir {
            docker_args.push("-w".to_string());
            docker_args.push(w.to_string());
        }

        if let Some(n) = network {
            docker_args.push("--network".to_string());
            docker_args.push(n.to_string());
        }

        if let Some(ref e) = env {
            for (k, v) in e {
                docker_args.push("-e".to_string());
                docker_args.push(format!("{}={}", k, v));
            }
        }

        docker_args.push(image.to_string());
        docker_args.extend(cmd.iter().cloned());

        let options = ExecuteOptions {
            program: "docker".to_string(),
            args: docker_args,
            workdir: None,
            env: vec![],
            pty: Some(PtyConfig { cols, rows }),
        };

        let mut executor = LocalExecutor::new();
        self.run_executor(&mut executor, options, timeout, event_tx)
    }

    /// Führt ein WASM Modul remote in einem Container aus
    fn execute_wasm_remote(
        &mut self,
        container: &str,
        module: &[u8],
        args: &[String],
        env: Option<std::collections::HashMap<String, String>>,
        cwd: Option<String>,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        let container = container.to_string();
        let module = module.to_vec();
        let args = args.to_vec();
        let env_vec: Vec<(String, String)> = env
            .map(|e| e.into_iter().collect())
            .unwrap_or_default();

        self.runtime.block_on(async {
            let mut executor = match RemoteExecutor::connect(&container).await {
                Ok(e) => e,
                Err(e) => {
                    let _ = event_tx.send(BackendEvent::Error(format!("Failed to connect to container: {}", e)));
                    return Err(format!("Failed to connect to container: {}", e));
                }
            };

            // Send WasmExec request
            let session_id = 1u32;
            let request = ebdev_remote::Request::WasmExec {
                session_id,
                module,
                args,
                env: env_vec,
                working_dir: cwd,
            };
            let msg = ebdev_remote::encode_message(&request)
                .map_err(|e| format!("Failed to encode request: {}", e))?;

            let stdin = executor.stdin_mut()
                .ok_or_else(|| "stdin not available".to_string())?;
            use tokio::io::AsyncWriteExt;
            stdin.write_all(&msg).await.map_err(|e| format!("Write error: {}", e))?;
            stdin.flush().await.map_err(|e| format!("Flush error: {}", e))?;

            // Wait for Started
            let stdout = executor.stdout_mut()
                .ok_or_else(|| "stdout not available".to_string())?;
            let mut buffer = Vec::new();
            let mut read_buf = [0u8; 4096];
            use tokio::io::AsyncReadExt;

            loop {
                let n = stdout.read(&mut read_buf).await.map_err(|e| format!("Read error: {}", e))?;
                if n == 0 {
                    return Err("Bridge closed unexpectedly".to_string());
                }
                buffer.extend_from_slice(&read_buf[..n]);

                if let Some((response, consumed)) = ebdev_remote::decode_message::<ebdev_remote::Response>(&buffer)
                    .map_err(|e| format!("Decode error: {}", e))?
                {
                    buffer.drain(..consumed);
                    match response {
                        ebdev_remote::Response::Started { .. } => break,
                        ebdev_remote::Response::Error { message, .. } => return Err(message),
                        _ => return Err(format!("Expected Started, got {:?}", response)),
                    }
                }
            }

            // Stream Output/Exit events with timeout
            let timeout_at = tokio::time::Instant::now() + timeout;
            let mut exit_code = None;
            let mut timed_out = false;

            loop {
                tokio::select! {
                    result = stdout.read(&mut read_buf) => {
                        let n = result.map_err(|e| format!("Read error: {}", e))?;
                        if n == 0 { break; }
                        buffer.extend_from_slice(&read_buf[..n]);

                        while let Some((response, consumed)) = ebdev_remote::decode_message::<ebdev_remote::Response>(&buffer)
                            .map_err(|e| format!("Decode error: {}", e))?
                        {
                            buffer.drain(..consumed);
                            match response {
                                ebdev_remote::Response::Output { data, .. } => {
                                    let _ = event_tx.send(BackendEvent::Output(data));
                                }
                                ebdev_remote::Response::Exit { code, .. } => {
                                    exit_code = code;
                                    break;
                                }
                                _ => {}
                            }
                        }
                        if exit_code.is_some() { break; }
                    }
                    _ = tokio::time::sleep_until(timeout_at) => {
                        timed_out = true;
                        break;
                    }
                }
            }

            let result = CommandResult {
                exit_code: exit_code.unwrap_or(-1),
                success: exit_code == Some(0),
                timed_out,
            };
            let _ = event_tx.send(BackendEvent::Completed(result));
            Ok(())
        })
    }

    /// Führt ein WASM Modul lokal aus
    #[cfg(feature = "wasm-runtime")]
    fn execute_wasm_local(
        &mut self,
        module: &[u8],
        args: &[String],
        env: Option<std::collections::HashMap<String, String>>,
        cwd: Option<String>,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        let module = module.to_vec();
        let args = args.to_vec();
        let env_vec: Vec<(String, String)> = env
            .map(|e| e.into_iter().collect())
            .unwrap_or_default();

        let (output_tx, output_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        // Run WASM in blocking thread
        let wasm_handle = std::thread::spawn(move || {
            ebdev_remote::wasm_host::run_wasm_module(
                &module,
                args,
                env_vec,
                cwd,
                output_tx,
            )
        });

        // Forward output with timeout
        let deadline = std::time::Instant::now() + timeout;
        let mut timed_out = false;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                timed_out = true;
                break;
            }

            match output_rx.recv_timeout(remaining) {
                Ok(data) => {
                    let _ = event_tx.send(BackendEvent::Output(data));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }

        let exit_code = if timed_out {
            -1
        } else {
            wasm_handle.join().unwrap_or(1)
        };

        let result = CommandResult {
            exit_code,
            success: exit_code == 0,
            timed_out,
        };
        let _ = event_tx.send(BackendEvent::Completed(result));
        Ok(())
    }

    #[cfg(not(feature = "wasm-runtime"))]
    fn execute_wasm_local(
        &mut self,
        _module: &[u8],
        _args: &[String],
        _env: Option<std::collections::HashMap<String, String>>,
        _cwd: Option<String>,
        _timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String> {
        let _ = event_tx.send(BackendEvent::Error("WASM runtime not enabled in this build".to_string()));
        Err("WASM runtime not enabled".to_string())
    }

    /// Führt einen Executor aus und sendet Events
    fn run_executor<E: Executor + Send + 'static>(
        &mut self,
        executor: &mut E,
        options: ExecuteOptions,
        timeout: Duration,
        event_tx: std_mpsc::Sender<BackendEvent>,
    ) -> Result<(), String>
    where
        E: Executor,
    {
        // Wir müssen die Runtime blockend verwenden
        let result = self.runtime.block_on(async {
            let (tx, mut rx) = mpsc::channel::<ExecuteEvent>(64);

            // Starte den Prozess
            let handle = match executor.execute(options, tx).await {
                Ok(h) => h,
                Err(e) => {
                    let _ = event_tx.send(BackendEvent::Error(e.to_string()));
                    return Err(e.to_string());
                }
            };

            // Sammle Events mit Timeout
            let timeout_at = tokio::time::Instant::now() + timeout;
            let mut exit_code = None;
            let mut timed_out = false;

            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(ExecuteEvent::Output { stream: _, data }) => {
                                let _ = event_tx.send(BackendEvent::Output(data));
                            }
                            Some(ExecuteEvent::Exit { code }) => {
                                exit_code = code;
                                break;
                            }
                            None => {
                                // Channel closed
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep_until(timeout_at) => {
                        timed_out = true;
                        // Kill the process
                        if let Some(kill_tx) = handle.kill_tx {
                            let _ = kill_tx.send(());
                        }
                        break;
                    }
                }
            }

            let result = CommandResult {
                exit_code: exit_code.unwrap_or(-1),
                success: exit_code == Some(0),
                timed_out,
            };

            let _ = event_tx.send(BackendEvent::Completed(result.clone()));
            Ok(())
        });

        result
    }
}

impl Default for ExecutionBackend {
    fn default() -> Self {
        Self::new().expect("Failed to create ExecutionBackend")
    }
}
