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
// REQ-AXO-902566 — STRICTEMENT sous `embedder::QUERY_EMBED_TIMEOUT` (15 s). Les
// deux étaient égaux, et les deux budgets démarrent quasi simultanément sur le
// chemin dispatch : c'était pile ou face. Quand le budget EXTERNE gagnait,
// l'appelant recevait « embedding timed out » — une forme que le test
// d'acceptation ACCEPTE — pendant que le vrai diagnostic ne survivait que dans un
// `tracing::warn`. Un vert qui masque. L'invariant est épinglé par
// `start_timeout_leaves_headroom_under_the_caller_budget`.
const DEFAULT_START_TIMEOUT_SECS: u64 = 12;
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
    // REQ-AXO-902566 — résolu EN PREMIER : on échoue avant de créer un répertoire
    // ou de lier une socket, et le chemin est ensuite disponible pour tout le
    // contexte. Le message d'absence est déjà structuré ; c'est le reste que
    // `with_context` habille.
    let binary = query_worker_binary()?;
    start_worker_with(&binary).with_context(|| worker_unavailable_message(&binary))
}

/// REQ-AXO-902566 — enveloppé par `start_worker`. C'est ICI, et pas dans
/// `dispatch_with_one_retry` ni dans `supervise`, que l'erreur doit être
/// structurée : seul ce site connaît le chemin résolu (le REQ exige de le
/// nommer), et il couvre d'un coup les trois appelants — préchauffage, rechargement
/// et dispatch. `supervise` serait le pire choix : il mélange les échecs de
/// démarrage et ceux de round-trip, et y coller « worker unavailable » serait faux
/// la moitié du temps.
fn start_worker_with(binary: &Path) -> anyhow::Result<WorkerConnection> {
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

    let provider = query_embed_effective_provider();
    let mut child = Command::new(binary)
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
        // REQ-AXO-902566 — les deux sorties d'erreur suivantes retirent la socket ;
        // celle-ci ne le faisait pas, et laissait un fichier orphelin dans l'arbre à
        // chaque échec de spawn (observé : `.axon/run-brain/query-embed.sock`).
        .map_err(|error| {
            let _ = fs::remove_file(&socket_path);
            error
        })
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

const WORKER_BIN_NAME: &str = "axon-query-embed-worker";

/// REQ-AXO-902566 — les emplacements candidats, dans l'ordre. PURE : rien n'est
/// lu sur disque ni dans l'environnement, tout entre par paramètre, si bien que
/// l'ORDRE lui-même est testable hermétiquement.
fn worker_binary_candidates(
    current_exe: &Path,
    cargo_target_dir: Option<&Path>,
    allow_build_tree: bool,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // 1. Vérité de production : les cinq artefacts sont installés dans UN
    //    répertoire, et `promote_live_safe.sh` refuse de publier si l'un manque.
    //    Le frère existe donc par contrat de release vérifié, jamais par chance.
    if let Some(dir) = current_exe.parent() {
        out.push(dir.join(WORKER_BIN_NAME));
    }
    // 2. Disposition cargo standard : le harnais de test vit dans `<profil>/deps/`,
    //    les `[[bin]]` un cran au-dessus.
    if let Some(dir) = current_exe.parent().and_then(Path::parent) {
        out.push(dir.join(WORKER_BIN_NAME));
    }
    // 3. `deps/` DÉPORTÉ. Sur ce poste `.axon/cargo-target/debug/deps` est un lien
    //    symbolique vers un cache hors arbre ; `current_exe()` lit `/proc/self/exe`
    //    et rend le chemin RÉSOLU, si bien que (2) atterrit dans ce cache et non
    //    dans l'arbre de build. C'est ce qui invalide le remède que REQ-AXO-902566
    //    prescrivait (« chercher dans le parent de deps/ »), et pourquoi il faut
    //    passer par la racine de cible.
    //    Builds DEBUG uniquement : un artefact de release ne doit jamais adopter
    //    le binaire d'un arbre de build.
    if allow_build_tree {
        if let Some(root) = cargo_target_dir {
            for profile in ["debug", "release"] {
                out.push(root.join(profile).join(WORKER_BIN_NAME));
            }
        }
    }
    out
}

/// REQ-AXO-902566 — le message d'échec quand aucun candidat n'est exécutable.
/// Contient `worker unavailable` et `Use structural search` (les formes que le
/// contrat appelant reconnaît) ET nomme chaque chemin cherché.
fn worker_binary_missing_message(candidates: &[PathBuf]) -> String {
    format!(
        "MCP real-time query embedding worker unavailable: no `{WORKER_BIN_NAME}` \
         executable found (searched: {}). Run `cargo build --bins`, or set \
         AXON_QUERY_EMBED_WORKER_BIN. Use structural search.",
        candidates
            .iter()
            .map(|c| c.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// REQ-AXO-902566 — LA phrase orientée appelant pour « le worker isolé n'a pas pu
/// démarrer ». Enveloppée par `Context`, `to_string()` rend exactement ceci, et
/// `{:#}` rend toujours la chaîne de causes complète : l'ajout est strictement
/// additif, l'opérateur ne perd aucun diagnostic.
fn worker_unavailable_message(binary: &Path) -> String {
    format!(
        "MCP real-time query embedding worker unavailable ({}). Use structural search.",
        binary.display()
    )
}

fn resolve_worker_binary(
    override_path: Option<PathBuf>,
    current_exe: &Path,
    cargo_target_dir: Option<&Path>,
    allow_build_tree: bool,
    is_executable: impl Fn(&Path) -> bool,
) -> anyhow::Result<PathBuf> {
    // Un override opérateur est honoré VERBATIM : s'il est faux, l'erreur de spawn
    // le nommera. Retomber en silence sur un autre binaire masquerait une intention
    // explicite.
    if let Some(path) = override_path {
        return Ok(path);
    }
    let candidates = worker_binary_candidates(current_exe, cargo_target_dir, allow_build_tree);
    candidates
        .iter()
        .find(|candidate| is_executable(candidate))
        .cloned()
        .ok_or_else(|| anyhow!("{}", worker_binary_missing_message(&candidates)))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn query_worker_binary() -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe().context("resolve current Axon executable")?;
    resolve_worker_binary(
        std::env::var_os("AXON_QUERY_EMBED_WORKER_BIN").map(PathBuf::from),
        &current,
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .as_deref(),
        cfg!(debug_assertions),
        is_executable_file,
    )
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

    // ── REQ-AXO-902566 ──────────────────────────────────────────────────────
    // `query_worker_binary()` résolvait le worker comme le SEUL frère de
    // `current_exe()`. Sous `cargo test`, `current_exe()` est le harnais, qui vit
    // dans `<profil>/deps/` — jamais là où Cargo pose les `[[bin]]`. La porte de
    // build `GUI-AXO-1034` était donc structurellement infranchissable.
    //
    // Ces tests portent sur des fonctions PURES : aucun accès disque, aucune
    // variable d'environnement, donc aucune dépendance à la disposition du
    // répertoire de build (critère d'acceptation 3 du REQ).

    fn faux_predicat(existants: &[&str]) -> impl Fn(&Path) -> bool {
        // La closure POSSÈDE ses chemins : pas d'emprunt, donc pas de durée de vie
        // à faire porter par le type de retour.
        let existants: Vec<PathBuf> = existants.iter().map(PathBuf::from).collect();
        move |p: &Path| existants.iter().any(|e| e.as_path() == p)
    }

    #[test]
    fn worker_binary_prefers_the_sibling_of_the_running_executable() {
        // En production les cinq artefacts vivent dans UN répertoire : le premier
        // candidat gagne toujours, les suivants ne sont jamais atteints.
        let resolved = resolve_worker_binary(
            None,
            Path::new("/opt/axon/bin/axon-brain"),
            Some(Path::new("/w/target")),
            true,
            faux_predicat(&[
                "/opt/axon/bin/axon-query-embed-worker",
                "/w/target/debug/axon-query-embed-worker",
            ]),
        )
        .expect("le frère doit être retenu");
        assert_eq!(
            resolved,
            PathBuf::from("/opt/axon/bin/axon-query-embed-worker")
        );
    }

    #[test]
    fn worker_binary_falls_back_to_the_bin_dir_above_cargo_deps() {
        // Disposition cargo standard (CI hors devenv) : `deps/` est un vrai
        // répertoire, le binaire est un cran au-dessus.
        let resolved = resolve_worker_binary(
            None,
            Path::new("/w/target/debug/deps/axon_core-abc123"),
            None,
            true,
            faux_predicat(&["/w/target/debug/axon-query-embed-worker"]),
        )
        .expect("le parent de deps/ doit être retenu");
        assert_eq!(resolved, PathBuf::from("/w/target/debug/axon-query-embed-worker"));
    }

    #[test]
    fn worker_binary_uses_cargo_target_dir_when_deps_is_relocated() {
        // LE cas de ce poste, et celui que le remède prescrit par le REQ ne couvre
        // PAS : `deps/` est un lien symbolique vers un cache hors arbre, et
        // `current_exe()` rend le chemin résolu. Le parent du parent vaut donc
        // `/data/codex-cache`, où aucun binaire n'est jamais posé.
        let resolved = resolve_worker_binary(
            None,
            Path::new("/data/codex-cache/axon-debug-deps-20260827/axon_core-abc123"),
            Some(Path::new("/home/u/axon/.axon/cargo-target")),
            true,
            faux_predicat(&["/home/u/axon/.axon/cargo-target/debug/axon-query-embed-worker"]),
        )
        .expect("la racine de cible doit rattraper le deps/ déporté");
        assert_eq!(
            resolved,
            PathBuf::from("/home/u/axon/.axon/cargo-target/debug/axon-query-embed-worker")
        );
    }

    #[test]
    fn release_builds_never_adopt_a_build_tree_worker() {
        // La sûreté de production, sous forme de test : hors debug la branche
        // « arbre de build » n'existe pas, et le message ne la mentionne pas.
        let erreur = resolve_worker_binary(
            None,
            Path::new("/data/codex-cache/axon-debug-deps-20260827/axon_core-abc123"),
            Some(Path::new("/home/u/axon/.axon/cargo-target")),
            false,
            faux_predicat(&["/home/u/axon/.axon/cargo-target/debug/axon-query-embed-worker"]),
        )
        .expect_err("un artefact de release ne doit jamais adopter un binaire d'arbre de build");
        assert!(
            !erreur.to_string().contains("cargo-target"),
            "le chemin d'arbre de build ne doit même pas être cherché : {erreur}"
        );
    }

    #[test]
    fn missing_worker_binary_is_reported_as_unavailable_and_names_every_candidate() {
        // Critère d'acceptation 2 : l'erreur porte une forme reconnue par le
        // contrat appelant ET nomme le chemin cherché.
        let erreur = resolve_worker_binary(
            None,
            Path::new("/w/target/debug/deps/axon_core-abc123"),
            Some(Path::new("/w/target")),
            true,
            faux_predicat(&[]),
        )
        .expect_err("aucun candidat exécutable");
        let msg = erreur.to_string();
        assert!(
            msg.contains("worker unavailable") && msg.contains("Use structural search"),
            "le message doit satisfaire le contrat appelant : {msg}"
        );
        assert!(
            msg.contains("/w/target/debug/deps/axon-query-embed-worker")
                && msg.contains("/w/target/debug/axon-query-embed-worker"),
            "chaque candidat cherché doit être nommé : {msg}"
        );
    }

    #[test]
    fn explicit_override_is_honoured_verbatim() {
        // L'erreur de l'opérateur lui est rendue, pas remplacée en douce.
        let resolved = resolve_worker_binary(
            Some(PathBuf::from("/nowhere/custom-worker")),
            Path::new("/opt/axon/bin/axon-brain"),
            None,
            false,
            faux_predicat(&["/opt/axon/bin/axon-query-embed-worker"]),
        )
        .expect("un override est honoré tel quel");
        assert_eq!(resolved, PathBuf::from("/nowhere/custom-worker"));
    }

    #[test]
    fn worker_unavailable_message_satisfies_the_caller_contract() {
        // Épingle ici la sous-chaîne que `cpu_query_service_tests.rs` cherche :
        // une reformulation casse ce test (instantané, clair) plutôt que celui-là
        // (lent, opaque).
        let msg = worker_unavailable_message(Path::new("/opt/axon/bin/axon-query-embed-worker"));
        assert!(msg.contains("worker unavailable"));
        assert!(msg.contains("Use structural search"));
        assert!(msg.contains("/opt/axon/bin/axon-query-embed-worker"));
    }

    #[test]
    fn context_wrapping_keeps_the_cause_for_the_operator() {
        // Les DEUX moitiés du contrat : l'appelant gagne une phrase, l'opérateur
        // ne perd rien. C'est ce qui rend l'ajout strictement additif.
        let binaire = Path::new("/opt/axon/bin/axon-query-embed-worker");
        let brute: anyhow::Result<()> =
            Err(io::Error::from(io::ErrorKind::NotFound)).context("spawn /opt/axon/bin/x");
        let enveloppee = brute
            .with_context(|| worker_unavailable_message(binaire))
            .expect_err("erreur attendue");
        assert!(
            enveloppee.to_string().contains("worker unavailable"),
            "l'appelant lit la phrase structurée : {enveloppee}"
        );
        assert!(
            format!("{enveloppee:#}").contains("spawn /opt/axon/bin/x"),
            "l'opérateur garde la cause d'origine : {enveloppee:#}"
        );
    }

    #[test]
    fn start_timeout_leaves_headroom_under_the_caller_budget() {
        // Le budget INTERNE doit gagner la course contre le `recv_timeout` externe,
        // sinon un échec de démarrage remonte sous le « embedding timed out »
        // générique — que le test d'acceptation accepte, ce qui rendrait le vert
        // MASQUANT au lieu de correct.
        assert!(
            DEFAULT_START_TIMEOUT_SECS + 2 <= super::super::QUERY_EMBED_TIMEOUT.as_secs(),
            "start={DEFAULT_START_TIMEOUT_SECS}s doit rester sous le budget appelant"
        );
    }
}
