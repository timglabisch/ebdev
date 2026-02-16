//! ebdev_remote - Bridge-Protokoll für Remote-Command-Execution
//!
//! Dieses Modul definiert das bincode-Protokoll für die Kommunikation
//! zwischen dem Host (ebdev) und der Remote-Binary im Container.
//! Unterstützt Multi-Session mit gleichzeitigen Prozessen und Keep-Alive.

use serde::{Deserialize, Serialize};

pub mod bridge;
pub mod executor;
pub mod local;
pub mod remote;

pub use bridge::run_bridge;
pub use executor::{ExecuteEvent, ExecuteHandle, ExecuteOptions, Executor, ExecutorError};
pub use local::LocalExecutor;
pub use remote::RemoteExecutor;

/// PTY-Konfiguration für interaktive Sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyConfig {
    /// Terminal-Breite in Spalten
    pub cols: u16,
    /// Terminal-Höhe in Zeilen
    pub rows: u16,
}

/// Request vom Host an die Remote-Binary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Führe einen Befehl aus
    Execute {
        /// Session-ID (vom Client generiert, eindeutig)
        session_id: u32,
        /// Programm das ausgeführt werden soll
        program: String,
        /// Argumente für das Programm
        args: Vec<String>,
        /// Arbeitsverzeichnis (optional)
        working_dir: Option<String>,
        /// Umgebungsvariablen (Key-Value Paare)
        env: Vec<(String, String)>,
        /// PTY-Konfiguration für interaktive Sessions (None = kein PTY)
        pty: Option<PtyConfig>,
    },
    /// Stdin-Daten für einen laufenden Prozess
    Stdin {
        /// Session-ID des Zielprozesses
        session_id: u32,
        /// Die Daten die an stdin gesendet werden sollen
        data: Vec<u8>,
    },
    /// Terminal-Größe ändern (nur bei PTY-Sessions)
    Resize {
        /// Session-ID des Zielprozesses
        session_id: u32,
        /// Neue Breite in Spalten
        cols: u16,
        /// Neue Höhe in Zeilen
        rows: u16,
    },
    /// Sende Signal an einen laufenden Prozess
    Signal {
        /// Session-ID des Zielprozesses
        session_id: u32,
        /// Signal-Nummer (z.B. SIGTERM=15, SIGKILL=9, SIGINT=2)
        signal: i32,
    },
    /// Beende einen spezifischen Prozess
    Kill {
        /// Session-ID des zu beendenden Prozesses
        session_id: u32,
    },
    /// Keep-alive Ping
    Ping,
    /// Beende die Remote-Binary (killt auch alle laufenden Prozesse)
    Shutdown,
}

/// Response von der Remote-Binary an den Host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Prozess wurde erfolgreich gestartet
    Started {
        /// Session-ID des gestarteten Prozesses
        session_id: u32,
    },
    /// Output-Chunk (stdout oder stderr)
    Output {
        /// Session-ID des Prozesses
        session_id: u32,
        /// Art des Outputs
        stream: OutputStream,
        /// Die Daten
        data: Vec<u8>,
    },
    /// Prozess wurde beendet
    Exit {
        /// Session-ID des beendeten Prozesses
        session_id: u32,
        /// Exit-Code des Prozesses (None wenn durch Signal beendet)
        code: Option<i32>,
    },
    /// Fehler bei der Ausführung
    Error {
        /// Session-ID (None für globale Fehler)
        session_id: Option<u32>,
        /// Fehlermeldung
        message: String,
    },
    /// Keep-alive Pong (Antwort auf Ping)
    Pong,
    /// Bestätigung dass die Binary bereit ist (mit Protokoll-Version)
    Ready {
        /// Protokoll-Version der Bridge
        protocol_version: u32,
    },
}

/// Art des Output-Streams
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Protokoll-Version für Kompatibilitätsprüfung
/// v4: Ready enthält jetzt protocol_version für Kompatibilitätsprüfung
pub const PROTOCOL_VERSION: u32 = 4;

/// Magic-Bytes für Protokoll-Identifikation
pub const MAGIC: &[u8; 4] = b"EBDV";

/// Serialisiert eine Nachricht mit Längen-Präfix
pub fn encode_message<T: Serialize>(msg: &T) -> Result<Vec<u8>, bincode::Error> {
    let payload = bincode::serialize(msg)?;
    let len = payload.len() as u32;
    let mut result = Vec::with_capacity(4 + payload.len());
    result.extend_from_slice(&len.to_le_bytes());
    result.extend_from_slice(&payload);
    Ok(result)
}

/// Liest eine Nachricht mit Längen-Präfix
/// Gibt (message, bytes_consumed) zurück
pub fn decode_message<T: for<'de> Deserialize<'de>>(
    data: &[u8],
) -> Result<Option<(T, usize)>, bincode::Error> {
    if data.len() < 4 {
        return Ok(None);
    }

    let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

    if data.len() < 4 + len {
        return Ok(None);
    }

    let msg: T = bincode::deserialize(&data[4..4 + len])?;
    Ok(Some((msg, 4 + len)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_request() {
        let req = Request::Execute {
            session_id: 42,
            program: "ls".to_string(),
            args: vec!["-la".to_string()],
            working_dir: Some("/tmp".to_string()),
            env: vec![("FOO".to_string(), "bar".to_string())],
            pty: Some(PtyConfig { cols: 80, rows: 24 }),
        };

        let encoded = encode_message(&req).unwrap();
        let (decoded, consumed): (Request, usize) = decode_message(&encoded).unwrap().unwrap();

        assert_eq!(consumed, encoded.len());
        match decoded {
            Request::Execute {
                session_id,
                program,
                args,
                pty,
                ..
            } => {
                assert_eq!(session_id, 42);
                assert_eq!(program, "ls");
                assert_eq!(args, vec!["-la"]);
                assert!(pty.is_some());
            }
            _ => panic!("Unexpected variant"),
        }
    }

    #[test]
    fn test_roundtrip_response() {
        let resp = Response::Output {
            session_id: 123,
            stream: OutputStream::Stdout,
            data: b"hello world".to_vec(),
        };

        let encoded = encode_message(&resp).unwrap();
        let (decoded, _): (Response, usize) = decode_message(&encoded).unwrap().unwrap();

        match decoded {
            Response::Output {
                session_id,
                stream,
                data,
            } => {
                assert_eq!(session_id, 123);
                assert_eq!(stream, OutputStream::Stdout);
                assert_eq!(data, b"hello world");
            }
            _ => panic!("Unexpected variant"),
        }
    }

    #[test]
    fn test_ping_pong() {
        let ping = Request::Ping;
        let encoded = encode_message(&ping).unwrap();
        let (decoded, _): (Request, usize) = decode_message(&encoded).unwrap().unwrap();
        assert!(matches!(decoded, Request::Ping));

        let pong = Response::Pong;
        let encoded = encode_message(&pong).unwrap();
        let (decoded, _): (Response, usize) = decode_message(&encoded).unwrap().unwrap();
        assert!(matches!(decoded, Response::Pong));
    }

    #[test]
    fn test_kill_request() {
        let req = Request::Kill { session_id: 99 };
        let encoded = encode_message(&req).unwrap();
        let (decoded, _): (Request, usize) = decode_message(&encoded).unwrap().unwrap();

        match decoded {
            Request::Kill { session_id } => {
                assert_eq!(session_id, 99);
            }
            _ => panic!("Unexpected variant"),
        }
    }

    #[test]
    fn test_started_response() {
        let resp = Response::Started { session_id: 1 };
        let encoded = encode_message(&resp).unwrap();
        let (decoded, _): (Response, usize) = decode_message(&encoded).unwrap().unwrap();

        match decoded {
            Response::Started { session_id } => {
                assert_eq!(session_id, 1);
            }
            _ => panic!("Unexpected variant"),
        }
    }
}
