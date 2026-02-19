pub mod headless;
pub mod tui;
pub mod types;
pub mod widgets;

use crate::command::{CommandId, CommandResult};
use std::io;

/// Trait für UI-Interaktionen während der Task-Ausführung.
/// Ermöglicht einheitliche Logik für Headless und TUI.
pub trait TaskRunnerUI {
    /// Wird aufgerufen wenn ein Task startet
    fn on_task_start(&mut self, id: CommandId, name: &str);

    /// Wird aufgerufen wenn ein Task Output produziert (PTY-Daten)
    fn on_task_output(&mut self, id: CommandId, output: &[u8]);

    /// Wird aufgerufen wenn ein Task abgeschlossen ist
    fn on_task_complete(&mut self, id: CommandId, result: &CommandResult);

    /// Wird aufgerufen wenn ein Task fehlschlägt
    fn on_task_error(&mut self, id: CommandId, error: &str);

    /// Wird aufgerufen wenn eine Parallel-Gruppe beginnt
    fn on_parallel_begin(&mut self, count: usize);

    /// Wird aufgerufen wenn eine Parallel-Gruppe endet
    fn on_parallel_end(&mut self);

    /// Wird aufgerufen wenn eine neue Stage beginnt
    /// Kollabiert die vorherige Stage und zeigt den neuen Stage-Header
    fn on_stage_begin(&mut self, name: &str);

    /// Wird aufgerufen wenn ein Task registriert wird (für Command Palette)
    fn on_task_registered(&mut self, _name: &str, _description: &str) {}

    /// Wird aufgerufen wenn ein Task deregistriert wird
    fn on_task_unregistered(&mut self, _name: &str) {}

    /// Gibt zurück ob ein Task getriggert werden soll (von TUI Command Palette)
    fn poll_triggered_task(&mut self) -> Option<String> { None }

    /// Log a message (works correctly in both headless and TUI mode)
    fn on_log(&mut self, message: &str);

    /// Returns whether the UI should auto-quit when tasks complete
    fn should_auto_quit(&self) -> bool { true }

    /// Prüft ob der Benutzer abbrechen möchte
    fn check_quit(&mut self) -> io::Result<bool>;

    /// Wird in der Hauptschleife aufgerufen (TUI: draw + events, Headless: noop/sleep)
    fn tick(&mut self) -> io::Result<()>;

    /// Setzt die Terminal-Größe für PTY-Output
    fn set_terminal_size(&mut self, _rows: u16, _cols: u16) {}

    /// Suspend the UI (for interactive commands that need real terminal access)
    fn suspend(&mut self) -> io::Result<()> { Ok(()) }

    /// Resume the UI after an interactive command finishes
    fn resume(&mut self) -> io::Result<()> { Ok(()) }

    /// Wird am Ende aufgerufen für Cleanup
    fn cleanup(&mut self) -> io::Result<()> { Ok(()) }
}

// Re-exports
pub use headless::HeadlessUI;
pub use tui::TuiUI;
