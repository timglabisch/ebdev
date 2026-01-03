use crate::command::{CommandId, CommandRequest, CommandResult, ExecutorMessage};
use crate::ui::TaskRunnerUI;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

/// Event vom PTY-Thread zurück zum Executor
pub enum PtyEvent {
    Output { id: CommandId, data: Vec<u8> },
    Completed { id: CommandId, result: CommandResult },
    Error { id: CommandId, error: String },
}

/// Der Task-Executor - führt Commands aus und kommuniziert über das UI-Trait
pub struct Executor {
    /// Channel um Messages von Deno zu empfangen
    rx: mpsc::UnboundedReceiver<ExecutorMessage>,
    /// Channel um PTY-Events zu empfangen (sync channel für Thread-Kommunikation)
    pty_rx: std_mpsc::Receiver<PtyEvent>,
    pty_tx: std_mpsc::Sender<PtyEvent>,
    /// Pending result callbacks (warten auf Completion)
    pending_results: HashMap<CommandId, tokio::sync::oneshot::Sender<CommandResult>>,
    /// Default working directory
    pub default_cwd: Option<String>,
    /// Terminal-Größe für PTY
    pub rows: u16,
    pub cols: u16,
}

impl Executor {
    pub fn new(rx: mpsc::UnboundedReceiver<ExecutorMessage>, default_cwd: Option<String>) -> Self {
        let (pty_tx, pty_rx) = std_mpsc::channel();
        Self {
            rx,
            pty_rx,
            pty_tx,
            pending_results: HashMap::new(),
            default_cwd,
            rows: 24,
            cols: 80,
        }
    }

    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    /// Hauptschleife - verarbeitet Messages und PTY-Events
    pub fn run<UI: TaskRunnerUI>(&mut self, ui: &mut UI) -> std::io::Result<()> {
        loop {
            // 1. Verarbeite ausstehende Executor-Messages (non-blocking)
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    ExecutorMessage::Execute(request) => {
                        self.execute(request, ui);
                    }
                    ExecutorMessage::ParallelBegin { count } => {
                        ui.on_parallel_begin(count);
                    }
                    ExecutorMessage::ParallelEnd => {
                        ui.on_parallel_end();
                    }
                    ExecutorMessage::StageBegin { name } => {
                        ui.on_stage_begin(&name);
                    }
                    ExecutorMessage::Shutdown => {
                        ui.cleanup()?;
                        return Ok(());
                    }
                }
            }

            // 2. Verarbeite ausstehende PTY-Events (non-blocking)
            while let Ok(event) = self.pty_rx.try_recv() {
                match event {
                    PtyEvent::Output { id, data } => {
                        ui.on_task_output(id, &data);
                    }
                    PtyEvent::Completed { id, result } => {
                        ui.on_task_complete(id, &result);
                        // Sende Result zurück an Deno
                        if let Some(tx) = self.pending_results.remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    PtyEvent::Error { id, error } => {
                        ui.on_task_error(id, &error);
                        // Sende Fehler-Result zurück an Deno
                        if let Some(tx) = self.pending_results.remove(&id) {
                            let _ = tx.send(CommandResult {
                                exit_code: -1,
                                success: false,
                                timed_out: false,
                            });
                        }
                    }
                }
            }

            // 3. UI tick (draw, handle input)
            ui.tick()?;

            // 4. Check quit
            if ui.check_quit()? {
                ui.cleanup()?;
                return Ok(());
            }
        }
    }

    /// Führt einen Command im PTY aus
    fn execute<UI: TaskRunnerUI>(&mut self, request: CommandRequest, ui: &mut UI) {
        let CommandRequest { id, command, result_tx } = request;
        let name = command.display_name();
        let (program, args) = command.to_cmd_args();
        let cwd = command.cwd().or(self.default_cwd.as_deref()).map(|s| s.to_string());
        let env = command.env().cloned();
        let timeout = command.timeout();

        // Speichere den Result-Sender
        self.pending_results.insert(id, result_tx);

        // Benachrichtige UI
        ui.on_task_start(id, &name);

        let rows = self.rows;
        let cols = self.cols;
        let pty_tx = self.pty_tx.clone();

        // Spawn PTY in einem eigenen Thread
        thread::spawn(move || {
            let pty_result = run_with_timeout(program, args, cwd, env, rows, cols, timeout, id, pty_tx.clone());

            match pty_result {
                Ok(result) => {
                    let _ = pty_tx.send(PtyEvent::Completed { id, result });
                }
                Err(error) => {
                    let _ = pty_tx.send(PtyEvent::Error { id, error });
                }
            }
        });
    }
}

fn run_with_timeout(
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    rows: u16,
    cols: u16,
    timeout: Duration,
    id: CommandId,
    pty_tx: std_mpsc::Sender<PtyEvent>,
) -> Result<CommandResult, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(&program);
    for arg in &args {
        cmd.arg(arg);
    }
    if let Some(ref dir) = cwd {
        cmd.cwd(dir);
    }
    if let Some(ref env_vars) = env {
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;

    // Reader-Thread für PTY-Output
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let output_tx = pty_tx.clone();

    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = output_tx.send(PtyEvent::Output {
                        id,
                        data: buf[..n].to_vec(),
                    });
                }
                Err(_) => break,
            }
        }
    });

    // Use a channel to receive the wait result with timeout
    let (wait_tx, wait_rx) = std_mpsc::channel();

    thread::spawn(move || {
        let status = child.wait();
        let _ = wait_tx.send(status);
    });

    // Wait for child with timeout
    match wait_rx.recv_timeout(timeout) {
        Ok(Ok(exit)) => {
            let code = exit.exit_code() as i32;
            Ok(CommandResult {
                exit_code: code,
                success: code == 0,
                timed_out: false,
            })
        }
        Ok(Err(e)) => {
            Err(e.to_string())
        }
        Err(std_mpsc::RecvTimeoutError::Timeout) => {
            // Timeout occurred - we can't easily kill the child from here
            // since it moved into the thread, but the process will be orphaned
            // and cleaned up when the thread drops
            Ok(CommandResult {
                exit_code: -1,
                success: false,
                timed_out: true,
            })
        }
        Err(std_mpsc::RecvTimeoutError::Disconnected) => {
            Err("Child process thread disconnected unexpectedly".to_string())
        }
    }
}
