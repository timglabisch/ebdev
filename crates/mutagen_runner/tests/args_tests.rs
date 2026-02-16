//! Tests for argument building

use ebdev_mutagen_runner::{
    build_create_args,
    config::{PermissionsConfig, PollingConfig},
    state::DesiredSession,
    SyncMode,
};
use std::path::PathBuf;

/// Helper to create a basic DesiredSession
fn basic_session() -> DesiredSession {
    DesiredSession::new(
        "test-12345678".to_string(),
        "test".to_string(),
        PathBuf::from("/local/path"),
        "docker://container/remote".to_string(),
        SyncMode::TwoWay,
        vec![],
    )
}

// ============================================================================
// Basic Argument Tests
// ============================================================================

#[test]
fn test_build_args_basic_structure() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    assert_eq!(args[0], "sync");
    assert_eq!(args[1], "create");
    assert_eq!(args[2], "/local/path");
    assert_eq!(args[3], "docker://container/remote");
    assert_eq!(args[4], "--name=test-12345678");
}

#[test]
fn test_build_args_sync_mode_two_way() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--sync-mode=two-way-safe".to_string()));
}

#[test]
fn test_build_args_sync_mode_one_way_create() {
    let mut session = basic_session();
    session.mode = SyncMode::OneWayCreate;
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--sync-mode=one-way-safe".to_string()));
}

#[test]
fn test_build_args_sync_mode_one_way_replica() {
    let mut session = basic_session();
    session.mode = SyncMode::OneWayReplica;
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--sync-mode=one-way-replica".to_string()));
}

// ============================================================================
// Ignore Pattern Tests
// ============================================================================

#[test]
fn test_build_args_no_ignore_patterns() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    // .ebdev is always excluded by default
    let ignore_count = args.iter().filter(|a| a.starts_with("--ignore=")).count();
    assert_eq!(ignore_count, 1);
    assert!(args.contains(&"--ignore=.ebdev".to_string()));
}

#[test]
fn test_build_args_single_ignore_pattern() {
    let mut session = basic_session();
    session.ignore = vec!["node_modules".to_string()];
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--ignore=node_modules".to_string()));
}

#[test]
fn test_build_args_multiple_ignore_patterns() {
    let mut session = basic_session();
    session.ignore = vec![
        ".git".to_string(),
        "node_modules".to_string(),
        "vendor".to_string(),
        "*.log".to_string(),
    ];
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--ignore=.git".to_string()));
    assert!(args.contains(&"--ignore=node_modules".to_string()));
    assert!(args.contains(&"--ignore=vendor".to_string()));
    assert!(args.contains(&"--ignore=*.log".to_string()));
}

// ============================================================================
// Watch Mode Tests
// ============================================================================

#[test]
fn test_build_args_no_watch_mode() {
    let session = basic_session();
    let args = build_create_args(&session, true);

    assert!(args.contains(&"--watch-mode=no-watch".to_string()));
}

#[test]
fn test_build_args_default_watch_mode() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    // No watch mode arg when using default (filesystem watching)
    let has_watch_arg = args.iter().any(|a| a.starts_with("--watch-mode="));
    assert!(!has_watch_arg);
}

#[test]
fn test_build_args_polling_enabled() {
    let mut session = basic_session();
    session.polling = PollingConfig {
        enabled: true,
        interval: 30,
    };
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--watch-mode=force-poll".to_string()));
    assert!(args.contains(&"--watch-polling-interval=30".to_string()));
}

#[test]
fn test_build_args_no_watch_overrides_polling() {
    let mut session = basic_session();
    session.polling = PollingConfig {
        enabled: true,
        interval: 30,
    };
    let args = build_create_args(&session, true); // no_watch = true

    // no-watch should take precedence
    assert!(args.contains(&"--watch-mode=no-watch".to_string()));
    assert!(!args.contains(&"--watch-mode=force-poll".to_string()));
}

// ============================================================================
// Permissions Tests
// ============================================================================

#[test]
fn test_build_args_default_permissions() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    // Default: 666 for files, 777 for directories (octal)
    assert!(args.contains(&"--default-file-mode=666".to_string()));
    assert!(args.contains(&"--default-directory-mode=777".to_string()));
}

#[test]
fn test_build_args_custom_permissions() {
    let mut session = basic_session();
    session.permissions = PermissionsConfig {
        default_file_mode: 0o644,
        default_directory_mode: 0o755,
    };
    let args = build_create_args(&session, false);

    assert!(args.contains(&"--default-file-mode=644".to_string()));
    assert!(args.contains(&"--default-directory-mode=755".to_string()));
}

// ============================================================================
// Full Command Tests
// ============================================================================

#[test]
fn test_build_args_full_command() {
    let mut session = DesiredSession::new(
        "myapp-aabbccdd".to_string(),
        "myapp".to_string(),
        PathBuf::from("/home/user/project"),
        "docker://webserver/var/www".to_string(),
        SyncMode::OneWayCreate,
        vec![".git".to_string(), "node_modules".to_string()],
    );
    session.polling = PollingConfig {
        enabled: true,
        interval: 15,
    };
    session.permissions = PermissionsConfig {
        default_file_mode: 0o644,
        default_directory_mode: 0o755,
    };

    let args = build_create_args(&session, false);

    // Verify complete argument list
    assert_eq!(args[0], "sync");
    assert_eq!(args[1], "create");
    assert_eq!(args[2], "/home/user/project");
    assert_eq!(args[3], "docker://webserver/var/www");
    assert!(args.contains(&"--name=myapp-aabbccdd".to_string()));
    assert!(args.contains(&"--sync-mode=one-way-safe".to_string()));
    assert!(args.contains(&"--ignore=.git".to_string()));
    assert!(args.contains(&"--ignore=node_modules".to_string()));
    assert!(args.contains(&"--watch-mode=force-poll".to_string()));
    assert!(args.contains(&"--watch-polling-interval=15".to_string()));
    assert!(args.contains(&"--default-file-mode=644".to_string()));
    assert!(args.contains(&"--default-directory-mode=755".to_string()));
}

#[test]
fn test_build_args_argument_order() {
    let session = basic_session();
    let args = build_create_args(&session, false);

    // First 5 args should always be in order
    assert_eq!(&args[0..2], &["sync", "create"]);

    // Alpha and beta paths should come before flags
    // sync, create, alpha, beta, --name=..., then --sync-mode, --ignore=.ebdev, ...
    let first_flag_idx = args.iter().position(|a| a.starts_with("--")).unwrap();
    assert_eq!(first_flag_idx, 4);
}
