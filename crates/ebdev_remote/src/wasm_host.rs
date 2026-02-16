//! WASM/WASI Runtime - Führt WASM Module mit wasmtime aus
//!
//! Stellt Host-Funktionen bereit die vom Guest über `ebdev_call` aufgerufen werden.
//! Kommunikation läuft über JSON im Shared Memory.

use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use wasmtime::*;
use wasmtime_wasi::preview1::WasiP1Ctx;
use wasmtime_wasi::WasiCtxBuilder;

/// Request vom Guest an den Host
#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
pub enum GuestRequest {
    #[serde(rename = "exec")]
    Exec {
        cmd: Vec<String>,
        cwd: Option<String>,
        env: Option<Vec<(String, String)>>,
    },
    #[serde(rename = "shell")]
    Shell {
        script: String,
        cwd: Option<String>,
        env: Option<Vec<(String, String)>>,
    },
    #[serde(rename = "log")]
    Log { message: String },
}

/// Response vom Host an den Guest
#[derive(Debug, Serialize)]
pub struct ExecResponse {
    pub exit_code: i32,
    pub success: bool,
    pub timed_out: bool,
}

/// State für die WASM Host-Funktionen
pub struct WasmHostState {
    pub wasi_ctx: WasiP1Ctx,
    pub working_dir: Option<String>,
    pub output_tx: mpsc::Sender<Vec<u8>>,
}

impl wasmtime_wasi::WasiView for WasmHostState {
    fn ctx(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        self.wasi_ctx.ctx()
    }

    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        self.wasi_ctx.table()
    }
}

/// Führt ein WASM Modul aus und gibt den Exit-Code zurück
pub fn run_wasm_module(
    wasm_bytes: &[u8],
    args: Vec<String>,
    env: Vec<(String, String)>,
    working_dir: Option<String>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> i32 {
    match run_wasm_module_inner(wasm_bytes, args, env, working_dir, output_tx.clone()) {
        Ok(code) => code,
        Err(e) => {
            let msg = format!("WASM execution error: {:#}\n", e);
            let _ = output_tx.send(msg.into_bytes());
            1
        }
    }
}

fn run_wasm_module_inner(
    wasm_bytes: &[u8],
    args: Vec<String>,
    env: Vec<(String, String)>,
    working_dir: Option<String>,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> anyhow::Result<i32> {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm_bytes)?;

    // WASI Context
    let mut wasi_builder = WasiCtxBuilder::new();

    // Args
    let mut wasi_args = vec!["wasm-module".to_string()];
    wasi_args.extend(args);
    wasi_builder.args(&wasi_args);

    // Env
    for (key, value) in &env {
        wasi_builder.env(key, value);
    }

    // stdout/stderr capture
    let stdout_tx = output_tx.clone();
    let stderr_tx = output_tx.clone();
    wasi_builder.stdout(OutputPipe::new(stdout_tx));
    wasi_builder.stderr(OutputPipe::new(stderr_tx));

    // Filesystem preopens
    if let Some(ref dir) = working_dir {
        let path = std::path::Path::new(dir);
        if path.exists() {
            wasi_builder.preopened_dir(
                path,
                dir,
                wasmtime_wasi::DirPerms::all(),
                wasmtime_wasi::FilePerms::all(),
            )?;
        }
    }

    // Root filesystem (read-only)
    let root = std::path::Path::new("/");
    if root.exists() {
        wasi_builder.preopened_dir(
            root,
            "/",
            wasmtime_wasi::DirPerms::READ,
            wasmtime_wasi::FilePerms::READ,
        )?;
    }

    let wasi_ctx = wasi_builder.build_p1();
    let host_state = WasmHostState {
        wasi_ctx,
        working_dir: working_dir.clone(),
        output_tx,
    };

    // Linker
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |state: &mut WasmHostState| {
        &mut state.wasi_ctx
    })?;

    // ebdev_call Host-Function
    linker.func_wrap(
        "ebdev",
        "call",
        |mut caller: Caller<'_, WasmHostState>, request_ptr: i32, request_len: i32| -> i64 {
            handle_guest_call(&mut caller, request_ptr, request_len)
        },
    )?;

    let mut store = Store::new(&engine, host_state);
    let instance = linker.instantiate(&mut store, &module)?;

    // _start aufrufen (WASI Entry Point)
    let start = instance.get_typed_func::<(), ()>(&mut store, "_start");
    match start {
        Ok(func) => match func.call(&mut store, ()) {
            Ok(()) => Ok(0),
            Err(e) => extract_exit_code(e),
        },
        Err(_) => {
            // Fallback: main()
            if let Ok(main) = instance.get_typed_func::<(), ()>(&mut store, "main") {
                match main.call(&mut store, ()) {
                    Ok(()) => Ok(0),
                    Err(e) => extract_exit_code(e),
                }
            } else {
                anyhow::bail!("No _start or main function found in WASM module")
            }
        }
    }
}

/// Extrahiert den Exit-Code aus einem wasmtime Error (z.B. WASI proc_exit)
fn extract_exit_code(e: anyhow::Error) -> anyhow::Result<i32> {
    // WASI proc_exit: I32Exit enthält den Exit-Code
    if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
        return Ok(exit.0);
    }
    // Fallback: prüfe Error-String
    let err_str = format!("{:#}", e);
    if err_str.contains("proc_exit") || err_str.contains("exit status") {
        return Ok(1);
    }
    Err(e)
}

/// Verarbeitet einen Guest-Call und gibt die Response als packed i64 zurück
fn handle_guest_call(
    caller: &mut Caller<'_, WasmHostState>,
    request_ptr: i32,
    request_len: i32,
) -> i64 {
    let memory = match caller.get_export("memory") {
        Some(Extern::Memory(mem)) => mem,
        _ => return 0,
    };

    // Request JSON aus Guest Memory lesen
    let data = memory.data(&caller);
    let ptr = request_ptr as usize;
    let len = request_len as usize;
    if ptr + len > data.len() {
        return 0;
    }
    let request_bytes = data[ptr..ptr + len].to_vec();

    let request: GuestRequest = match serde_json::from_slice(&request_bytes) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Invalid guest request: {}\n", e);
            let _ = caller.data().output_tx.send(msg.into_bytes());
            return 0;
        }
    };

    let response_json = match request {
        GuestRequest::Exec { cmd, cwd, env } => {
            let working_dir = cwd.as_deref().or(caller.data().working_dir.as_deref());
            let result =
                execute_command(&cmd, working_dir, env.as_deref(), &caller.data().output_tx);
            serde_json::to_vec(&result).unwrap_or_default()
        }
        GuestRequest::Shell { script, cwd, env } => {
            let cmd = vec!["sh".to_string(), "-c".to_string(), script];
            let working_dir = cwd.as_deref().or(caller.data().working_dir.as_deref());
            let result =
                execute_command(&cmd, working_dir, env.as_deref(), &caller.data().output_tx);
            serde_json::to_vec(&result).unwrap_or_default()
        }
        GuestRequest::Log { message } => {
            let msg = format!("[wasm] {}\n", message);
            let _ = caller.data().output_tx.send(msg.into_bytes());
            return 0;
        }
    };

    if response_json.is_empty() {
        return 0;
    }

    // Alloziere Speicher im Guest via ebdev_alloc
    let alloc = match caller.get_export("ebdev_alloc") {
        Some(Extern::Func(f)) => f,
        _ => return 0,
    };

    let response_len = response_json.len() as i32;
    let mut results = [Val::I32(0)];
    if alloc
        .call(&mut *caller, &[Val::I32(response_len)], &mut results)
        .is_err()
    {
        return 0;
    }

    let response_ptr = match results[0] {
        Val::I32(ptr) => ptr,
        _ => return 0,
    };

    // Response in Guest Memory schreiben
    let memory = match caller.get_export("memory") {
        Some(Extern::Memory(mem)) => mem,
        _ => return 0,
    };

    let dest = response_ptr as usize;
    let mem_data = memory.data_mut(&mut *caller);
    if dest + response_json.len() <= mem_data.len() {
        mem_data[dest..dest + response_json.len()].copy_from_slice(&response_json);
    }

    ((response_ptr as i64) << 32) | (response_len as i64)
}

/// Führt einen Befehl synchron aus und streamt Output
fn execute_command(
    cmd: &[String],
    working_dir: Option<&str>,
    env: Option<&[(String, String)]>,
    output_tx: &mpsc::Sender<Vec<u8>>,
) -> ExecResponse {
    if cmd.is_empty() {
        return ExecResponse {
            exit_code: 1,
            success: false,
            timed_out: false,
        };
    }

    let mut command = std::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]);

    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    if let Some(env_vars) = env {
        for (key, value) in env_vars {
            command.env(key, value);
        }
    }

    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let msg = format!("Failed to spawn '{}': {}\n", cmd[0], e);
            let _ = output_tx.send(msg.into_bytes());
            return ExecResponse {
                exit_code: 127,
                success: false,
                timed_out: false,
            };
        }
    };

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => {
            let msg = format!("Failed to wait for '{}': {}\n", cmd[0], e);
            let _ = output_tx.send(msg.into_bytes());
            return ExecResponse {
                exit_code: 1,
                success: false,
                timed_out: false,
            };
        }
    };

    if !output.stdout.is_empty() {
        let _ = output_tx.send(output.stdout);
    }
    if !output.stderr.is_empty() {
        let _ = output_tx.send(output.stderr);
    }

    let exit_code = output.status.code().unwrap_or(-1);
    ExecResponse {
        exit_code,
        success: exit_code == 0,
        timed_out: false,
    }
}

/// Output pipe that sends bytes to an mpsc channel
struct OutputPipe {
    tx: mpsc::Sender<Vec<u8>>,
}

impl OutputPipe {
    fn new(tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self { tx }
    }
}

impl wasmtime_wasi::StdoutStream for OutputPipe {
    fn stream(&self) -> Box<dyn wasmtime_wasi::HostOutputStream> {
        Box::new(OutputWriter {
            tx: self.tx.clone(),
        })
    }

    fn isatty(&self) -> bool {
        false
    }
}

struct OutputWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

#[async_trait::async_trait]
impl wasmtime_wasi::Subscribe for OutputWriter {
    async fn ready(&mut self) {}
}

impl wasmtime_wasi::HostOutputStream for OutputWriter {
    fn write(&mut self, bytes: bytes::Bytes) -> Result<(), wasmtime_wasi::StreamError> {
        self.tx
            .send(bytes.to_vec())
            .map_err(|_| wasmtime_wasi::StreamError::Closed)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), wasmtime_wasi::StreamError> {
        Ok(())
    }

    fn check_write(&mut self) -> Result<usize, wasmtime_wasi::StreamError> {
        Ok(1024 * 1024)
    }
}
