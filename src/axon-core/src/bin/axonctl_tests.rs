// REQ-AXO-116 — axonctl Rust-side socket cleanup contract.
//
// The bash stop wrapper (scripts/stop.sh) and the Rust supervisor
// (axonctl stop, this binary) must unlink the same role-specific
// AF_UNIX sockets and pid file or one side leaves orphans that block
// the next start (REQ-AXO-093 root cause). Bash side is exercised by
// scripts/test_axon_socket_lifecycle.sh; Rust side is exercised here.

use super::*;

#[test]
fn cleanup_files_unlinks_role_sockets_and_pid_file() {
    let tmp = std::env::temp_dir().join(format!(
        "axonctl-cleanup-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let telemetry = tmp.join("telemetry.sock");
    let mcp = tmp.join("mcp.sock");
    let pid_file = tmp.join("axon.pid");
    let nonexistent = tmp.join("missing.sock");
    fs::write(&telemetry, b"").unwrap();
    fs::write(&mcp, b"").unwrap();
    fs::write(&pid_file, b"42\n").unwrap();
    assert!(telemetry.exists());
    assert!(mcp.exists());
    assert!(pid_file.exists());
    // Cycle 1: cleanup removes existing files.
    cleanup_files(&[&telemetry, &mcp, &pid_file]);
    assert!(!telemetry.exists(), "telemetry sock should be unlinked");
    assert!(!mcp.exists(), "mcp sock should be unlinked");
    assert!(!pid_file.exists(), "pid file should be unlinked");
    // Cycle 2: idempotent on already-clean paths.
    cleanup_files(&[&telemetry, &mcp, &pid_file]);
    // Missing path is a no-op (catches the orphan-block pattern where
    // a previous cycle already cleaned the file).
    cleanup_files(&[&nonexistent]);
    assert!(!nonexistent.exists());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn instance_config_socket_paths_match_bash_contract() {
    for (kind, role, expected_telemetry, expected_mcp) in [
        (
            InstanceKind::Live,
            RuntimeRole::Brain,
            "/tmp/axon-live-brain-telemetry.sock",
            "/tmp/axon-live-brain-mcp.sock",
        ),
        (
            InstanceKind::Live,
            RuntimeRole::Indexer,
            "/tmp/axon-live-indexer-telemetry.sock",
            "/tmp/axon-live-indexer-mcp.sock",
        ),
        (
            InstanceKind::Dev,
            RuntimeRole::Brain,
            "/tmp/axon-dev-brain-telemetry.sock",
            "/tmp/axon-dev-brain-mcp.sock",
        ),
        (
            InstanceKind::Dev,
            RuntimeRole::Indexer,
            "/tmp/axon-dev-indexer-telemetry.sock",
            "/tmp/axon-dev-indexer-mcp.sock",
        ),
    ] {
        let c = InstanceConfig::new(PathBuf::from("/srv/axon"), kind, role);
        assert_eq!(c.telemetry_sock, PathBuf::from(expected_telemetry));
        assert_eq!(c.mcp_sock, PathBuf::from(expected_mcp));
    }
}
