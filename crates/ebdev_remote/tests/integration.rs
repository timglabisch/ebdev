//! Integration tests für das Bridge-Protokoll
//!
//! Diese Tests starten die Bridge als separaten Prozess und kommunizieren
//! über stdin/stdout mit dem bincode-Protokoll.

use ebdev_remote::{
    decode_message, encode_message, PtyConfig, Request, Response, OutputStream, MAGIC, PROTOCOL_VERSION,
};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

/// Test-Helper: Verbindung zur Bridge
struct BridgeTestClient {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    buffer: Vec<u8>,
}

impl BridgeTestClient {
    /// Startet die Bridge als Child-Prozess
    async fn spawn() -> Self {
        // Bridge-Binary bauen falls nötig und starten
        let mut child = Command::new(env!("CARGO_BIN_EXE_ebdev-bridge"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to spawn bridge process");

        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = child.stdout.take().expect("stdout");

        // Warte auf MAGIC bytes
        let mut magic_buf = [0u8; 4];
        stdout.read_exact(&mut magic_buf).await.expect("read magic");
        assert_eq!(&magic_buf, MAGIC, "Invalid magic bytes");

        let mut client = Self {
            child,
            stdin,
            stdout,
            buffer: Vec::new(),
        };

        // Warte auf Ready mit Versionscheck
        let response = client.read_response().await;
        match response {
            Response::Ready { protocol_version } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION, "Protocol version mismatch");
            }
            _ => panic!("Expected Ready, got {:?}", response),
        }

        client
    }

    /// Sendet einen Request
    async fn send(&mut self, request: &Request) {
        let msg = encode_message(request).expect("encode");
        self.stdin.write_all(&msg).await.expect("write");
        self.stdin.flush().await.expect("flush");
    }

    /// Liest eine Response (blockiert bis eine vollständige Nachricht da ist)
    async fn read_response(&mut self) -> Response {
        let mut read_buf = [0u8; 4096];

        loop {
            // Versuche erst aus dem Buffer zu dekodieren
            if let Some((response, consumed)) = decode_message::<Response>(&self.buffer).expect("decode") {
                self.buffer.drain(..consumed);
                return response;
            }

            // Mehr Daten lesen
            let n = self.stdout.read(&mut read_buf).await.expect("read");
            if n == 0 {
                panic!("Bridge closed unexpectedly");
            }
            self.buffer.extend_from_slice(&read_buf[..n]);
        }
    }

    /// Liest Responses bis Exit oder Timeout
    async fn collect_until_exit(&mut self, session_id: u32) -> (Vec<(OutputStream, Vec<u8>)>, Option<i32>) {
        let mut outputs = Vec::new();
        let timeout = tokio::time::Duration::from_secs(5);

        loop {
            let response = tokio::time::timeout(timeout, self.read_response())
                .await
                .expect("timeout waiting for response");

            match response {
                Response::Output { session_id: sid, stream, data } if sid == session_id => {
                    outputs.push((stream, data));
                }
                Response::Exit { session_id: sid, code } if sid == session_id => {
                    return (outputs, code);
                }
                Response::Error { session_id: Some(sid), message } if sid == session_id => {
                    panic!("Unexpected error: {}", message);
                }
                Response::Pong => {} // Ignorieren
                other => {
                    // Andere Session oder unerwartete Response
                    panic!("Unexpected response: {:?}", other);
                }
            }
        }
    }

    /// Beendet die Bridge
    async fn shutdown(mut self) {
        self.send(&Request::Shutdown).await;
        let _ = self.child.wait().await;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_simple_echo() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte echo Befehl
    client.send(&Request::Execute {
        session_id: 1,
        program: "echo".to_string(),
        args: vec!["Hello, World!".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    // Warte auf Started
    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Sammle Output bis Exit
    let (outputs, exit_code) = client.collect_until_exit(1).await;

    // Prüfe Ergebnis
    assert_eq!(exit_code, Some(0));
    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "Hello, World!");

    client.shutdown().await;
}

#[tokio::test]
async fn test_exit_code() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte Befehl der mit Code 42 beendet
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "exit 42".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (_, exit_code) = client.collect_until_exit(1).await;
    assert_eq!(exit_code, Some(42));

    client.shutdown().await;
}

#[tokio::test]
async fn test_stderr() {
    let mut client = BridgeTestClient::spawn().await;

    // Befehl der auf stderr schreibt
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo error >&2".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));
    let stderr: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stderr)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert_eq!(String::from_utf8_lossy(&stderr).trim(), "error");

    client.shutdown().await;
}

#[tokio::test]
async fn test_stdout_and_stderr() {
    let mut client = BridgeTestClient::spawn().await;

    // Befehl der auf stdout und stderr schreibt
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo out; echo err >&2".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));

    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    let stderr: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stderr)
        .flat_map(|(_, d)| d.clone())
        .collect();

    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "out");
    assert_eq!(String::from_utf8_lossy(&stderr).trim(), "err");

    client.shutdown().await;
}

#[tokio::test]
async fn test_working_directory() {
    let mut client = BridgeTestClient::spawn().await;

    let tmp_dir = std::env::temp_dir();
    let tmp_dir_str = tmp_dir.to_str().unwrap().to_string();

    client.send(&Request::Execute {
        session_id: 1,
        program: "pwd".to_string(),
        args: vec!["-P".to_string()],
        working_dir: Some(tmp_dir_str.clone()),
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));
    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    let pwd = String::from_utf8_lossy(&stdout).trim().to_string();
    // std::env::temp_dir() gibt den physischen Pfad, pwd -P löst Symlinks auf
    let expected = tmp_dir.canonicalize().unwrap();
    assert_eq!(pwd, expected.to_str().unwrap());

    client.shutdown().await;
}

#[tokio::test]
async fn test_environment_variable() {
    let mut client = BridgeTestClient::spawn().await;

    // Setze Umgebungsvariable
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo $TEST_VAR".to_string()],
        working_dir: None,
        env: vec![("TEST_VAR".to_string(), "hello_env".to_string())],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));
    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "hello_env");

    client.shutdown().await;
}

#[tokio::test]
async fn test_ping_pong() {
    let mut client = BridgeTestClient::spawn().await;

    // Sende Ping
    client.send(&Request::Ping).await;

    // Erwarte Pong
    let response = client.read_response().await;
    assert!(matches!(response, Response::Pong));

    // Nochmal
    client.send(&Request::Ping).await;
    let response = client.read_response().await;
    assert!(matches!(response, Response::Pong));

    client.shutdown().await;
}

#[tokio::test]
async fn test_multi_session_sequential() {
    let mut client = BridgeTestClient::spawn().await;

    // Erste Session
    client.send(&Request::Execute {
        session_id: 1,
        program: "echo".to_string(),
        args: vec!["first".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, _) = client.collect_until_exit(1).await;
    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "first");

    // Zweite Session (nach Beendigung der ersten)
    client.send(&Request::Execute {
        session_id: 2,
        program: "echo".to_string(),
        args: vec!["second".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 2 }));

    let (outputs, _) = client.collect_until_exit(2).await;
    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "second");

    client.shutdown().await;
}

#[tokio::test]
async fn test_stdin_simple() {
    let mut client = BridgeTestClient::spawn().await;

    // cat liest von stdin und gibt auf stdout aus
    client.send(&Request::Execute {
        session_id: 1,
        program: "cat".to_string(),
        args: vec![],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Sende Daten an stdin
    client.send(&Request::Stdin {
        session_id: 1,
        data: b"Hello from stdin\n".to_vec(),
    }).await;

    // Kleine Pause damit cat die Daten verarbeiten kann
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Kill cat (sonst wartet es ewig auf mehr Input)
    client.send(&Request::Kill { session_id: 1 }).await;

    // Warte auf Output und Exit
    let timeout = tokio::time::Duration::from_secs(2);
    let mut outputs = Vec::new();

    loop {
        match tokio::time::timeout(timeout, client.read_response()).await {
            Ok(Response::Output { session_id: 1, stream, data }) => {
                outputs.push((stream, data));
            }
            Ok(Response::Exit { session_id: 1, .. }) => break,
            Ok(_) => {}
            Err(_) => break, // Timeout
        }
    }

    let stdout: Vec<u8> = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert!(String::from_utf8_lossy(&stdout).contains("Hello from stdin"));

    client.shutdown().await;
}

#[tokio::test]
async fn test_duplicate_session_id_error() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte lang laufenden Prozess
    client.send(&Request::Execute {
        session_id: 1,
        program: "sleep".to_string(),
        args: vec!["10".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Versuche gleiche Session-ID nochmal
    client.send(&Request::Execute {
        session_id: 1,
        program: "echo".to_string(),
        args: vec!["should fail".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    // Erwarte Fehler
    let response = client.read_response().await;
    match response {
        Response::Error { session_id: Some(1), message } => {
            assert!(message.contains("already exists"), "Expected 'already exists' error, got: {}", message);
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Cleanup
    client.send(&Request::Kill { session_id: 1 }).await;
    client.shutdown().await;
}

#[tokio::test]
async fn test_invalid_program() {
    let mut client = BridgeTestClient::spawn().await;

    // Nicht existierendes Programm
    client.send(&Request::Execute {
        session_id: 1,
        program: "/nonexistent/program".to_string(),
        args: vec![],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    // Entweder Error oder Started mit Exit 127
    let response = client.read_response().await;
    match response {
        Response::Error { .. } => {
            // OK - Fehler beim Starten
        }
        Response::Started { session_id: 1 } => {
            // PTY-Modus gibt Started zurück, dann Exit 127
            let (_, code) = client.collect_until_exit(1).await;
            // Non-PTY sollte direkt einen IO-Fehler geben
            assert!(code.is_none() || code == Some(127) || code == Some(1));
        }
        other => panic!("Unexpected response: {:?}", other),
    }

    client.shutdown().await;
}

#[tokio::test]
async fn test_pty_basic() {
    let mut client = BridgeTestClient::spawn().await;

    // PTY-Modus mit echo
    client.send(&Request::Execute {
        session_id: 1,
        program: "echo".to_string(),
        args: vec!["PTY test".to_string()],
        working_dir: None,
        env: vec![],
        pty: Some(PtyConfig { cols: 80, rows: 24 }),
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));
    // Bei PTY kommt alles über stdout
    let output: Vec<u8> = outputs.iter()
        .flat_map(|(_, d)| d.clone())
        .collect();
    assert!(String::from_utf8_lossy(&output).contains("PTY test"));

    client.shutdown().await;
}

#[tokio::test]
async fn test_pty_term_variable() {
    let mut client = BridgeTestClient::spawn().await;

    // Prüfe dass TERM gesetzt ist im PTY-Modus
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "echo $TERM".to_string()],
        working_dir: None,
        env: vec![],
        pty: Some(PtyConfig { cols: 80, rows: 24 }),
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, _) = client.collect_until_exit(1).await;

    let output: Vec<u8> = outputs.iter()
        .flat_map(|(_, d)| d.clone())
        .collect();
    let term = String::from_utf8_lossy(&output);
    assert!(term.contains("xterm"), "Expected TERM to contain 'xterm', got: {}", term);

    client.shutdown().await;
}

#[tokio::test]
async fn test_large_output() {
    let mut client = BridgeTestClient::spawn().await;

    // Generiere 100KB Output
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "yes | head -n 10000".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    assert_eq!(exit_code, Some(0));
    let total_bytes: usize = outputs.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .map(|(_, d)| d.len())
        .sum();

    // 10000 Zeilen "y\n" = 20000 bytes
    assert!(total_bytes >= 19000, "Expected at least 19000 bytes, got {}", total_bytes);

    client.shutdown().await;
}

#[tokio::test]
async fn test_multi_session_parallel() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte zwei parallele Sessions
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 0.1; echo session1".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    client.send(&Request::Execute {
        session_id: 2,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "sleep 0.05; echo session2".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    // Sammle alle Responses
    let mut started = std::collections::HashSet::new();
    let mut outputs1 = Vec::new();
    let mut outputs2 = Vec::new();
    let mut exit1 = None;
    let mut exit2 = None;

    let timeout = tokio::time::Duration::from_secs(5);

    while exit1.is_none() || exit2.is_none() {
        let response = tokio::time::timeout(timeout, client.read_response())
            .await
            .expect("timeout");

        match response {
            Response::Started { session_id } => {
                started.insert(session_id);
            }
            Response::Output { session_id: 1, stream, data } => {
                outputs1.push((stream, data));
            }
            Response::Output { session_id: 2, stream, data } => {
                outputs2.push((stream, data));
            }
            Response::Exit { session_id: 1, code } => {
                exit1 = Some(code);
            }
            Response::Exit { session_id: 2, code } => {
                exit2 = Some(code);
            }
            _ => {}
        }
    }

    assert!(started.contains(&1));
    assert!(started.contains(&2));
    assert_eq!(exit1, Some(Some(0)));
    assert_eq!(exit2, Some(Some(0)));

    let stdout1: Vec<u8> = outputs1.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();
    let stdout2: Vec<u8> = outputs2.iter()
        .filter(|(s, _)| *s == OutputStream::Stdout)
        .flat_map(|(_, d)| d.clone())
        .collect();

    assert!(String::from_utf8_lossy(&stdout1).contains("session1"));
    assert!(String::from_utf8_lossy(&stdout2).contains("session2"));

    client.shutdown().await;
}

#[tokio::test]
async fn test_kill_running_process() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte lang laufenden Prozess
    client.send(&Request::Execute {
        session_id: 1,
        program: "sleep".to_string(),
        args: vec!["60".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Kurz warten, dann killen
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    client.send(&Request::Kill { session_id: 1 }).await;

    // Warte auf Exit
    let timeout = tokio::time::Duration::from_secs(2);
    let response = tokio::time::timeout(timeout, client.read_response())
        .await
        .expect("timeout waiting for exit after kill");

    match response {
        Response::Exit { session_id: 1, .. } => {
            // OK - Prozess wurde beendet
        }
        other => panic!("Expected Exit, got {:?}", other),
    }

    client.shutdown().await;
}

#[tokio::test]
async fn test_pty_resize() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte ein Programm das die Terminal-Größe ausgibt
    client.send(&Request::Execute {
        session_id: 1,
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "stty size || echo 'no stty'".to_string()],
        working_dir: None,
        env: vec![],
        pty: Some(PtyConfig { cols: 120, rows: 40 }),
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    let (outputs, exit_code) = client.collect_until_exit(1).await;

    // stty size gibt "rows cols" aus
    let output: Vec<u8> = outputs.iter()
        .flat_map(|(_, d)| d.clone())
        .collect();
    let output_str = String::from_utf8_lossy(&output);

    // Entweder "40 120" oder Fehler wenn stty nicht verfügbar
    if !output_str.contains("no stty") {
        assert!(output_str.contains("40") && output_str.contains("120"),
            "Expected size 40x120, got: {}", output_str);
    }

    assert_eq!(exit_code, Some(0));
    client.shutdown().await;
}

#[tokio::test]
async fn test_pty_stdin_echo() {
    let mut client = BridgeTestClient::spawn().await;

    // cat im PTY-Modus
    client.send(&Request::Execute {
        session_id: 1,
        program: "cat".to_string(),
        args: vec![],
        working_dir: None,
        env: vec![],
        pty: Some(PtyConfig { cols: 80, rows: 24 }),
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Sende Daten
    client.send(&Request::Stdin {
        session_id: 1,
        data: b"hello pty\n".to_vec(),
    }).await;

    // Warte auf Echo (PTY echot Input)
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Kill
    client.send(&Request::Kill { session_id: 1 }).await;

    // Sammle Output
    let timeout = tokio::time::Duration::from_secs(2);
    let mut outputs = Vec::new();

    loop {
        match tokio::time::timeout(timeout, client.read_response()).await {
            Ok(Response::Output { session_id: 1, data, .. }) => {
                outputs.extend(data);
            }
            Ok(Response::Exit { session_id: 1, .. }) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let output = String::from_utf8_lossy(&outputs);
    assert!(output.contains("hello pty"), "Expected 'hello pty' in output, got: {}", output);

    client.shutdown().await;
}

#[tokio::test]
async fn test_rapid_ping_pong() {
    let mut client = BridgeTestClient::spawn().await;

    // Sende 100 Pings schnell hintereinander
    for _ in 0..100 {
        client.send(&Request::Ping).await;
    }

    // Erwarte 100 Pongs
    for i in 0..100 {
        let response = tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            client.read_response()
        ).await.expect(&format!("timeout waiting for pong {}", i));

        assert!(matches!(response, Response::Pong), "Expected Pong, got {:?}", response);
    }

    client.shutdown().await;
}

#[tokio::test]
async fn test_shutdown_kills_running_processes() {
    let mut client = BridgeTestClient::spawn().await;

    // Starte einen lang laufenden Prozess
    client.send(&Request::Execute {
        session_id: 1,
        program: "sleep".to_string(),
        args: vec!["60".to_string()],
        working_dir: None,
        env: vec![],
        pty: None,
    }).await;

    let response = client.read_response().await;
    assert!(matches!(response, Response::Started { session_id: 1 }));

    // Shutdown sollte den Prozess beenden und die Bridge schließen
    client.send(&Request::Shutdown).await;

    // Bridge sollte sich beenden
    let status = tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        client.child.wait()
    ).await.expect("timeout waiting for bridge to exit");

    assert!(status.is_ok(), "Bridge should exit cleanly");
}
