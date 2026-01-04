use crate::command::{CommandId, CommandRequest, CommandResult, DebugMessage, ExecutorMessage, RegisteredTask, TuiEvent};
use crate::ui::TaskRunnerUI;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Event vom PTY-Thread zurück zum Executor
pub enum PtyEvent {
    Output { id: CommandId, data: Vec<u8> },
    Completed { id: CommandId, result: CommandResult },
    Error { id: CommandId, error: String },
}

/// Debug logger for recording all executor communication
pub struct DebugLogger {
    file: File,
    start_time: Instant,
}

impl DebugLogger {
    pub fn new(path: &PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            file,
            start_time: Instant::now(),
        })
    }

    pub fn log(&mut self, msg: &DebugMessage) {
        let elapsed = self.start_time.elapsed();
        let timestamp = format!("{:.3}s", elapsed.as_secs_f64());
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = writeln!(self.file, "[{}] {}", timestamp, json);
            let _ = self.file.flush();
        }
    }

    pub fn log_raw(&mut self, msg: &str) {
        let elapsed = self.start_time.elapsed();
        let timestamp = format!("{:.3}s", elapsed.as_secs_f64());
        let _ = writeln!(self.file, "[{}] {}", timestamp, msg);
        let _ = self.file.flush();
    }
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
    /// Channel um TUI Events zurück an TypeScript zu senden
    tui_event_tx: mpsc::UnboundedSender<TuiEvent>,
    /// Registrierte Tasks die von der TUI getriggert werden können
    registered_tasks: Vec<RegisteredTask>,
    /// Optional debug logger
    debug_logger: Option<DebugLogger>,
}

impl Executor {
    pub fn new(
        rx: mpsc::UnboundedReceiver<ExecutorMessage>,
        default_cwd: Option<String>,
        tui_event_tx: mpsc::UnboundedSender<TuiEvent>,
    ) -> Self {
        let (pty_tx, pty_rx) = std_mpsc::channel();
        Self {
            rx,
            pty_rx,
            pty_tx,
            pending_results: HashMap::new(),
            default_cwd,
            rows: 24,
            cols: 80,
            tui_event_tx,
            registered_tasks: Vec::new(),
            debug_logger: None,
        }
    }

    /// Enable debug logging to a file
    pub fn with_debug_log(mut self, path: PathBuf) -> Self {
        match DebugLogger::new(&path) {
            Ok(logger) => {
                self.debug_logger = Some(logger);
            }
            Err(e) => {
                eprintln!("Warning: Could not create debug log at {:?}: {}", path, e);
            }
        }
        self
    }

    /// Log a debug message if debug logging is enabled
    fn log_debug(&mut self, msg: DebugMessage) {
        if let Some(ref mut logger) = self.debug_logger {
            logger.log(&msg);
        }
    }

    /// Get the list of registered tasks
    pub fn registered_tasks(&self) -> &[RegisteredTask] {
        &self.registered_tasks
    }

    /// Trigger a task (sends event back to TypeScript)
    pub fn trigger_task(&self, name: &str) {
        let _ = self.tui_event_tx.send(TuiEvent::TaskTriggered {
            name: name.to_string(),
        });
    }

    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
    }

    /// Hauptschleife - verarbeitet Messages und PTY-Events
    pub fn run<UI: TaskRunnerUI>(&mut self, ui: &mut UI) -> std::io::Result<()> {
        if let Some(ref mut logger) = self.debug_logger {
            logger.log_raw("=== Executor started ===");
        }

        loop {
            // 1. Verarbeite ausstehende Executor-Messages (non-blocking)
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    ExecutorMessage::Execute(request) => {
                        self.log_debug(DebugMessage::Execute {
                            id: request.id,
                            command: request.command.clone(),
                        });
                        self.execute(request, ui);
                    }
                    ExecutorMessage::ParallelBegin { count } => {
                        self.log_debug(DebugMessage::ParallelBegin { count });
                        ui.on_parallel_begin(count);
                    }
                    ExecutorMessage::ParallelEnd => {
                        self.log_debug(DebugMessage::ParallelEnd);
                        ui.on_parallel_end();
                    }
                    ExecutorMessage::StageBegin { name } => {
                        self.log_debug(DebugMessage::StageBegin { name: name.clone() });
                        ui.on_stage_begin(&name);
                    }
                    ExecutorMessage::TaskRegister { name, description } => {
                        self.log_debug(DebugMessage::TaskRegister {
                            name: name.clone(),
                            description: description.clone(),
                        });
                        // Remove existing task with same name if any
                        self.registered_tasks.retain(|t| t.name != name);
                        self.registered_tasks.push(RegisteredTask {
                            name: name.clone(),
                            description: description.clone(),
                        });
                        ui.on_task_registered(&name, &description);
                    }
                    ExecutorMessage::TaskUnregister { name } => {
                        self.log_debug(DebugMessage::TaskUnregister { name: name.clone() });
                        self.registered_tasks.retain(|t| t.name != name);
                        ui.on_task_unregistered(&name);
                    }
                    ExecutorMessage::Log { message } => {
                        self.log_debug(DebugMessage::Log { message: message.clone() });
                        ui.on_log(&message);
                    }
                    ExecutorMessage::Shutdown => {
                        self.log_debug(DebugMessage::Shutdown);
                        if let Some(ref mut logger) = self.debug_logger {
                            logger.log_raw(&format!("Shutdown received, auto_quit={}", ui.should_auto_quit()));
                        }
                        // If auto-quit is enabled, exit immediately
                        // Otherwise, keep the UI open until user manually quits
                        if ui.should_auto_quit() {
                            if let Some(ref mut logger) = self.debug_logger {
                                logger.log_raw("Calling cleanup and returning");
                            }
                            ui.cleanup()?;
                            return Ok(());
                        }
                        if let Some(ref mut logger) = self.debug_logger {
                            logger.log_raw("Not auto-quitting, continuing loop");
                        }
                        // Continue running - user will quit manually with q/Esc
                    }
                }
            }

            // 2. Verarbeite ausstehende PTY-Events (non-blocking)
            while let Ok(event) = self.pty_rx.try_recv() {
                match event {
                    PtyEvent::Output { id, data } => {
                        self.log_debug(DebugMessage::PtyOutput {
                            id,
                            data_utf8: String::from_utf8(data.clone()).ok(),
                            data_len: data.len(),
                        });
                        ui.on_task_output(id, &data);
                    }
                    PtyEvent::Completed { id, result } => {
                        self.log_debug(DebugMessage::PtyCompleted {
                            id,
                            result: result.clone(),
                        });
                        ui.on_task_complete(id, &result);
                        // Sende Result zurück an Deno
                        if let Some(tx) = self.pending_results.remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    PtyEvent::Error { id, error } => {
                        self.log_debug(DebugMessage::PtyError {
                            id,
                            error: error.clone(),
                        });
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

            // 4. Check for triggered tasks from Command Palette
            if let Some(task_name) = ui.poll_triggered_task() {
                self.trigger_task(&task_name);
            }

            // 5. Check quit
            if ui.check_quit()? {
                if let Some(ref mut logger) = self.debug_logger {
                    logger.log_raw("User quit via check_quit()");
                }
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
