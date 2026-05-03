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

// REQ-AXO-097 — auto-restart cross-process restart half. Role
// health probe must distinguish: Healthy (alive AND cmdline match),
// Dead (pidfile present, process gone — stale pidfile), PidReused
// (pid alive but cmdline does not match — different process now
// owns this PID), Absent (no pidfile at all). is_healthy is true
// only for Healthy; auto-restart triggers on anything else.

#[test]
fn role_health_is_absent_when_no_pid_file() {
    let tmp = std::env::temp_dir().join(format!(
        "axonctl-rh-absent-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    // Build a config whose pid_file path lives under tmp and was
    // never written. We bypass the canonical InstanceConfig::new
    // path computation so the test does not depend on the real
    // /tmp/axon-*-runtime/ layout.
    let mut config = InstanceConfig::new(
        PathBuf::from("/srv/axon"),
        InstanceKind::Dev,
        RuntimeRole::Brain,
    );
    config.pid_file = tmp.join("brain.pid");
    assert_eq!(role_health(&config), RoleHealth::Absent);
    assert!(!RoleHealth::Absent.is_healthy());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn role_health_is_dead_when_pid_file_points_at_dead_process() {
    let tmp = std::env::temp_dir().join(format!(
        "axonctl-rh-dead-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let mut config = InstanceConfig::new(
        PathBuf::from("/srv/axon"),
        InstanceKind::Dev,
        RuntimeRole::Brain,
    );
    config.pid_file = tmp.join("brain.pid");
    // PID 1 is the init process; we instead use a never-allocated
    // PID by writing a sentinel that we know is dead. PID 0 is
    // never assigned to a process and `kill -0 0` returns ESRCH.
    fs::write(&config.pid_file, "0\n").unwrap();
    let health = role_health(&config);
    assert!(
        matches!(health, RoleHealth::Dead),
        "stale pidfile pointing at PID 0 must report Dead, got {health:?}"
    );
    assert!(!health.is_healthy());
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn role_health_is_pid_reused_when_alive_but_cmdline_mismatch() {
    let tmp = std::env::temp_dir().join(format!(
        "axonctl-rh-reused-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let mut config = InstanceConfig::new(
        PathBuf::from("/srv/axon"),
        InstanceKind::Dev,
        RuntimeRole::Brain,
    );
    config.pid_file = tmp.join("brain.pid");
    // The current test process is alive but its cmdline (the
    // cargo test runner) does not match an `axon-brain` cmdline.
    let our_pid = std::process::id() as i32;
    fs::write(&config.pid_file, format!("{our_pid}\n")).unwrap();
    let health = role_health(&config);
    assert!(
        matches!(health, RoleHealth::PidReused),
        "alive PID with non-matching cmdline must report PidReused, got {health:?}"
    );
    assert!(!health.is_healthy());
    let _ = fs::remove_dir_all(&tmp);
}

// REQ-AXO-151 — role contract: brain MUST expose its MCP surface; indexer
// MUST expose telemetry socket. A live process whose contract is broken
// reports `degraded`, never `healthy`.
//
// REQ-AXO-156 — MCP availability = socket present OR `hydra_http_port`
// listening; HTTP-only brains do not violate the contract.

fn socket(name: &str, exists: bool) -> SocketStatus {
    SocketStatus {
        name: name.into(),
        path: format!("/tmp/{name}.sock"),
        exists,
    }
}

#[test]
fn role_contract_violations_empty_for_brain_with_mcp_socket() {
    let sockets = vec![socket("telemetry", true), socket("mcp", true)];
    let violations = compute_role_contract_violations(RuntimeRole::Brain, &sockets, false);
    assert!(
        violations.is_empty(),
        "brain with mcp socket must satisfy contract, got {violations:?}"
    );
}

#[test]
fn role_contract_violations_brain_with_http_listening_no_socket_satisfies_contract() {
    // REQ-AXO-156 — production brains may serve MCP via HTTP only.
    let sockets = vec![socket("telemetry", true), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::Brain, &sockets, true);
    assert!(
        violations.is_empty(),
        "brain with HTTP MCP listening should satisfy contract, got {violations:?}"
    );
}

#[test]
fn role_contract_violations_brain_without_mcp_socket_or_http_flags_mcp_unavailable() {
    let sockets = vec![socket("telemetry", true), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::Brain, &sockets, false);
    assert_eq!(violations, vec!["mcp_unavailable".to_string()]);
}

#[test]
fn role_contract_violations_indexer_without_telemetry_flags_telemetry_socket_missing() {
    // Indexer telemetry is socket-only; mcp_http listening is irrelevant.
    let sockets = vec![socket("telemetry", false), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::Indexer, &sockets, true);
    assert_eq!(violations, vec!["telemetry_socket_missing".to_string()]);
}

#[test]
fn role_contract_violations_indexer_with_telemetry_satisfies_contract_even_without_mcp() {
    let sockets = vec![socket("telemetry", true), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::Indexer, &sockets, false);
    assert!(violations.is_empty(), "indexer should not require mcp");
}

#[test]
fn role_contract_violations_all_role_requires_both_mcp_and_telemetry() {
    let sockets = vec![socket("telemetry", false), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::All, &sockets, false);
    assert!(violations.contains(&"mcp_unavailable".to_string()));
    assert!(violations.contains(&"telemetry_socket_missing".to_string()));
}

#[test]
fn role_contract_violations_all_role_with_http_mcp_only_flags_telemetry_only() {
    // REQ-AXO-156 — HTTP MCP satisfies the brain side; telemetry still needs
    // its socket.
    let sockets = vec![socket("telemetry", false), socket("mcp", false)];
    let violations = compute_role_contract_violations(RuntimeRole::All, &sockets, true);
    assert_eq!(violations, vec!["telemetry_socket_missing".to_string()]);
}
