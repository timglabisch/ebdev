//! Executor Trait für lokale und remote Prozessausführung

use crate::{OutputStream, PtyConfig};
use tokio::sync::{mpsc, oneshot};

/// Optionen für die Prozessausführung
#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    pub program: String,
    pub args: Vec<String>,
    pub workdir: Option<String>,
    pub env: Vec<(String, String)>,
    pub pty: Option<PtyConfig>,
}

/// Events die während der Ausführung auftreten
#[derive(Debug)]
pub enum ExecuteEvent {
    /// Output von stdout oder stderr
    Output { stream: OutputStream, data: Vec<u8> },
    /// Prozess beendet
    Exit { code: Option<i32> },
}

/// Handle für einen laufenden Prozess
pub struct ExecuteHandle {
    /// Sender für stdin-Daten
    pub stdin_tx: mpsc::Sender<Vec<u8>>,
    /// Sender für resize-Events (nur bei PTY)
    pub resize_tx: Option<mpsc::Sender<(u16, u16)>>,
    /// Sender zum Killen des Prozesses
    pub kill_tx: Option<oneshot::Sender<()>>,
}

/// Fehlertyp für Executor-Operationen
#[derive(Debug)]
pub enum ExecutorError {
    Io(std::io::Error),
    Spawn(String),
    Protocol(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::Io(e) => write!(f, "IO error: {}", e),
            ExecutorError::Spawn(e) => write!(f, "Spawn error: {}", e),
            ExecutorError::Protocol(e) => write!(f, "Protocol error: {}", e),
        }
    }
}

impl std::error::Error for ExecutorError {}

impl From<std::io::Error> for ExecutorError {
    fn from(e: std::io::Error) -> Self {
        ExecutorError::Io(e)
    }
}

/// Trait für Prozessausführung (lokal oder remote)
pub trait Executor {
    /// Startet einen Prozess und gibt ein Handle zurück
    ///
    /// Events werden über den `event_tx` Channel gesendet.
    /// Das Handle kann verwendet werden um stdin zu senden und resize durchzuführen.
    fn execute(
        &mut self,
        options: ExecuteOptions,
        event_tx: mpsc::Sender<ExecuteEvent>,
    ) -> impl std::future::Future<Output = Result<ExecuteHandle, ExecutorError>> + Send;
}
