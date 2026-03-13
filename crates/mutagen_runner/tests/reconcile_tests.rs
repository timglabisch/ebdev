//! Tests for the reconcile logic

use ebdev_mutagen_runner::{
    state::{DesiredSession, DesiredState},
    test_utils::{mock_session, MockMutagen},
    MutagenBackend, SessionStatus, SessionStatusInfo, SyncMode,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Helper to create a DesiredSession for tests
fn desired_session(name: &str, project_crc32: u32) -> DesiredSession {
    DesiredSession::new(
        format!("{}-{:08x}", name, project_crc32),
        name.to_string(),
        PathBuf::from("/test"),
        "docker://container/path".to_string(),
        SyncMode::TwoWaySafe,
        vec![],
    )
}

// ============================================================================
// Reconcile Logic Tests
// ============================================================================

#[tokio::test]
async fn test_reconcile_creates_missing_sessions() {
    let backend = MockMutagen::new();
    let project_crc32: u32 = 0x12345678;

    let desired = DesiredState::from_sessions(
        vec![
            desired_session("frontend", project_crc32),
            desired_session("backend", project_crc32),
        ],
        project_crc32,
    );

    // Simulate reconcile loop (one iteration)
    for session in &desired.sessions {
        let existing = backend.list_sessions().await.unwrap();
        let found = existing.iter().any(|s| s.name == session.name);
        if !found {
            backend.create_session_from_desired(session, false).await.unwrap();
        }
    }

    let created = backend.created_sessions();
    assert_eq!(created.len(), 2);
    assert!(created.iter().any(|(name, _)| name == "frontend-12345678"));
    assert!(created.iter().any(|(name, _)| name == "backend-12345678"));
}

#[tokio::test]
async fn test_reconcile_empty_sessions_terminates_all() {
    let backend = MockMutagen::new();
    let project_crc32: u32 = 0x12345678;
    let suffix = format!("-{:08x}", project_crc32);

    // Existing sessions for this project
    let mut s1 = mock_session("frontend-12345678");
    s1.identifier = "session-1".to_string();
    backend.add_session(s1);

    let mut s2 = mock_session("backend-12345678");
    s2.identifier = "session-2".to_string();
    backend.add_session(s2);

    // Session from different project (should NOT be terminated)
    let mut other = mock_session("other-aaaabbbb");
    other.identifier = "other-session".to_string();
    backend.add_session(other);

    // Empty desired state = cleanup: pause first, then terminate
    let sessions = backend.list_sessions().await.unwrap();
    let project_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s.name.ends_with(&suffix))
        .collect();

    for session in &project_sessions {
        backend.pause_session(&session.identifier).await.unwrap();
    }
    for session in &project_sessions {
        backend.terminate_session(&session.identifier).await.unwrap();
    }

    // Sessions must be paused before termination to prevent sync of empty state
    let paused = backend.paused_sessions();
    assert_eq!(paused.len(), 2);
    assert!(paused.contains(&"session-1".to_string()));
    assert!(paused.contains(&"session-2".to_string()));

    let terminated = backend.terminated_sessions();
    assert_eq!(terminated.len(), 2);
    assert!(terminated.contains(&"session-1".to_string()));
    assert!(terminated.contains(&"session-2".to_string()));

    // Other project session should remain
    let remaining = backend.list_sessions().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "other-aaaabbbb");
}

#[tokio::test]
async fn test_status_callback_receives_updates() {
    let statuses: Arc<Mutex<Vec<Vec<SessionStatusInfo>>>> = Arc::new(Mutex::new(vec![]));
    let statuses_clone = statuses.clone();

    // Simulate status callback
    let callback = move |status: Vec<SessionStatusInfo>| {
        statuses_clone.lock().unwrap().push(status);
    };

    // Call callback with mock data
    callback(vec![
        SessionStatusInfo {
            name: "frontend-12345678".to_string(),
            status: SessionStatus::Watching,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        },
        SessionStatusInfo {
            name: "backend-12345678".to_string(),
            status: SessionStatus::Scanning,
            staging_progress: None,
            endpoint_files: 0,
            endpoint_dirs: 0,
            polling_interval: None,
            sync_mode: None,
        },
    ]);

    let received = statuses.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].len(), 2);
    assert_eq!(received[0][0].status, SessionStatus::Watching);
    assert_eq!(received[0][1].status, SessionStatus::Scanning);
}

// ============================================================================
// Session Name Format Tests
// ============================================================================

#[test]
fn test_session_name_format() {
    let project_crc32: u32 = 0xaabbccdd;
    let session = desired_session("myapp", project_crc32);

    assert_eq!(session.name, "myapp-aabbccdd");
    assert_eq!(session.project_name, "myapp");
}
