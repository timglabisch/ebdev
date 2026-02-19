//! Remote file operations — standalone async functions that open a bridge
//! connection, send a single file request, read the response, and shut down.

use crate::{
    decode_message, encode_message, ExecutorError, Request, Response, PROTOCOL_VERSION,
};
use crate::remote::RemoteExecutor;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// File stat result returned from remote_stat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub exists: bool,
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
}

/// Internal helper: start a bridge, run a file operation, read the response, shut down.
async fn with_bridge<F, R>(
    container: &str,
    embedded_binary: &[u8],
    session_id: u32,
    build_request: F,
) -> Result<R, ExecutorError>
where
    F: FnOnce() -> Request,
    R: TryFromResponse,
{
    let container_binary_path = "/tmp/ebdev";

    // Try to start existing binary first
    let start_result = RemoteExecutor::try_start_bridge(container, container_binary_path).await;

    let (mut child, mut stdin, mut stdout, mut buffer, version) = match start_result {
        Ok(tup @ (_, _, _, _, v)) if v == PROTOCOL_VERSION => tup,
        Ok((mut child, _, _, _, _version)) => {
            let _ = child.kill().await;
            // Copy binary and retry
            copy_binary(container, embedded_binary, container_binary_path).await?;
            RemoteExecutor::try_start_bridge(container, container_binary_path).await?
        }
        Err(_) => {
            copy_binary(container, embedded_binary, container_binary_path).await?;
            RemoteExecutor::try_start_bridge(container, container_binary_path).await?
        }
    };

    if version != PROTOCOL_VERSION {
        let _ = child.kill().await;
        return Err(ExecutorError::Protocol(format!(
            "Bridge protocol version mismatch (got {}, need {})",
            version, PROTOCOL_VERSION
        )));
    }

    // Send the file request
    let request = build_request();
    let msg = encode_message(&request).map_err(|e| ExecutorError::Protocol(e.to_string()))?;
    stdin.write_all(&msg).await?;
    stdin.flush().await?;

    // Read response
    let mut read_buf = [0u8; 65536];
    loop {
        // Check buffer first
        if let Some((response, consumed)) =
            decode_message::<Response>(&buffer).map_err(|e| ExecutorError::Protocol(e.to_string()))?
        {
            buffer.drain(..consumed);

            // Send shutdown
            let shutdown = encode_message(&Request::Shutdown)
                .map_err(|e| ExecutorError::Protocol(e.to_string()))?;
            let _ = stdin.write_all(&shutdown).await;
            let _ = stdin.flush().await;
            drop(stdin);

            return R::try_from_response(response, session_id);
        }

        let n = stdout.read(&mut read_buf).await?;
        if n == 0 {
            return Err(ExecutorError::Protocol("Bridge closed unexpectedly".into()));
        }
        buffer.extend_from_slice(&read_buf[..n]);
    }
}

/// Copy the embedded binary to the container
async fn copy_binary(
    container: &str,
    embedded_binary: &[u8],
    container_binary_path: &str,
) -> Result<(), ExecutorError> {
    if embedded_binary.is_empty() {
        return Err(ExecutorError::Spawn(
            "No embedded Linux bridge binary. Build with 'make build' to include it.".into(),
        ));
    }

    use std::process::Stdio;
    use tokio::process::Command;

    let mut child = Command::new("docker")
        .args([
            "exec", "-i", container, "sh", "-c",
            &format!("cat > {} && chmod +x {}", container_binary_path, container_binary_path),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(ExecutorError::Io)?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(embedded_binary).await?;
        drop(stdin);
    }

    let status = child.wait().await.map_err(ExecutorError::Io)?;
    if !status.success() {
        return Err(ExecutorError::Spawn(
            "Failed to copy embedded binary to container".into(),
        ));
    }

    Ok(())
}

/// Trait to extract typed result from a Response
trait TryFromResponse: Sized {
    fn try_from_response(response: Response, expected_session_id: u32) -> Result<Self, ExecutorError>;
}

/// Unit result (for write, append, mkdir, remove)
impl TryFromResponse for () {
    fn try_from_response(response: Response, expected_session_id: u32) -> Result<Self, ExecutorError> {
        match response {
            Response::FileWritten { session_id } if session_id == expected_session_id => Ok(()),
            Response::DirCreated { session_id } if session_id == expected_session_id => Ok(()),
            Response::Removed { session_id } if session_id == expected_session_id => Ok(()),
            Response::Error { message, .. } => Err(ExecutorError::Spawn(message)),
            other => Err(ExecutorError::Protocol(format!(
                "Unexpected response: {:?}",
                other
            ))),
        }
    }
}

/// Vec<u8> result (for read)
impl TryFromResponse for Vec<u8> {
    fn try_from_response(response: Response, expected_session_id: u32) -> Result<Self, ExecutorError> {
        match response {
            Response::FileContent { session_id, data } if session_id == expected_session_id => Ok(data),
            Response::Error { message, .. } => Err(ExecutorError::Spawn(message)),
            other => Err(ExecutorError::Protocol(format!(
                "Unexpected response: {:?}",
                other
            ))),
        }
    }
}

/// FileStat result
impl TryFromResponse for FileStat {
    fn try_from_response(response: Response, expected_session_id: u32) -> Result<Self, ExecutorError> {
        match response {
            Response::FileStat {
                session_id,
                exists,
                is_file,
                is_dir,
                size,
            } if session_id == expected_session_id => Ok(FileStat {
                exists,
                is_file,
                is_dir,
                size,
            }),
            Response::Error { message, .. } => Err(ExecutorError::Spawn(message)),
            other => Err(ExecutorError::Protocol(format!(
                "Unexpected response: {:?}",
                other
            ))),
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

pub async fn remote_write_file(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
    data: &[u8],
) -> Result<(), ExecutorError> {
    let path = path.to_string();
    let data = data.to_vec();
    with_bridge(container, embedded_binary, 1, move || Request::WriteFile {
        session_id: 1,
        path,
        data,
    })
    .await
}

pub async fn remote_read_file(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
) -> Result<Vec<u8>, ExecutorError> {
    let path = path.to_string();
    with_bridge(container, embedded_binary, 1, move || Request::ReadFile {
        session_id: 1,
        path,
    })
    .await
}

pub async fn remote_append_file(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
    data: &[u8],
) -> Result<(), ExecutorError> {
    let path = path.to_string();
    let data = data.to_vec();
    with_bridge(container, embedded_binary, 1, move || Request::AppendFile {
        session_id: 1,
        path,
        data,
    })
    .await
}

pub async fn remote_mkdir(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
    recursive: bool,
) -> Result<(), ExecutorError> {
    let path = path.to_string();
    with_bridge(container, embedded_binary, 1, move || Request::MkDir {
        session_id: 1,
        path,
        recursive,
    })
    .await
}

pub async fn remote_remove(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
    recursive: bool,
) -> Result<(), ExecutorError> {
    let path = path.to_string();
    with_bridge(container, embedded_binary, 1, move || Request::Remove {
        session_id: 1,
        path,
        recursive,
    })
    .await
}

pub async fn remote_stat(
    container: &str,
    embedded_binary: &[u8],
    path: &str,
) -> Result<FileStat, ExecutorError> {
    let path = path.to_string();
    with_bridge(container, embedded_binary, 1, move || Request::Stat {
        session_id: 1,
        path,
    })
    .await
}
