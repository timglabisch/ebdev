//! ebdev-wasm - Guest Library für WASM Plugins
//!
//! Stellt eine einfache API bereit um Befehle auf dem Host auszuführen
//! und Nachrichten an den Task Runner zu senden.
//!
//! # Example
//! ```rust,no_run
//! use ebdev_wasm::prelude::*;
//!
//! fn main() {
//!     log("Starting task...");
//!     exec(&["echo", "Hello from WASM!"]);
//!     let result = try_exec(&["ls", "-la"]);
//!     if result.success {
//!         log("Command succeeded");
//!     }
//! }
//! ```

mod host;
pub mod protocol;

pub use protocol::ExecResult;

pub mod prelude {
    pub use crate::{args, exec, log, shell, try_exec, try_shell, ExecResult};
}

/// Führe einen Befehl aus, panic bei Fehler
pub fn exec(cmd: &[&str]) -> ExecResult {
    let result = try_exec(cmd);
    if !result.success {
        panic!(
            "Command '{}' failed with exit code {}",
            cmd.join(" "),
            result.exit_code
        );
    }
    result
}

/// Führe einen Befehl aus, gibt Result zurück (wirft nicht)
pub fn try_exec(cmd: &[&str]) -> ExecResult {
    let request = protocol::GuestRequest::Exec {
        cmd: cmd.to_vec(),
        cwd: None,
        env: None,
    };
    call_and_parse(&request)
}

/// Shell-Script ausführen, panic bei Fehler
pub fn shell(script: &str) -> ExecResult {
    let result = try_shell(script);
    if !result.success {
        panic!("Shell command failed with exit code {}", result.exit_code);
    }
    result
}

/// Shell-Script ausführen, gibt Result zurück (wirft nicht)
pub fn try_shell(script: &str) -> ExecResult {
    let request = protocol::GuestRequest::Shell {
        script,
        cwd: None,
        env: None,
    };
    call_and_parse(&request)
}

/// Log-Message an den Task Runner senden
pub fn log(message: &str) {
    let request = protocol::GuestRequest::Log { message };
    let json = serde_json::to_vec(&request).expect("Failed to serialize log request");
    host::call_host(&json);
}

/// WASI args (wrapper um std::env::args)
pub fn args() -> Vec<String> {
    std::env::args().skip(1).collect() // Skip program name
}

fn call_and_parse(request: &protocol::GuestRequest) -> ExecResult {
    let json = serde_json::to_vec(request).expect("Failed to serialize request");
    match host::call_host(&json) {
        Some(response) => {
            serde_json::from_slice(&response).unwrap_or(ExecResult {
                exit_code: 1,
                success: false,
                timed_out: false,
            })
        }
        None => ExecResult {
            exit_code: 1,
            success: false,
            timed_out: false,
        },
    }
}
