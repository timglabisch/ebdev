//! JSON Protocol Types für Guest-Host Kommunikation

use serde::{Deserialize, Serialize};

/// Request vom Guest an den Host
#[derive(Debug, Serialize)]
#[serde(tag = "op")]
pub enum GuestRequest<'a> {
    /// Führe einen Befehl aus
    #[serde(rename = "exec")]
    Exec {
        cmd: Vec<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        env: Option<Vec<(&'a str, &'a str)>>,
    },
    /// Führe ein Shell-Script aus
    #[serde(rename = "shell")]
    Shell {
        script: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        env: Option<Vec<(&'a str, &'a str)>>,
    },
    /// Log-Nachricht an den Task Runner
    #[serde(rename = "log")]
    Log { message: &'a str },
}

/// Response vom Host an den Guest
#[derive(Debug, Clone, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub success: bool,
    pub timed_out: bool,
}
