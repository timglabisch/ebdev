use super::TaskRunnerUI;
use crate::command::{CommandId, CommandResult};
use std::io::{self, Write};

/// Headless UI - leitet Output direkt an stdout weiter
pub struct HeadlessUI {
    current_task: Option<(CommandId, String)>,
    task_starts: std::collections::HashMap<CommandId, (String, std::time::Instant)>,
    current_stage: Option<String>,
    in_parallel: bool,
}

impl HeadlessUI {
    pub fn new() -> Self {
        Self {
            current_task: None,
            task_starts: std::collections::HashMap::new(),
            current_stage: None,
            in_parallel: false,
        }
    }
}

impl Default for HeadlessUI {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRunnerUI for HeadlessUI {
    fn on_task_start(&mut self, id: CommandId, name: &str) {
        self.current_task = Some((id, name.to_string()));
        self.task_starts.insert(id, (name.to_string(), std::time::Instant::now()));
        if self.in_parallel {
            println!("\x1b[1;34m▶ [{}] {}\x1b[0m", id, name);
        } else {
            println!("\x1b[1;34m▶ {}\x1b[0m", name);
        }
    }

    fn on_task_output(&mut self, _id: CommandId, output: &[u8]) {
        let _ = io::stdout().write_all(output);
        let _ = io::stdout().flush();
    }

    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult) {
        let (name, duration) = self.task_starts.remove(&id)
            .map(|(n, t)| (n, t.elapsed().as_secs_f64()))
            .unwrap_or_else(|| ("?".to_string(), 0.0));
        if result.success {
            if self.in_parallel {
                println!("\x1b[1;32m✓ [{}] {} ({:.1}s)\x1b[0m", id, name, duration);
            } else {
                println!("\x1b[1;32m✓ {} ({:.1}s)\x1b[0m", name, duration);
            }
        } else if self.in_parallel {
            eprintln!("\x1b[1;31m✗ [{}] {} failed (exit code {}, {:.1}s)\x1b[0m", id, name, result.exit_code, duration);
        } else {
            eprintln!("\x1b[1;31m✗ {} failed (exit code {}, {:.1}s)\x1b[0m", name, result.exit_code, duration);
        }
        if self.current_task.as_ref().map(|(i, _)| *i) == Some(id) {
            self.current_task = None;
        }
    }

    fn on_task_error(&mut self, id: CommandId, error: &str) {
        let name = self.task_starts.remove(&id)
            .map(|(n, _)| n)
            .unwrap_or_else(|| "?".to_string());
        if self.in_parallel {
            eprintln!("\x1b[1;31m✗ [{}] {} error: {}\x1b[0m", id, name, error);
        } else {
            eprintln!("\x1b[1;31m✗ {} error: {}\x1b[0m", name, error);
        }
        if self.current_task.as_ref().map(|(i, _)| *i) == Some(id) {
            self.current_task = None;
        }
    }

    fn on_parallel_begin(&mut self, count: usize) {
        self.in_parallel = true;
        println!("Running {} tasks in parallel...", count);
    }

    fn on_parallel_end(&mut self) {
        self.in_parallel = false;
    }

    fn on_stage_begin(&mut self, name: &str) {
        if self.current_stage.is_some() {
            println!();
        }
        println!();
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!("  {}", name);
        println!("═══════════════════════════════════════════════════════════════════════════════");
        println!();
        self.current_stage = Some(name.to_string());
    }

    fn on_log(&mut self, message: &str) {
        println!("{}", message);
    }

    fn check_quit(&mut self) -> io::Result<bool> {
        Ok(false)
    }

    fn tick(&mut self) -> io::Result<()> {
        // Yield CPU to avoid busy-loop starving execution threads
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(())
    }
}
