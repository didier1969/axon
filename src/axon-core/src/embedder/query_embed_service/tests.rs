//! REQ-AXO-902547: exercise the real supervisor IPC without loading ORT/GPU.
use super::*;
use crate::runtime_readiness::{self, Subsystem, SubsystemState};
use crate::test_support::EnvVarGuard;

const TEST_MODULE: &str = "embedder::query_embed_service::resilience_tests";

// Isolate process-global readiness/provider state from concurrently running
// tests. The child runs exactly one oracle; worker children run only the IPC
// fixture below. No live database, model or service endpoint is used.
fn enter_isolated_case(name: &str) -> bool {
    if std::env::var("AXON_902547_ISOLATED_CASE").as_deref() == Ok(name) {
        return true;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("{TEST_MODULE}::{name}"), "--nocapture"])
        .env("AXON_902547_ISOLATED_CASE", name)
        .output().unwrap();
    assert!(output.status.success(), "isolated oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    false
}

fn embedder_state() -> SubsystemState {
    runtime_readiness::snapshot_subsystem_reports().into_iter()
        .find(|report| report.subsystem == "embedder").unwrap().state
}

fn configure_fake_worker(root: &Path) -> Vec<EnvVarGuard> {
    let executable = root.join("fake-query-worker.sh");
    fs::write(&executable, format!(
        "#!/bin/sh\nexport AXON_902547_FAKE_SOCKET=\"$2\"\nexec \"$AXON_902547_TEST_EXE\" --exact {TEST_MODULE}::ipc_worker_fixture --ignored --nocapture\n"
    )).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    vec![
        EnvVarGuard::set("AXON_QUERY_EMBED_WORKER_BIN", executable.to_str().unwrap()),
        EnvVarGuard::set("AXON_RUN_ROOT", root.to_str().unwrap()),
        EnvVarGuard::set("AXON_902547_TEST_EXE", std::env::current_exe().unwrap().to_str().unwrap()),
    ]
}

#[test]
fn terminal_inference_failure_invalidates_ready_and_recovers() {
    if !enter_isolated_case("terminal_inference_failure_invalidates_ready_and_recovers") { return; }
    let _lock = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let _env = configure_fake_worker(root.path());
    let mut worker = None;
    {
        let _failure = EnvVarGuard::set("AXON_902547_FAKE_FAIL", "true");
        runtime_readiness::report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);
        let error = dispatch_with_one_retry(
            QueryWorkerSupervisorKind::Primary,
            &mut worker,
            vec!["fail inference".into()],
            Instant::now() + Duration::from_secs(15),
        )
        .unwrap_err();
        assert!(error.to_string().contains("synthetic inference allocation failure"), "{error:#}");
        assert!(worker.is_none(), "both failed workers must be shut down");
        assert!(matches!(embedder_state(), SubsystemState::Failed { ref reason }
            if reason == "isolated_query_worker_inference_failed"),
            "successful handshakes must not mask terminal inference failure: {:?}", embedder_state());
    }
    let recovered = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::Primary,
        &mut worker,
        vec!["recover".into()],
        Instant::now() + Duration::from_secs(15),
    );
    let state = embedder_state();
    if let Some(active) = worker.take() { active.shutdown(); }
    assert_eq!(recovered.unwrap(), vec![vec![1.0]]);
    assert_eq!(state, SubsystemState::Ready);
}

#[test]
fn successful_inference_refreshes_readiness_without_restarting_worker() {
    if !enter_isolated_case("successful_inference_refreshes_readiness_without_restarting_worker") { return; }
    let _lock = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let _env = configure_fake_worker(root.path());
    let _failure = EnvVarGuard::unset("AXON_902547_FAKE_FAIL");
    let mut worker = Some(start_worker(QueryWorkerSupervisorKind::Primary).unwrap());
    let pid = worker.as_ref().unwrap().child.id();
    runtime_readiness::report_subsystem_state(Subsystem::Embedder,
        SubsystemState::Failed { reason: "previous inference failure".into() });
    let result = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::Primary,
        &mut worker,
        vec!["healthy existing connection".into()],
        Instant::now() + Duration::from_secs(15),
    );
    let state = embedder_state();
    let same_worker = worker.as_ref().map(|w| w.child.id()) == Some(pid);
    if let Some(active) = worker.take() { active.shutdown(); }
    assert_eq!(result.unwrap(), vec![vec![1.0]]);
    assert!(same_worker);
    assert_eq!(state, SubsystemState::Ready);
}

#[test]
fn oversized_request_does_not_mark_a_healthy_service_failed() {
    if !enter_isolated_case("oversized_request_does_not_mark_a_healthy_service_failed") { return; }
    runtime_readiness::report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);
    let mut worker = None;
    let error = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::Primary,
        &mut worker,
        vec![String::new(); MAX_TEXTS_PER_REQUEST + 1],
        Instant::now() + Duration::from_secs(15),
    )
    .unwrap_err();
    assert!(error.to_string().contains("maximum"));
    assert!(worker.is_none());
    assert_eq!(embedder_state(), SubsystemState::Ready);
}

#[test]
fn expired_request_in_queue_aborts_without_inference() {
    if !enter_isolated_case("expired_request_in_queue_aborts_without_inference") { return; }
    runtime_readiness::report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);
    let mut worker = None;
    let past_deadline = Instant::now() - Duration::from_millis(100);
    let error = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::Primary,
        &mut worker,
        vec!["should never execute".into()],
        past_deadline,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("expired") || error.to_string().contains("timed out"),
        "error must explicitly report timeout/expiration, got: {error:#}"
    );
    assert!(worker.is_none(), "no worker connection should be opened for expired request");
    assert_eq!(embedder_state(), SubsystemState::Ready);
}

#[test]
fn cpu_fallback_supervisor_isolates_worker_and_leaves_readiness_untouched() {
    // REQ-AXO-902646: Fallback worker operates out-of-process via socket and does not alter canonical readiness.
    if !enter_isolated_case("cpu_fallback_supervisor_isolates_worker_and_leaves_readiness_untouched") { return; }
    let _lock = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let _env = configure_fake_worker(root.path());
    runtime_readiness::report_subsystem_state(Subsystem::Embedder, SubsystemState::Ready);

    let mut worker = None;
    let result = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::CpuFallback,
        &mut worker,
        vec!["cpu fallback embed".into()],
        Instant::now() + Duration::from_secs(15),
    );
    assert_eq!(result.unwrap(), vec![vec![1.0]]);
    assert!(worker.is_some());
    assert_eq!(
        worker.as_ref().unwrap().socket_path.file_name().and_then(|f| f.to_str()),
        Some("query-embed-cpu-fallback.sock"),
        "Fallback worker must use dedicated CPU fallback socket"
    );
    // Canonical Embedder subsystem readiness must stay unaffected
    assert_eq!(embedder_state(), SubsystemState::Ready);
    if let Some(active) = worker.take() { active.shutdown(); }
}

#[test]
fn worker_idle_drop_exit_is_detected_and_respawned_transparently() {
    // REQ-AXO-902646: When child worker exits (e.g. idle timeout reached), supervisor detects
    // the exit proactively and launches a fresh worker for the next request.
    if !enter_isolated_case("worker_idle_drop_exit_is_detected_and_respawned_transparently") { return; }
    let _lock = crate::test_support::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let _env = configure_fake_worker(root.path());

    let mut worker = Some(start_worker(QueryWorkerSupervisorKind::Primary).unwrap());
    let initial_pid = worker.as_ref().unwrap().child.id();

    // Simulate child process exit upon idle_drop
    worker.as_mut().unwrap().child.kill().unwrap();
    worker.as_mut().unwrap().child.wait().unwrap();

    // Next request must transparently respawn a new worker process without returning an error
    let result = dispatch_with_one_retry(
        QueryWorkerSupervisorKind::Primary,
        &mut worker,
        vec!["request after idle exit".into()],
        Instant::now() + Duration::from_secs(15),
    );
    assert_eq!(result.unwrap(), vec![vec![1.0]]);
    assert!(worker.is_some());
    let new_pid = worker.as_ref().unwrap().child.id();
    assert_ne!(initial_pid, new_pid, "Supervisor must have spawned a fresh worker process");
    if let Some(active) = worker.take() { active.shutdown(); }
}

#[test]
#[ignore = "IPC subprocess fixture only; invoked explicitly by the supervisor tests"]
fn ipc_worker_fixture() {
    let socket = std::env::var_os("AXON_902547_FAKE_SOCKET").expect("fixture socket");
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let respond = |stream: &mut UnixStream, request_id, embeddings, error| {
        write_frame(stream, &WireResponse {
            request_id, embeddings, error, provider: "CPU".into(), rss_bytes: 1024,
        }, RESPONSE_FRAME_MAX).unwrap();
    };
    respond(&mut stream, 0, Some(vec![]), None);
    loop {
        match read_frame::<WireRequest>(&mut stream, REQUEST_FRAME_MAX) {
            Ok(WireRequest::Embed { request_id, texts }) => {
                if std::env::var("AXON_902547_FAKE_FAIL").as_deref() == Ok("true") {
                    respond(&mut stream, request_id, None, Some("synthetic inference allocation failure".into()));
                } else {
                    respond(&mut stream, request_id, Some(vec![vec![1.0]; texts.len()]), None);
                }
            }
            Ok(WireRequest::Shutdown) => {
                respond(&mut stream, 0, Some(vec![]), None);
                return;
            }
            Err(_) => return,
        }
    }
}
