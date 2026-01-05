//! Bridge-Implementierung mit LocalExecutor

use crate::executor::{ExecuteEvent, ExecuteHandle, ExecuteOptions, Executor};
use crate::local::LocalExecutor;
use crate::{decode_message, encode_message, Request, Response, MAGIC, PROTOCOL_VERSION};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// Fehlertyp für Bridge-Operationen
#[derive(Debug)]
pub enum BridgeError {
    Io(std::io::Error),
    Bincode(bincode::Error),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Io(e) => write!(f, "IO error: {}", e),
            BridgeError::Bincode(e) => write!(f, "Bincode error: {}", e),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self {
        BridgeError::Io(e)
    }
}

impl From<bincode::Error> for BridgeError {
    fn from(e: bincode::Error) -> Self {
        BridgeError::Bincode(e)
    }
}

/// Session mit Handle und Event-Receiver
struct Session {
    handle: ExecuteHandle,
}

/// Nachricht vom Prozess an den Main-Loop
enum SessionEvent {
    Event { session_id: u32, event: ExecuteEvent },
}

/// Startet den Bridge-Modus
pub async fn run_bridge() -> Result<(), BridgeError> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    // Sende Magic-Bytes und Ready-Signal mit Protokoll-Version
    stdout.write_all(MAGIC).await?;
    let ready_msg = encode_message(&Response::Ready { protocol_version: PROTOCOL_VERSION })?;
    stdout.write_all(&ready_msg).await?;
    stdout.flush().await?;

    let mut buffer = Vec::new();
    let mut read_buf = [0u8; 4096];

    // Session-Management
    let mut sessions: HashMap<u32, Session> = HashMap::new();
    let mut executor = LocalExecutor::new();

    // Gemeinsamer Event-Channel für alle Prozesse
    let (event_tx, mut event_rx) = mpsc::channel::<SessionEvent>(256);

    loop {
        tokio::select! {
            biased;

            // Handle Prozess-Events von allen Sessions
            Some(msg) = event_rx.recv() => {
                let SessionEvent::Event { session_id, event } = msg;

                let response = match event {
                    ExecuteEvent::Output { stream, data } => {
                        Response::Output { session_id, stream, data }
                    }
                    ExecuteEvent::Exit { code } => {
                        sessions.remove(&session_id);
                        Response::Exit { session_id, code }
                    }
                };

                let encoded = encode_message(&response)?;
                stdout.write_all(&encoded).await?;
                stdout.flush().await?;
            }

            // Lese von Host-stdin (Requests)
            result = stdin.read(&mut read_buf) => {
                let n = result?;
                if n == 0 {
                    // EOF - Beende alle Sessions und exit
                    sessions.clear();
                    break;
                }
                buffer.extend_from_slice(&read_buf[..n]);

                // Verarbeite alle vollständigen Nachrichten
                while let Some((request, consumed)) = decode_message::<Request>(&buffer)? {
                    buffer.drain(..consumed);

                    match request {
                        Request::Execute {
                            session_id,
                            program,
                            args,
                            working_dir,
                            env,
                            pty,
                        } => {
                            // Prüfe ob Session-ID schon existiert
                            if sessions.contains_key(&session_id) {
                                let error = Response::Error {
                                    session_id: Some(session_id),
                                    message: format!("Session {} already exists", session_id),
                                };
                                let encoded = encode_message(&error)?;
                                stdout.write_all(&encoded).await?;
                                stdout.flush().await?;
                                continue;
                            }

                            // Event-Channel für diese Session
                            let (session_event_tx, mut session_event_rx) = mpsc::channel(64);

                            // Forward events to main channel
                            let main_tx = event_tx.clone();
                            tokio::spawn(async move {
                                while let Some(event) = session_event_rx.recv().await {
                                    if main_tx.send(SessionEvent::Event { session_id, event }).await.is_err() {
                                        break;
                                    }
                                }
                            });

                            let options = ExecuteOptions {
                                program,
                                args,
                                workdir: working_dir,
                                env,
                                pty,
                            };

                            match executor.execute(options, session_event_tx).await {
                                Ok(handle) => {
                                    sessions.insert(session_id, Session { handle });

                                    // Sende Started Response
                                    let started = Response::Started { session_id };
                                    let encoded = encode_message(&started)?;
                                    stdout.write_all(&encoded).await?;
                                    stdout.flush().await?;
                                }
                                Err(e) => {
                                    let error = Response::Error {
                                        session_id: Some(session_id),
                                        message: format!("Failed to start process: {}", e),
                                    };
                                    let encoded = encode_message(&error)?;
                                    stdout.write_all(&encoded).await?;
                                    stdout.flush().await?;
                                }
                            }
                        }
                        Request::Stdin { session_id, data } => {
                            if let Some(session) = sessions.get(&session_id) {
                                let _ = session.handle.stdin_tx.send(data).await;
                            }
                        }
                        Request::Resize { session_id, cols, rows } => {
                            if let Some(session) = sessions.get(&session_id) {
                                if let Some(ref resize_tx) = session.handle.resize_tx {
                                    let _ = resize_tx.send((cols, rows)).await;
                                }
                            }
                        }
                        Request::Signal { session_id, signal } => {
                            // Signal-Handling würde PID benötigen - für jetzt ignorieren
                            // In einer vollständigen Implementierung würde ExecuteHandle die PID enthalten
                            let _ = (session_id, signal);
                        }
                        Request::Kill { session_id } => {
                            // Kill the process by sending kill signal
                            if let Some(session) = sessions.remove(&session_id) {
                                if let Some(kill_tx) = session.handle.kill_tx {
                                    let _ = kill_tx.send(());
                                }
                            }
                        }
                        Request::Ping => {
                            let pong = Response::Pong;
                            let encoded = encode_message(&pong)?;
                            stdout.write_all(&encoded).await?;
                            stdout.flush().await?;
                        }
                        Request::Shutdown => {
                            sessions.clear();
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
