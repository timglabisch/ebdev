use crate::command::{CommandId, CommandRequest, CommandResult, ExecutorMessage, RegisteredTask, TuiEvent};
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
    /// Channel um TUI Events zurück an TypeScript zu senden
    tui_event_tx: mpsc::UnboundedSender<TuiEvent>,
    /// Registrierte Tasks die von der TUI getriggert werden können
    registered_tasks: Vec<RegisteredTask>,
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
                    ExecutorMessage::TaskRegister { name, description } => {
                        // Remove existing task with same name if any
                        self.registered_tasks.retain(|t| t.name != name);
                        self.registered_tasks.push(RegisteredTask {
                            name: name.clone(),
                            description: description.clone(),
                        });
                        ui.on_task_registered(&name, &description);
                    }
                    ExecutorMessage::TaskUnregister { name } => {
                        self.registered_tasks.retain(|t| t.name != name);
                        ui.on_task_unregistered(&name);
                    }
                    ExecutorMessage::Log { message } => {
                        ui.on_log(&message);
                    }
                    ExecutorMessage::Shutdown => {
                        // If auto-quit is enabled, exit immediately
                        // Otherwise, keep the UI open until user manually quits
                        if ui.should_auto_quit() {
                            ui.cleanup()?;
                            return Ok(());
                        }
                        // Continue running - user will quit manually with q/Esc
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

            // 4. Check for triggered tasks from Command Palette
            if let Some(task_name) = ui.poll_triggered_task() {
                self.trigger_task(&task_name);
            }

            // 5. Check quit
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
