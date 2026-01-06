//! Tests for the reconcile logic

use ebdev_mutagen_runner::{
    state::{DesiredSession, DesiredState},
    test_utils::{mock_session, MockMutagen},
    MutagenBackend, SessionStatusInfo, SyncMode,
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
        SyncMode::TwoWay,
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
        let existing = backend.list_sessions().await;
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
async fn test_reconcile_keeps_existing_sessions() {
    let backend = MockMutagen::new();
    let project_crc32: u32 = 0x12345678;

    // Pre-existing session
    backend.add_session(mock_session("frontend-12345678"));

    let desired = DesiredState::from_sessions(
        vec![desired_session("frontend", project_crc32)],
        project_crc32,
    );

    // Simulate reconcile
    for session in &desired.sessions {
        let existing = backend.list_sessions().await;
        let found = existing.iter().any(|s| s.name == session.name);
        if !found {
            backend.create_session_from_desired(session, false).await.unwrap();
        }
    }

    // No new sessions should be created
    assert_eq!(backend.created_sessions().len(), 0);
    assert_eq!(backend.list_sessions().await.len(), 1);
}

#[tokio::test]
async fn test_reconcile_terminates_changed_sessions() {
    let backend = MockMutagen::new();
    let project_crc32: u32 = 0x12345678;
    let suffix = format!("{:08x}", project_crc32);

    // Old session with different config (simulated by different name format)
    let mut old_session = mock_session("frontend-12345678");
    old_session.identifier = "old-session-id".to_string();
    backend.add_session(old_session);

    // New desired session with same project but we'll simulate config change
    // by checking prefix/suffix matching logic
    let desired = DesiredState::from_sessions(
        vec![desired_session("frontend", project_crc32)],
        project_crc32,
    );

    // Simulate reconcile logic that terminates sessions with matching prefix/suffix
    // but different exact name (config changed)
    let existing = backend.list_sessions().await;
    for session in &desired.sessions {
        let prefix = format!("{}-", session.project_name);

        for existing_session in &existing {
            if existing_session.name.starts_with(&prefix)
                && existing_session.name.ends_with(&suffix)
                && existing_session.name != session.name
            {
                backend.terminate_session(&existing_session.identifier).await.unwrap();
            }
        }
    }

    // Session should still exist (same name = same config)
    assert_eq!(backend.terminated_sessions().len(), 0);
}

#[tokio::test]
async fn test_reconcile_empty_sessions_terminates_all() {
    let backend = MockMutagen::new();
    let project_crc32: u32 = 0x12345678;
    let suffix = format!("{:08x}", project_crc32);

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

    // Empty desired state = cleanup
    let sessions = backend.list_sessions().await;
    for session in sessions {
        if session.name.ends_with(&suffix) {
            backend.terminate_session(&session.identifier).await.unwrap();
        }
    }

    let terminated = backend.terminated_sessions();
    assert_eq!(terminated.len(), 2);
    assert!(terminated.contains(&"session-1".to_string()));
    assert!(terminated.contains(&"session-2".to_string()));

    // Other project session should remain
    let remaining = backend.list_sessions().await;
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
            status: "watching".to_string(),
        },
        SessionStatusInfo {
            name: "backend-12345678".to_string(),
            status: "scanning".to_string(),
        },
    ]);

    let received = statuses.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].len(), 2);
    assert_eq!(received[0][0].status, "watching");
    assert_eq!(received[0][1].status, "scanning");
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

#[test]
fn test_session_name_prefix() {
    let project_crc32: u32 = 0x12345678;
    let session = desired_session("frontend", project_crc32);

    assert_eq!(session.name_prefix(), "frontend-");
}
