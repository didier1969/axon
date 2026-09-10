// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902559:
//! Le budget VRAM n'est pas divisé entre les sessions ORT d'un même processus.
//!
//! Critères d'acceptation :
//! 1. La réservation d'arène CUDA est divisée par le nombre de sessions concurrentes à créer,
//!    sans attribuer le plafond entier à chaque session.
//! 2. Le floor minimal par session (512 MB) est respecté.
//! 3. Le pré-vol GPU n'émet pas d'erreur sur un TensorRT absent et non demandé (repli CUDA prévu).

use crate::embedder::gpu_backend::{
    cuda_memory_limit_bytes_for_workers, ort_tensorrt_provider_library_available,
};
use crate::embedder::gpu_preflight::preflight_gpu_libraries;
use crate::test_support::{env_test_lock, EnvVarGuard};

#[test]
fn c1_vram_budget_is_divided_across_concurrent_workers() {
    let _lock = env_test_lock().lock().unwrap();
    let _limit = EnvVarGuard::set("AXON_CUDA_MEMORY_LIMIT_MB", "4000");
    let _sessions = EnvVarGuard::unset("AXON_GPU_CONCURRENT_SESSIONS");
    let _workers = EnvVarGuard::unset("AXON_B2_WORKERS");

    // 1 worker: doit recevoir la totalité (4000 MB)
    let single = cuda_memory_limit_bytes_for_workers(1);
    assert_eq!(single, 4000 * 1024 * 1024);

    // 2 workers: chacun doit recevoir la moitié (2000 MB)
    let double = cuda_memory_limit_bytes_for_workers(2);
    assert_eq!(double, 2000 * 1024 * 1024);

    // 4 workers: chacun doit recevoir le quart (1000 MB)
    let quad = cuda_memory_limit_bytes_for_workers(4);
    assert_eq!(quad, 1000 * 1024 * 1024);
}

#[test]
fn c2_vram_budget_respects_floor_per_worker() {
    let _lock = env_test_lock().lock().unwrap();
    let _limit = EnvVarGuard::set("AXON_CUDA_MEMORY_LIMIT_MB", "1000");
    let _sessions = EnvVarGuard::unset("AXON_GPU_CONCURRENT_SESSIONS");
    let _workers = EnvVarGuard::unset("AXON_B2_WORKERS");

    // 4 workers sur 1000 MB: 1000 / 4 = 250 MB < floor 512 MB.
    // Doit être relevé au floor de 512 MB.
    let per_worker = cuda_memory_limit_bytes_for_workers(4);
    assert_eq!(per_worker, 512 * 1024 * 1024);
}

#[test]
fn c3_preflight_does_not_require_missing_optional_tensorrt() {
    // Si TensorRT n'est pas présent sur disque, le pré-vol ne doit pas lever
    // une erreur statique le déclarant manquant.
    if !ort_tensorrt_provider_library_available() {
        let preflight = preflight_gpu_libraries();
        if let Err(reason) = preflight {
            assert!(
                !reason.contains("libonnxruntime_providers_tensorrt.so"),
                "optional TensorRT provider must not cause preflight failure when absent: {reason}"
            );
        }
    }
}
