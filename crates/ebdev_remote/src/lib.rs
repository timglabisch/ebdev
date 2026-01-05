//! ebdev_remote - Bridge-Protokoll für Remote-Command-Execution
//!
//! Dieses Modul definiert das bincode-Protokoll für die Kommunikation
//! zwischen dem Host (ebdev) und der Remote-Binary im Container.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Request vom Host an die Remote-Binary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Führe einen Befehl aus
    Execute {
        /// Programm das ausgeführt werden soll
        program: String,
        /// Argumente für das Programm
        args: Vec<String>,
        /// Arbeitsverzeichnis (optional)
        working_dir: Option<String>,
        /// Umgebungsvariablen (Key-Value Paare)
        env: Vec<(String, String)>,
    },
    /// Beende die Remote-Binary
    Shutdown,
}

/// Response von der Remote-Binary an den Host
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Output-Chunk (stdout oder stderr)
    Output {
        /// Art des Outputs
        stream: OutputStream,
        /// Die Daten
        data: Vec<u8>,
    },
    /// Prozess wurde beendet
    Exit {
        /// Exit-Code des Prozesses (None wenn durch Signal beendet)
        code: Option<i32>,
    },
    /// Fehler bei der Ausführung
    Error {
        /// Fehlermeldung
        message: String,
    },
    /// Bestätigung dass die Binary bereit ist
    Ready,
}

/// Art des Output-Streams
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Protokoll-Version für Kompatibilitätsprüfung
pub const PROTOCOL_VERSION: u32 = 1;

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

/// Startet den Bridge-Modus (wird aufgerufen wenn ebdev --remote-bridge läuft)
/// Kommuniziert über stdin/stdout mit dem Host
pub fn run_bridge() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    // Sende Magic-Bytes und Ready-Signal
    stdout.write_all(MAGIC)?;
    let ready_msg = encode_message(&Response::Ready)?;
    stdout.write_all(&ready_msg)?;
    stdout.flush()?;

    let mut buffer = Vec::new();
    let mut read_buf = [0u8; 4096];

    loop {
        // Lese Daten von stdin
        let n = stdin.read(&mut read_buf)?;
        if n == 0 {
            // EOF - Beende
            break;
        }
        buffer.extend_from_slice(&read_buf[..n]);

        // Versuche Nachrichten zu dekodieren
        while let Some((request, consumed)) = decode_message::<Request>(&buffer)? {
            buffer.drain(..consumed);

            match request {
                Request::Execute {
                    program,
                    args,
                    working_dir,
                    env,
                } => {
                    execute_command(&mut stdout, &program, &args, working_dir.as_deref(), &env)?;
                }
                Request::Shutdown => {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn execute_command(
    stdout: &mut impl Write,
    program: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &[(String, String)],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::new(program);
    cmd.args(args);

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    for (key, value) in env {
        cmd.env(key, value);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let error_msg = encode_message(&Response::Error {
                message: format!("Failed to spawn process: {}", e),
            })?;
            stdout.write_all(&error_msg)?;
            stdout.flush()?;
            return Ok(());
        }
    };

    // Lese stdout und stderr in separaten Threads
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    let stdout_handle = if let Some(mut child_out) = child_stdout {
        Some(std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut chunks = Vec::new();
            loop {
                match child_out.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        chunks.push((OutputStream::Stdout, buf[..n].to_vec()));
                    }
                    Err(_) => break,
                }
            }
            chunks
        }))
    } else {
        None
    };

    let stderr_handle = if let Some(mut child_err) = child_stderr {
        Some(std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut chunks = Vec::new();
            loop {
                match child_err.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        chunks.push((OutputStream::Stderr, buf[..n].to_vec()));
                    }
                    Err(_) => break,
                }
            }
            chunks
        }))
    } else {
        None
    };

    // Sammle stdout-Chunks
    if let Some(handle) = stdout_handle {
        if let Ok(chunks) = handle.join() {
            for (stream, data) in chunks {
                let msg = encode_message(&Response::Output { stream, data })?;
                stdout.write_all(&msg)?;
            }
        }
    }

    // Sammle stderr-Chunks
    if let Some(handle) = stderr_handle {
        if let Ok(chunks) = handle.join() {
            for (stream, data) in chunks {
                let msg = encode_message(&Response::Output { stream, data })?;
                stdout.write_all(&msg)?;
            }
        }
    }

    // Warte auf Prozess-Ende
    let status = child.wait()?;
    let exit_msg = encode_message(&Response::Exit {
        code: status.code(),
    })?;
    stdout.write_all(&exit_msg)?;
    stdout.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_request() {
        let req = Request::Execute {
            program: "ls".to_string(),
            args: vec!["-la".to_string()],
            working_dir: Some("/tmp".to_string()),
            env: vec![("FOO".to_string(), "bar".to_string())],
        };

        let encoded = encode_message(&req).unwrap();
        let (decoded, consumed): (Request, usize) =
            decode_message(&encoded).unwrap().unwrap();

        assert_eq!(consumed, encoded.len());
        match decoded {
            Request::Execute { program, args, .. } => {
                assert_eq!(program, "ls");
                assert_eq!(args, vec!["-la"]);
            }
            _ => panic!("Unexpected variant"),
        }
    }

    #[test]
    fn test_roundtrip_response() {
        let resp = Response::Output {
            stream: OutputStream::Stdout,
            data: b"hello world".to_vec(),
        };

        let encoded = encode_message(&resp).unwrap();
        let (decoded, _): (Response, usize) = decode_message(&encoded).unwrap().unwrap();

        match decoded {
            Response::Output { stream, data } => {
                assert_eq!(stream, OutputStream::Stdout);
                assert_eq!(data, b"hello world");
            }
            _ => panic!("Unexpected variant"),
        }
    }
}
