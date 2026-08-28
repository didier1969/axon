//! Isolated query-embedding service (REQ-AXO-902547 / DEC-AXO-901676).
//!
//! ORT and CUDA own large monotonic arenas. One child owns one model, exits
//! after a bounded idle period, and is recreated on demand by a tiny Brain-side
//! supervisor. Process exit, not allocator trimming, is the reclamation proof.

use super::{
    query_embed_effective_provider, query_reload_generation, query_worker_compute_label,
    set_query_worker_compute_gpu, QueryEmbeddingRequest, SemanticWorkerPool,
};
use anyhow::{anyhow, Context};
use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const REQUEST_FRAME_MAX: usize = 4 * 1024 * 1024;
const RESPONSE_FRAME_MAX: usize = 16 * 1024 * 1024;
const DEFAULT_IDLE_SECS: u64 = 300;
const DEFAULT_START_TIMEOUT_SECS: u64 = 15;
// Leaves roughly 1.8 GiB for the ORT-free Brain parent while keeping the hot
// parent+worker aggregate below the 4 GiB contract.
const DEFAULT_QUERY_RSS_LIMIT_MB: u64 = 2_200;
const DEFAULT_QUERY_GPU_LIMIT_MB: u64 = 2_200;
const MAX_TEXTS_PER_REQUEST: usize = 64;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum WireRequest {
    Embed { request_id: u64, texts: Vec<String> },
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct WireResponse {
    request_id: u64,
    embeddings: Option<Vec<Vec<f32>>>,
    error: Option<String>,
    provider: String,
    rss_bytes: u64,
}

struct WorkerConnection {
    child: Child,
    stream: UnixStream,
    socket_path: PathBuf,
}

impl WorkerConnection {
    fn shutdown(mut self) {
        let _ = write_frame(&mut self.stream, &WireRequest::Shutdown, REQUEST_FRAME_MAX);
        let _ = read_frame::<WireResponse>(&mut self.stream, RESPONSE_FRAME_MAX);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub(super) fn spawn_supervisor(
    requests: Receiver<QueryEmbeddingRequest>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("axon-query-supervisor".into())
        .spawn(move || supervise(requests))
}

fn supervise(requests: Receiver<QueryEmbeddingRequest>) {
    let mut observed_reload_generation = query_reload_generation();
    let mut worker = match start_worker() {
        Ok(worker) => {
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::Embedder,
                crate::runtime_readiness::SubsystemState::Ready,
            );
            Some(worker)
        }
        Err(error) => {
            tracing::warn!("query embedding worker did not prewarm: {error:#}");
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::Embedder,
                crate::runtime_readiness::SubsystemState::Failed {
                    reason: "isolated_query_worker_start_failed".to_string(),
                },
            );
            None
        }
    };

    while let Ok(request) = requests.recv() {
        let current_reload_generation = query_reload_generation();
        let reload_requested = current_reload_generation != observed_reload_generation;
        if reload_requested || request.texts.is_empty() {
            if let Some(active) = worker.take() {
                active.shutdown();
            }
            worker = match start_worker() {
                Ok(started) => {
                    crate::runtime_readiness::report_subsystem_state(
                        crate::runtime_readiness::Subsystem::Embedder,
                        crate::runtime_readiness::SubsystemState::Ready,
                    );
                    Some(started)
                }
                Err(error) => {
                    tracing::warn!("query embedding worker reload failed: {error:#}");
                    crate::runtime_readiness::report_subsystem_state(
                        crate::runtime_readiness::Subsystem::Embedder,
                        crate::runtime_readiness::SubsystemState::Failed {
                            reason: "isolated_query_worker_reload_failed".to_string(),
                        },
                    );
                    None
                }
            };
            observed_reload_generation = current_reload_generation;
        }
        if request.texts.is_empty() {
            let _ = request.reply.send(Ok(Vec::new()));
            continue;
        }

        let result = dispatch_with_one_retry(&mut worker, request.texts);
        let _ = request.reply.send(result);
    }

    if let Some(active) = worker {
        active.shutdown();
    }
}

fn dispatch_with_one_retry(
    worker: &mut Option<WorkerConnection>,
    texts: Vec<String>,
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.len() > MAX_TEXTS_PER_REQUEST {
        return Err(anyhow!(
            "query embedding request contains {} texts; maximum is {}",
            texts.len(),
            MAX_TEXTS_PER_REQUEST
        ));
    }
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..=1 {
        if worker.is_none() {
            match start_worker() {
                Ok(started) => {
                    crate::runtime_readiness::report_subsystem_state(
                        crate::runtime_readiness::Subsystem::Embedder,
                        crate::runtime_readiness::SubsystemState::Ready,
                    );
                    *worker = Some(started);
                }
                Err(error) => {
                    crate::runtime_readiness::report_subsystem_state(
                        crate::runtime_readiness::Subsystem::Embedder,
                        crate::runtime_readiness::SubsystemState::Failed {
                            reason: "isolated_query_worker_start_failed".to_string(),
                        },
                    );
                    return Err(error);
                }
            }
        }
        let active = worker.as_mut().expect("worker ensured");
        let round_trip = (|| {
            write_frame(
                &mut active.stream,
                &WireRequest::Embed {
                    request_id,
                    texts: texts.clone(),
                },
                REQUEST_FRAME_MAX,
            )?;
            let response: WireResponse = read_frame(&mut active.stream, RESPONSE_FRAME_MAX)?;
            if response.request_id != request_id {
                return Err(anyhow!(
                    "query embedding response id mismatch: expected {request_id}, got {}",
                    response.request_id
                ));
            }
            set_query_worker_compute_gpu(response.provider.eq_ignore_ascii_case("GPU"));
            match (response.embeddings, response.error) {
                (Some(embeddings), None) => Ok(embeddings),
                (_, Some(error)) => Err(anyhow!(error)),
                _ => Err(anyhow!("query embedding worker returned an empty response")),
            }
        })();

        match round_trip {
            Ok(value) => return Ok(value),
            Err(error) if attempt == 0 => {
                tracing::warn!("query embedding worker disconnected; restarting once: {error:#}");
                if let Some(stale) = worker.take() {
                    stale.shutdown();
                }
            }
            Err(error) => {
                if let Some(stale) = worker.take() {
                    stale.shutdown();
                }
                return Err(error);
            }
        }
    }
    unreachable!()
}

fn start_worker() -> anyhow::Result<WorkerConnection> {
    let socket_path = query_socket_path();
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create query worker run dir {}", parent.display()))?;
    }
    match fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("remove stale query worker socket"),
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind query worker socket {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .context("restrict query worker socket permissions")?;
    listener.set_nonblocking(true)?;

    let binary = query_worker_binary()?;
    let provider = query_embed_effective_provider();
    let mut child = Command::new(&binary)
        .arg("--socket")
        .arg(&socket_path)
        .env("AXON_QUERY_EMBED_PROVIDER", provider)
        .env(
            "AXON_ORT_INTRA_THREADS",
            std::env::var("AXON_QUERY_ORT_INTRA_THREADS").unwrap_or_else(|_| "1".into()),
        )
        .env(
            "AXON_CUDA_MEMORY_SOFT_LIMIT_MB",
            std::env::var("AXON_QUERY_GPU_MEMORY_LIMIT_MB")
                .unwrap_or_else(|_| DEFAULT_QUERY_GPU_LIMIT_MB.to_string()),
        )
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;

    let timeout = env_u64(
        "AXON_QUERY_EMBED_START_TIMEOUT_SECS",
        DEFAULT_START_TIMEOUT_SECS,
    );
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    let _ = fs::remove_file(&socket_path);
                    return Err(anyhow!("query worker exited during startup: {status}"));
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(&socket_path);
                    return Err(anyhow!("query worker startup exceeded {timeout}s"));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error).context("accept query worker connection"),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;

    let hello: WireResponse =
        read_frame(&mut stream, RESPONSE_FRAME_MAX).context("read query worker ready handshake")?;
    if hello.request_id != 0 || hello.error.is_some() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow!(
            "invalid query worker handshake: {}",
            hello.error.unwrap_or_else(|| "missing ready marker".into())
        ));
    }
    set_query_worker_compute_gpu(hello.provider.eq_ignore_ascii_case("GPU"));
    tracing::info!(
        pid = child.id(),
        provider = %hello.provider,
        rss_mb = hello.rss_bytes / 1024 / 1024,
        "isolated query embedding worker ready"
    );
    Ok(WorkerConnection {
        child,
        stream,
        socket_path,
    })
}

pub fn run_worker(socket_path: &Path) -> anyhow::Result<()> {
    // SAFETY: the worker is a fresh single-threaded process at this point; no
    // other thread can concurrently access the process environment.
    unsafe {
        std::env::set_var(
            "AXON_ORT_INTRA_THREADS",
            std::env::var("AXON_QUERY_ORT_INTRA_THREADS").unwrap_or_else(|_| "1".into()),
        );
        std::env::set_var("OMP_WAIT_POLICY", "PASSIVE");
    }
    let mut model = SemanticWorkerPool::build_text_embedding_model("query", 0)
        .ok_or_else(|| anyhow!("query embedding model failed to load"))?;
    let provider = query_worker_compute_label()
        .unwrap_or("UNKNOWN")
        .to_string();
    let rss_limit_bytes = env_u64("AXON_QUERY_EMBED_MAX_RSS_MB", DEFAULT_QUERY_RSS_LIMIT_MB)
        .saturating_mul(1024 * 1024);
    let rss = current_rss_bytes();
    if rss > rss_limit_bytes {
        return Err(anyhow!(
            "query worker RSS {} MiB exceeds {} MiB budget",
            rss / 1024 / 1024,
            rss_limit_bytes / 1024 / 1024
        ));
    }

    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect query worker socket {}", socket_path.display()))?;
    let idle_secs = env_u64("AXON_QUERY_EMBED_IDLE_SECS", DEFAULT_IDLE_SECS);
    if idle_secs > 0 {
        stream.set_read_timeout(Some(Duration::from_secs(idle_secs)))?;
    }
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    write_frame(
        &mut stream,
        &WireResponse {
            request_id: 0,
            embeddings: Some(Vec::new()),
            error: None,
            provider: provider.clone(),
            rss_bytes: rss,
        },
        RESPONSE_FRAME_MAX,
    )?;

    loop {
        let request = match read_frame::<WireRequest>(&mut stream, REQUEST_FRAME_MAX) {
            Ok(request) => request,
            Err(error) if is_timeout(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        match request {
            WireRequest::Shutdown => {
                write_frame(
                    &mut stream,
                    &WireResponse {
                        request_id: 0,
                        embeddings: Some(Vec::new()),
                        error: None,
                        provider: provider.clone(),
                        rss_bytes: current_rss_bytes(),
                    },
                    RESPONSE_FRAME_MAX,
                )?;
                return Ok(());
            }
            WireRequest::Embed { request_id, texts } => {
                let result = if texts.len() > MAX_TEXTS_PER_REQUEST {
                    Err(anyhow!("too many texts in query embedding request"))
                } else {
                    model.embed(texts, None)
                };
                let rss = current_rss_bytes();
                let over_budget = rss > rss_limit_bytes;
                let (embeddings, error) = match result {
                    Ok(value) if !over_budget => (Some(value), None),
                    Ok(_) => (
                        None,
                        Some(format!(
                            "query worker RSS {} MiB exceeds {} MiB budget",
                            rss / 1024 / 1024,
                            rss_limit_bytes / 1024 / 1024
                        )),
                    ),
                    Err(error) => (None, Some(error.to_string())),
                };
                write_frame(
                    &mut stream,
                    &WireResponse {
                        request_id,
                        embeddings,
                        error,
                        provider: provider.clone(),
                        rss_bytes: rss,
                    },
                    RESPONSE_FRAME_MAX,
                )?;
                if over_budget {
                    return Ok(());
                }
            }
        }
    }
}

fn query_socket_path() -> PathBuf {
    let run_root = std::env::var_os("AXON_RUN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".axon/run-brain"));
    let root = if run_root.is_absolute() {
        run_root
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(run_root)
    };
    root.join("query-embed.sock")
}

fn query_worker_binary() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("AXON_QUERY_EMBED_WORKER_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().context("resolve current Axon executable")?;
    Ok(current.with_file_name("axon-query-embed-worker"))
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn current_rss_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb * 1024)
            })
        })
        .unwrap_or(0)
}

fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T, max: usize) -> anyhow::Result<()> {
    let payload = rmp_serde::to_vec_named(value)?;
    if payload.len() > max {
        return Err(anyhow!(
            "IPC frame {} exceeds {} byte limit",
            payload.len(),
            max
        ));
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(
    reader: &mut impl Read,
    max: usize,
) -> anyhow::Result<T> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > max {
        return Err(anyhow!("IPC frame {length} exceeds {max} byte limit"));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    Ok(rmp_serde::from_slice(&payload)?)
}

fn is_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messagepack_frame_round_trips() {
        let request = WireRequest::Embed {
            request_id: 42,
            texts: vec!["why does Axon exist?".into()],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request, REQUEST_FRAME_MAX).unwrap();
        let decoded: WireRequest = read_frame(&mut bytes.as_slice(), REQUEST_FRAME_MAX).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let mut bytes = ((REQUEST_FRAME_MAX as u32) + 1).to_be_bytes().to_vec();
        bytes.extend_from_slice(&[0; 8]);
        let error =
            read_frame::<WireRequest>(&mut bytes.as_slice(), REQUEST_FRAME_MAX).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn default_limits_match_the_runtime_contract() {
        assert_eq!(DEFAULT_IDLE_SECS, 300);
        assert_eq!(DEFAULT_QUERY_RSS_LIMIT_MB, 2_200);
        assert_eq!(DEFAULT_QUERY_GPU_LIMIT_MB, 2_200);
    }
}
