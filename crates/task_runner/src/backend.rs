//! Execution Backend - Wrapper um LocalExecutor und RemoteExecutor
//!
//! Dieses Modul bietet eine synchrone Schnittstelle für die Ausführung von Commands,
//! die intern die async Executoren aus ebdev_remote verwendet.

use crate::command::{Command, CommandResult};
use ebdev_remote::{ExecuteEvent, ExecuteOptions, Executor, LocalExecutor, PtyConfig};
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

    /// Führt einen Befehl in einem Docker-Container aus
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
        // Baue docker exec Befehl
        let mut docker_args = vec!["exec".to_string(), "-i".to_string(), "-t".to_string()];

        if let Some(u) = user {
            docker_args.push("-u".to_string());
            docker_args.push(u.to_string());
        }

        if let Some(ref e) = env {
            for (k, v) in e {
                docker_args.push("-e".to_string());
                docker_args.push(format!("{}={}", k, v));
            }
        }

        docker_args.push(container.to_string());
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
