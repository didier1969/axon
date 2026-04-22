use crate::bridge::RuntimeTruthFeed;
use crate::graph::GraphStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_PROXY_TIMEOUT_MS: u64 = 250;
const DEFAULT_PROXY_MAX_RETRIES: u8 = 1;
#[cfg(test)]
const DEFAULT_PROXY_TEST_LATENCY_ENV: &str = "AXON_RUNTIME_COMMAND_PROXY_TEST_LATENCY_MS";
const DEFAULT_PROXY_TIMEOUT_ENV: &str = "AXON_RUNTIME_COMMAND_PROXY_TIMEOUT_MS";
#[cfg(test)]
const DEFAULT_PROXY_MODE: &str = "simulated_local_proxy";
#[cfg(not(test))]
const DEFAULT_PROXY_MODE: &str = "filesystem_command_bridge";
#[cfg(test)]
const DEFAULT_PROXY_TIMEOUT_KIND: &str = "simulated_test_only";
#[cfg(not(test))]
const DEFAULT_PROXY_TIMEOUT_KIND: &str = "bridge_response_deadline";
const REQUESTS_DIR_NAME: &str = "runtime-command-requests";
const RESPONSES_DIR_NAME: &str = "runtime-command-responses";
const BRIDGE_POLL_INTERVAL_MS: u64 = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyRequest {
    pub tool_name: String,
    pub requested_at_ms: i64,
    pub timeout_ms: u64,
    pub simulated_latency_ms: u64,
    pub idempotency_key: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyOwnership {
    pub proxy_role: String,
    pub execution_role: String,
    pub mutation_owner: String,
    pub idempotency_key: String,
    pub duplicate_execution_prevented: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyRetryPolicy {
    pub retryable: bool,
    pub max_attempts: u8,
    pub idempotent: bool,
    pub duplicate_execution_prevented: bool,
    pub recommended_delay_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyTimeout {
    pub timeout_kind: String,
    pub timeout_ms: u64,
    pub simulated_latency_ms: u64,
    pub retryable: bool,
    pub max_retries: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyRefusal {
    pub code: String,
    pub reason: String,
    pub stale: bool,
    pub observed_age_ms: Option<u64>,
    pub stale_after_ms: u64,
    pub degraded_reason: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeCommandProxyAccepted {
    pub request: RuntimeCommandProxyRequest,
    pub ownership: RuntimeCommandProxyOwnership,
    pub timeout: RuntimeCommandProxyTimeout,
    pub retry_policy: RuntimeCommandProxyRetryPolicy,
    pub proxy: Value,
    pub result_contract: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RuntimeCommandProxyDecision {
    Accepted(RuntimeCommandProxyAccepted),
    Refused(RuntimeCommandProxyRefusal),
    TimedOut(RuntimeCommandProxyTimeout),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
struct RuntimeCommandBridgeEnvelope {
    request_id: String,
    tool_name: String,
    requested_at_ms: i64,
    idempotency_key: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
struct RuntimeCommandBridgeReply {
    request_id: String,
    tool_name: String,
    finished_at_ms: i64,
    ok: bool,
    response: Value,
    error_text: String,
}

pub struct RuntimeCommandProxy;

impl RuntimeCommandProxy {
    pub fn enabled() -> bool {
        #[cfg(not(test))]
        {
            return false;
        }
        #[cfg(test)]
        {
            std::env::var("AXON_RUNTIME_COMMAND_PROXY_ENABLED")
                .ok()
                .map(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false)
        }
    }

    pub fn timeout_ms() -> u64 {
        std::env::var(DEFAULT_PROXY_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_PROXY_TIMEOUT_MS)
    }

    pub fn simulated_latency_ms() -> u64 {
        #[cfg(test)]
        {
            return std::env::var(DEFAULT_PROXY_TEST_LATENCY_ENV)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(0);
        }

        #[cfg(not(test))]
        {
            0
        }
    }

    pub fn proxy_mode() -> &'static str {
        DEFAULT_PROXY_MODE
    }

    pub fn timeout_kind() -> &'static str {
        DEFAULT_PROXY_TIMEOUT_KIND
    }

    pub(crate) fn request_for_resume_vectorization(
        arguments: &Value,
    ) -> RuntimeCommandProxyRequest {
        let requested_at_ms = crate::mcp::McpServer::now_unix_ms();
        let arguments_json = serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string());
        let idempotency_key =
            format!("runtime-command-proxy:resume_vectorization:{arguments_json}");
        RuntimeCommandProxyRequest {
            tool_name: "resume_vectorization".to_string(),
            requested_at_ms,
            timeout_ms: Self::timeout_ms(),
            simulated_latency_ms: Self::simulated_latency_ms(),
            idempotency_key,
            arguments: arguments.clone(),
        }
    }

    pub(crate) fn ownership_for_request(
        request: &RuntimeCommandProxyRequest,
    ) -> RuntimeCommandProxyOwnership {
        RuntimeCommandProxyOwnership {
            proxy_role: "brain".to_string(),
            execution_role: "indexer".to_string(),
            mutation_owner: "indexer".to_string(),
            idempotency_key: request.idempotency_key.clone(),
            duplicate_execution_prevented: true,
        }
    }

    pub(crate) fn retry_policy_for_timeout(timeout_ms: u64) -> RuntimeCommandProxyRetryPolicy {
        RuntimeCommandProxyRetryPolicy {
            retryable: true,
            max_attempts: DEFAULT_PROXY_MAX_RETRIES,
            idempotent: true,
            duplicate_execution_prevented: true,
            recommended_delay_ms: timeout_ms,
        }
    }

    pub(crate) fn timeout_for_request(
        request: &RuntimeCommandProxyRequest,
    ) -> RuntimeCommandProxyTimeout {
        RuntimeCommandProxyTimeout {
            timeout_kind: Self::timeout_kind().to_string(),
            timeout_ms: request.timeout_ms,
            simulated_latency_ms: request.simulated_latency_ms,
            retryable: true,
            max_retries: DEFAULT_PROXY_MAX_RETRIES,
        }
    }

    pub fn result_contract_for_resume_vectorization() -> Value {
        json!({
            "follow_up_tool": "job_status",
            "terminal_state_field": "state",
            "raw_status_field": "status",
            "terminal_states": ["completed", "failed"],
            "result_data_fields": ["queued_files", "runtime_mode", "semantic_workers_enabled"],
            "request_field": "request",
            "ownership_field": "ownership",
            "timeout_field": "timeout",
            "retry_policy_field": "retry_policy",
            "error_field": "error_text",
            "notes": "The proxy job response carries explicit request, ownership, timeout, and retry metadata so retries remain idempotent."
        })
    }

    pub fn decision_for_resume_vectorization(
        runtime_truth: &RuntimeTruthFeed,
        arguments: &Value,
    ) -> RuntimeCommandProxyDecision {
        let request = Self::request_for_resume_vectorization(arguments);
        let timeout = Self::timeout_for_request(&request);
        if runtime_truth.stale {
            return RuntimeCommandProxyDecision::Refused(RuntimeCommandProxyRefusal {
                code: "indexer_unavailable".to_string(),
                reason: "indexer_feed_stale".to_string(),
                stale: true,
                observed_age_ms: runtime_truth.observed_age_ms,
                stale_after_ms: runtime_truth.stale_after_ms,
                degraded_reason: runtime_truth.degraded_reason.clone(),
                retryable: false,
            });
        }
        if runtime_truth.degraded_reason.is_some() {
            return RuntimeCommandProxyDecision::Refused(RuntimeCommandProxyRefusal {
                code: "indexer_unavailable".to_string(),
                reason: "indexer_feed_degraded".to_string(),
                stale: false,
                observed_age_ms: runtime_truth.observed_age_ms,
                stale_after_ms: runtime_truth.stale_after_ms,
                degraded_reason: runtime_truth.degraded_reason.clone(),
                retryable: false,
            });
        }
        if request.simulated_latency_ms > request.timeout_ms {
            return RuntimeCommandProxyDecision::TimedOut(timeout);
        }

        let ownership = Self::ownership_for_request(&request);
        let retry_policy = Self::retry_policy_for_timeout(request.timeout_ms);
        RuntimeCommandProxyDecision::Accepted(RuntimeCommandProxyAccepted {
            proxy: json!({
                "enabled": true,
                "mode": Self::proxy_mode(),
                "transport": Self::proxy_mode(),
                "target_role": "indexer"
            }),
            request,
            ownership,
            timeout,
            retry_policy,
            result_contract: Self::result_contract_for_resume_vectorization(),
        })
    }

    pub fn accepted_response_metadata(accepted: &RuntimeCommandProxyAccepted) -> Value {
        json!({
            "outcome": "accepted",
            "proxy": accepted.proxy,
            "request": accepted.request,
            "ownership": accepted.ownership,
            "timeout": accepted.timeout,
            "retry_policy": accepted.retry_policy,
            "result_contract": accepted.result_contract,
        })
    }

    pub fn timeout_response_metadata(
        timeout: &RuntimeCommandProxyTimeout,
        request: &RuntimeCommandProxyRequest,
        ownership: &RuntimeCommandProxyOwnership,
    ) -> Value {
        json!({
            "outcome": "timeout",
            "timeout": timeout,
            "request": request,
            "ownership": ownership,
            "retry_policy": Self::retry_policy_for_timeout(timeout.timeout_ms),
        })
    }

    pub fn refusal_response_metadata(
        refusal: &RuntimeCommandProxyRefusal,
        request: &RuntimeCommandProxyRequest,
        ownership: &RuntimeCommandProxyOwnership,
    ) -> Value {
        json!({
            "outcome": "refused",
            "refusal": refusal,
            "request": request,
            "ownership": ownership,
            "retry_policy": json!({
                "retryable": refusal.retryable,
                "max_attempts": 0,
                "idempotent": true,
                "duplicate_execution_prevented": true,
                "recommended_delay_ms": refusal.stale_after_ms,
            })
        })
    }

    pub fn use_external_bridge() -> bool {
        false
    }

    fn current_run_root() -> Option<PathBuf> {
        std::env::var("AXON_RUN_ROOT")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
    }

    fn indexer_run_root() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("AXON_INDEXER_RUN_ROOT") {
            let candidate = PathBuf::from(path);
            if !candidate.as_os_str().is_empty() {
                return Some(candidate);
            }
        }

        let current = Self::current_run_root()?;
        let parent = current.parent()?.to_path_buf();
        let leaf = current.file_name()?.to_str()?;
        if leaf == "run-brain" {
            return Some(parent.join("run-indexer"));
        }
        if leaf == "run-indexer" {
            return Some(current);
        }
        Some(parent.join("run-indexer"))
    }

    fn requests_dir_for(root: &Path) -> PathBuf {
        root.join(REQUESTS_DIR_NAME)
    }

    fn responses_dir_for(root: &Path) -> PathBuf {
        root.join(RESPONSES_DIR_NAME)
    }

    fn bridge_request_id(request: &RuntimeCommandProxyRequest) -> String {
        format!(
            "{}-{}",
            request.requested_at_ms,
            request
                .idempotency_key
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .take(48)
                .collect::<String>()
        )
    }

    fn persist_bridge_request(
        envelope: &RuntimeCommandBridgeEnvelope,
        requests_dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(requests_dir)?;
        let final_path = requests_dir.join(format!("{}.json", envelope.request_id));
        let temp_path = requests_dir.join(format!("{}.json.tmp", envelope.request_id));
        fs::write(
            &temp_path,
            serde_json::to_vec_pretty(envelope).unwrap_or_else(|_| b"{}".to_vec()),
        )?;
        fs::rename(&temp_path, &final_path)?;
        Ok(final_path)
    }

    pub fn dispatch_resume_vectorization(
        request: &RuntimeCommandProxyRequest,
    ) -> anyhow::Result<Value> {
        let indexer_root = Self::indexer_run_root()
            .ok_or_else(|| anyhow::anyhow!("indexer run root unavailable for runtime proxy"))?;
        let requests_dir = Self::requests_dir_for(&indexer_root);
        let responses_dir = Self::responses_dir_for(&indexer_root);
        fs::create_dir_all(&responses_dir)?;
        let request_id = Self::bridge_request_id(request);
        let response_path = responses_dir.join(format!("{}.json", request_id));
        let envelope = RuntimeCommandBridgeEnvelope {
            request_id: request_id.clone(),
            tool_name: request.tool_name.clone(),
            requested_at_ms: request.requested_at_ms,
            idempotency_key: request.idempotency_key.clone(),
            arguments: request.arguments.clone(),
        };
        let _request_path = Self::persist_bridge_request(&envelope, &requests_dir)?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms.max(1));
        loop {
            if response_path.exists() {
                let reply: RuntimeCommandBridgeReply =
                    serde_json::from_slice(&fs::read(&response_path)?)?;
                let _ = fs::remove_file(&response_path);
                if reply.ok {
                    return Ok(reply.response);
                }
                return Err(anyhow::anyhow!(reply.error_text));
            }
            if Instant::now() >= deadline {
                return Err(anyhow::anyhow!(
                    "runtime command proxy timed out waiting for indexer response"
                ));
            }
            std::thread::sleep(Duration::from_millis(BRIDGE_POLL_INTERVAL_MS));
        }
    }

    pub fn spawn_indexer_bridge_worker(store: Arc<GraphStore>) {
        if cfg!(test) {
            return;
        }
        let Some(run_root) = Self::current_run_root() else {
            return;
        };
        let requests_dir = Self::requests_dir_for(&run_root);
        let responses_dir = Self::responses_dir_for(&run_root);
        std::thread::spawn(move || loop {
            if let Err(err) =
                Self::process_pending_requests(store.clone(), &requests_dir, &responses_dir)
            {
                tracing::warn!("runtime command bridge worker error: {err:#}");
            }
            std::thread::sleep(Duration::from_millis(BRIDGE_POLL_INTERVAL_MS));
        });
    }

    fn process_pending_requests(
        store: Arc<GraphStore>,
        requests_dir: &Path,
        responses_dir: &Path,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(requests_dir)?;
        fs::create_dir_all(responses_dir)?;
        let mut request_files = fs::read_dir(requests_dir)?
            .filter_map(|entry| entry.ok().map(|value| value.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        request_files.sort();

        for request_path in request_files {
            let processing_path = request_path.with_extension("processing");
            if fs::rename(&request_path, &processing_path).is_err() {
                continue;
            }

            let reply = match fs::read(&processing_path).ok().and_then(|bytes| {
                serde_json::from_slice::<RuntimeCommandBridgeEnvelope>(&bytes).ok()
            }) {
                Some(request) => Self::execute_bridge_request(&store, request),
                None => RuntimeCommandBridgeReply {
                    request_id: processing_path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown-request")
                        .to_string(),
                    tool_name: "unknown".to_string(),
                    finished_at_ms: crate::mcp::McpServer::now_unix_ms(),
                    ok: false,
                    response: Value::Null,
                    error_text: "invalid runtime command bridge request".to_string(),
                },
            };

            let response_path = responses_dir.join(format!("{}.json", reply.request_id));
            let _ = fs::write(
                &response_path,
                serde_json::to_vec_pretty(&reply).unwrap_or_else(|_| b"{}".to_vec()),
            );
            let _ = fs::remove_file(&processing_path);
        }
        Ok(())
    }

    fn execute_bridge_request(
        store: &Arc<GraphStore>,
        request: RuntimeCommandBridgeEnvelope,
    ) -> RuntimeCommandBridgeReply {
        let finished_at_ms = crate::mcp::McpServer::now_unix_ms();
        match request.tool_name.as_str() {
            "resume_vectorization" => match store.backfill_file_vectorization_queue() {
                Ok(count) => {
                    let runtime_mode = crate::runtime_mode::AxonRuntimeMode::from_env();
                    RuntimeCommandBridgeReply {
                        request_id: request.request_id,
                        tool_name: request.tool_name,
                        finished_at_ms,
                        ok: true,
                        response: json!({
                            "content": [{
                                "type": "text",
                                "text": format!("Queued {count} file(s) for deferred chunk vectorization.")
                            }],
                            "data": {
                                "queued_files": count,
                                "runtime_mode": runtime_mode.as_str(),
                                "semantic_workers_enabled": runtime_mode.semantic_workers_enabled()
                            }
                        }),
                        error_text: String::new(),
                    }
                }
                Err(err) => RuntimeCommandBridgeReply {
                    request_id: request.request_id,
                    tool_name: request.tool_name,
                    finished_at_ms,
                    ok: false,
                    response: Value::Null,
                    error_text: format!("Resume vectorization error: {err}"),
                },
            },
            _ => RuntimeCommandBridgeReply {
                request_id: request.request_id,
                tool_name: request.tool_name.clone(),
                finished_at_ms,
                ok: false,
                response: Value::Null,
                error_text: format!(
                    "unsupported runtime command proxy tool: {}",
                    request.tool_name
                ),
            },
        }
    }
}
