// Copyright (c) Didier Stadelmann. All rights reserved.
// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
mod main_background;
mod main_services;
mod main_telemetry;

use axon_core::bridge::BridgeEvent;
use axon_core::embedder::{embedding_lane_config_from_env, EmbeddingLaneConfig};
use axon_core::file_ingress_guard::{FileIngressGuard, SharedFileIngressGuard};
use axon_core::graph::GraphStore;
use axon_core::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
use axon_core::queue::QueueStore;
use axon_core::runtime_mode::AxonRuntimeMode;
use axon_core::runtime_profile::{
    recommend_embedding_lane_sizing, EmbeddingLaneSizing, RuntimeProfile,
};
use axon_core::runtime_writer_guard::WriterGuard;
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tracing::{error, info, warn};

fn results_broadcast_capacity() -> usize {
    const DEFAULT_CAPACITY: usize = 2_048;

    std::env::var("AXON_RESULTS_BROADCAST_CAPACITY")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
        .unwrap_or(DEFAULT_CAPACITY)
}

fn canonical_embedding_provider_request(gpu_present: bool) -> String {
    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if gpu_present {
                "cuda".to_string()
            } else {
                "cpu".to_string()
            }
        })
}

fn canonical_effective_embedding_lane_config() -> EmbeddingLaneConfig {
    let effective = embedding_lane_config_from_env();
    unsafe {
        std::env::set_var(
            "AXON_QUERY_EMBED_WORKERS",
            effective.query_workers.to_string(),
        );
        std::env::set_var("AXON_VECTOR_WORKERS", effective.vector_workers.to_string());
        std::env::set_var("AXON_GRAPH_WORKERS", effective.graph_workers.to_string());
        std::env::set_var(
            "AXON_CHUNK_BATCH_SIZE",
            effective.chunk_batch_size.to_string(),
        );
        std::env::set_var(
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            effective.file_vectorization_batch_size.to_string(),
        );
        std::env::set_var(
            "AXON_GRAPH_BATCH_SIZE",
            effective.graph_batch_size.to_string(),
        );
    }
    effective
}

fn apply_canonical_ort_runtime_env(gpu_execution_requested: bool) {
    if !gpu_execution_requested {
        return;
    }

    if std::env::var("OMP_NUM_THREADS").is_err() {
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "1");
            std::env::set_var("AXON_ORT_OMP_AUTOCONFIGURED", "true");
        }
    }

    if std::env::var("OMP_WAIT_POLICY").is_err() {
        unsafe {
            std::env::set_var("OMP_WAIT_POLICY", "PASSIVE");
        }
    }

    if std::env::var("AXON_ORT_INTRA_THREADS").is_err() {
        if let Ok(omp_threads) = std::env::var("OMP_NUM_THREADS") {
            let omp_threads = omp_threads.trim();
            if !omp_threads.is_empty() {
                unsafe {
                    std::env::set_var("AXON_ORT_INTRA_THREADS", omp_threads);
                    std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
                }
            }
        }
    }

    let wsl_cuda_lib_dir = "/usr/lib/wsl/lib";
    if std::path::Path::new(wsl_cuda_lib_dir).exists() {
        let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let already_present = current
            .split(':')
            .any(|segment| segment.trim() == wsl_cuda_lib_dir);
        if !already_present {
            let next = if current.trim().is_empty() {
                wsl_cuda_lib_dir.to_string()
            } else {
                format!("{wsl_cuda_lib_dir}:{current}")
            };
            unsafe {
                std::env::set_var("LD_LIBRARY_PATH", next);
            }
        }
    }
}

fn apply_canonical_watcher_runtime_env() {
    if std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").is_err() {
        unsafe {
            std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "128");
        }
    }
}

fn apply_canonical_ort_thread_defaults_from_openmp() {
    if std::env::var("AXON_ORT_INTRA_THREADS").is_ok() {
        return;
    }
    let Ok(omp_threads) = std::env::var("OMP_NUM_THREADS") else {
        return;
    };
    let omp_threads = omp_threads.trim();
    if omp_threads.is_empty() {
        return;
    }
    unsafe {
        std::env::set_var("AXON_ORT_INTRA_THREADS", omp_threads);
        std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
    }
}

fn apply_canonical_embedding_lane_sizing_defaults(lane_sizing: &EmbeddingLaneSizing) {
    for (env_name, marker_name, value) in [
        (
            "AXON_QUERY_EMBED_WORKERS",
            "AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED",
            lane_sizing.query_workers.to_string(),
        ),
        (
            "AXON_VECTOR_WORKERS",
            "AXON_VECTOR_WORKERS_AUTOCONFIGURED",
            lane_sizing.vector_workers.to_string(),
        ),
        (
            "AXON_GRAPH_WORKERS",
            "AXON_GRAPH_WORKERS_AUTOCONFIGURED",
            lane_sizing.graph_workers.to_string(),
        ),
        (
            "AXON_CHUNK_BATCH_SIZE",
            "AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.chunk_batch_size.to_string(),
        ),
        (
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            "AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.file_vectorization_batch_size.to_string(),
        ),
        (
            "AXON_GRAPH_BATCH_SIZE",
            "AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.graph_batch_size.to_string(),
        ),
    ] {
        if std::env::var(env_name).is_err() {
            unsafe {
                std::env::set_var(env_name, value);
                std::env::set_var(marker_name, "true");
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    axon_core::runtime_boot::run_monolith()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_canonical_embedding_lane_sizing_defaults, apply_canonical_ort_runtime_env,
        apply_canonical_ort_thread_defaults_from_openmp, apply_canonical_watcher_runtime_env,
        canonical_effective_embedding_lane_config, canonical_embedding_provider_request,
    };
    use axon_core::runtime_profile::EmbeddingLaneSizing;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cuda_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(canonical_embedding_provider_request(true), "cuda");
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cpu_without_gpu() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(canonical_embedding_provider_request(false), "cpu");
    }

    #[test]
    fn canonical_embedding_provider_request_respects_explicit_cpu_override_even_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        }

        assert_eq!(canonical_embedding_provider_request(true), "cpu");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }
    }

    #[test]
    fn canonical_effective_embedding_lane_config_caps_gpu_vector_workers_in_env() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "2");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }

        let config = canonical_effective_embedding_lane_config();
        assert_eq!(config.vector_workers, 2);
        assert_eq!(
            std::env::var("AXON_VECTOR_WORKERS").unwrap(),
            "2",
            "L'environnement doit exposer le sizing effectif et non le sizing recommande"
        );

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
        }
    }

    #[test]
    fn apply_canonical_embedding_lane_sizing_defaults_marks_autoconfigured_values() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
        }

        apply_canonical_embedding_lane_sizing_defaults(&EmbeddingLaneSizing {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 0,
            chunk_batch_size: 64,
            file_vectorization_batch_size: 24,
            graph_batch_size: 8,
        });

        assert_eq!(
            std::env::var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );

        unsafe {
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_sets_gpu_safe_openmp_defaults() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }

        apply_canonical_ort_runtime_env(true);

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "1");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "PASSIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "1");
        assert_eq!(
            std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").unwrap(),
            "true"
        );
        if std::path::Path::new("/usr/lib/wsl/lib").exists() {
            assert!(std::env::var("LD_LIBRARY_PATH")
                .unwrap_or_default()
                .split(':')
                .any(|segment| segment == "/usr/lib/wsl/lib"));
        }

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_preserves_explicit_openmp_configuration() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "3");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::set_var("LD_LIBRARY_PATH", "/tmp/custom-lib");
        }

        apply_canonical_ort_runtime_env(true);

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "4");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "ACTIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "3");
        assert!(std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());
        let ld_library_path = std::env::var("LD_LIBRARY_PATH").unwrap();
        assert!(ld_library_path.contains("/tmp/custom-lib"));
        if std::path::Path::new("/usr/lib/wsl/lib").exists() {
            assert!(ld_library_path
                .split(':')
                .any(|segment| segment == "/usr/lib/wsl/lib"));
        }

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_leaves_cpu_hosts_unchanged() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }

        apply_canonical_ort_runtime_env(false);

        assert!(
            std::env::var("OMP_NUM_THREADS").is_err(),
            "CPU hosts should not receive GPU-specific OpenMP overrides by default"
        );
        assert!(
            std::env::var("OMP_WAIT_POLICY").is_err(),
            "CPU hosts should not receive GPU-specific OpenMP overrides by default"
        );
        assert!(std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());
        assert!(
            std::env::var("LD_LIBRARY_PATH").is_err(),
            "CPU hosts should not receive GPU-specific loader overrides by default"
        );
    }

    #[test]
    fn apply_canonical_ort_thread_defaults_from_openmp_sets_missing_ort_threads() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }

        apply_canonical_ort_thread_defaults_from_openmp();

        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "4");
        assert_eq!(
            std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").unwrap(),
            "true"
        );

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }
    }

    #[test]
    fn apply_canonical_ort_thread_defaults_from_openmp_preserves_explicit_ort_threads() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "3");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }

        apply_canonical_ort_thread_defaults_from_openmp();

        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "3");
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
        }
    }

    #[test]
    fn apply_canonical_watcher_runtime_env_sets_default_budget() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }

        apply_canonical_watcher_runtime_env();

        assert_eq!(
            std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").unwrap(),
            "128"
        );

        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }
    }

    #[test]
    fn apply_canonical_watcher_runtime_env_preserves_explicit_budget() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "32");
        }

        apply_canonical_watcher_runtime_env();

        assert_eq!(
            std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").unwrap(),
            "32"
        );

        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }
    }
}
