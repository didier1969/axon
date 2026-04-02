// Copyright (c) Didier Stadelmann. All rights reserved.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axon_core::file_ingress_guard::{guard_metrics_snapshot, SharedFileIngressGuard};
use axon_core::fs_watcher::{self, HOT_PRIORITY};
use axon_core::graph::GraphStore;
use axon_core::graph::PendingFile;
use axon_core::ingress_buffer::{
    record_ingress_flush, IngressMetricsSnapshot, SharedIngressBuffer,
};
use axon_core::queue::{ProcessingMode, QueueStore};
use axon_core::runtime_observability::{
    duckdb_memory_snapshot, duckdb_storage_snapshot, process_memory_snapshot,
};
use axon_core::scanner::Scanner;
use axon_core::service_guard;
use axon_core::service_guard::ServicePressure;
use axon_core::watcher_probe;
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
struct WatchTarget {
    path: PathBuf,
    recursive: bool,
}

#[derive(Debug, Clone, Default)]
struct AdmissionPlan {
    selected: Vec<AdmissionSelection>,
    deferred: Vec<PendingFile>,
    oversized: Vec<PendingFile>,
    degraded: Vec<String>,
}

#[derive(Debug, Clone)]
struct AdmissionSelection {
    file: PendingFile,
    mode: ProcessingMode,
}

const CLAIM_MODE_SENTINEL: u8 = u8::MAX;
const FAIRNESS_PROMOTION_DEFER_THRESHOLD: u32 = 3;
const OVERSIZED_PROBATION_DEFER_THRESHOLD: u32 = 3;
const INGRESS_PROMOTER_POLL_INTERVAL_MS: u64 = 50;
const INGRESS_HOT_FLUSH_WINDOW_MS: u64 = 100;
const INGRESS_BULK_FLUSH_WINDOW_MS: u64 = 400;
const INGRESS_HINT_FLUSH_WINDOW_MS: u64 = 150;
const INGRESS_MAX_BATCH_SIZE: usize = 512;
const INGRESS_FORCE_BATCH_SIZE: usize = 1_024;
const MEMORY_RECLAIMER_POLL_INTERVAL_SECS: u64 = 15;

static OVERSIZED_REFUSALS_TOTAL: AtomicU64 = AtomicU64::new(0);
static DEGRADED_MODE_ENTRIES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_SUCCESSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_REPORTED_CLAIM_MODE: AtomicU8 = AtomicU8::new(CLAIM_MODE_SENTINEL);
static HOST_PRESSURE_SAMPLER: LazyLock<Mutex<HostPressureSampler>> =
    LazyLock::new(|| Mutex::new(HostPressureSampler::default()));

#[derive(Debug, Clone)]
pub(crate) struct RuntimeTelemetrySnapshot {
    pub budget_bytes: u64,
    pub reserved_bytes: u64,
    pub exhaustion_ratio: f64,
    pub queue_depth: usize,
    pub claim_mode: String,
    pub service_pressure: String,
    pub oversized_refusals_total: u64,
    pub degraded_mode_entries_total: u64,
    pub guard_hits: u64,
    pub guard_misses: u64,
    pub guard_bypassed_total: u64,
    pub guard_hydrated_entries: u64,
    pub guard_hydration_duration_ms: u64,
    pub ingress_enabled: bool,
    pub ingress_buffered_entries: usize,
    pub ingress_subtree_hints: usize,
    pub ingress_collapsed_total: u64,
    pub ingress_flush_count: u64,
    pub ingress_last_flush_duration_ms: u64,
    pub ingress_last_promoted_count: u64,
    pub cpu_load: f64,
    pub ram_load: f64,
    pub io_wait: f64,
    pub host_state: String,
    pub host_guidance_slots: usize,
    pub rss_bytes: u64,
    pub rss_anon_bytes: u64,
    pub rss_file_bytes: u64,
    pub rss_shmem_bytes: u64,
    pub db_file_bytes: u64,
    pub db_wal_bytes: u64,
    pub db_total_bytes: u64,
    pub duckdb_memory_bytes: u64,
    pub duckdb_temporary_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct HostPressureSnapshot {
    cpu_load: f64,
    ram_load: f64,
    io_wait: f64,
}

#[derive(Debug, Clone, Copy)]
struct ProcStatSample {
    total: u64,
    idle: u64,
    iowait: u64,
}

#[derive(Debug, Default)]
struct HostPressureSampler {
    previous: Option<ProcStatSample>,
}

pub(crate) fn start_memory_watchdog() {
    std::thread::spawn(|| {
        let page_size = 4096;
        let limit_bytes = memory_limit_bytes();
        let mut above_limit = false;
        loop {
            if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                if let Some(rss_pages) = parse_rss_from_statm(&content) {
                    let rss_bytes = rss_pages * page_size;
                    if rss_bytes > limit_bytes {
                        if !above_limit {
                            error!(
                            "CRITICAL: Memory threshold reached ({} GB). Holding runtime in degraded mode instead of suicide...",
                            rss_bytes / 1024 / 1024 / 1024
                            );
                            above_limit = true;
                        }
                    } else if above_limit {
                        warn!(
                            "Memory watchdog: RSS returned below threshold ({} GB).",
                            rss_bytes / 1024 / 1024 / 1024
                        );
                        above_limit = false;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

pub(crate) fn spawn_memory_reclaimer(
    queue: Arc<QueueStore>,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(MEMORY_RECLAIMER_POLL_INTERVAL_SECS));

        if !memory_reclaimer_enabled() {
            continue;
        }

        if queue.common_len() > 0 {
            continue;
        }

        let ingress_metrics = ingress_buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .metrics_snapshot();
        if ingress_metrics.buffered_entries > 0 || ingress_metrics.subtree_hints > 0 {
            continue;
        }

        let process_memory = process_memory_snapshot();
        let min_anon_bytes = memory_reclaimer_min_anon_bytes();
        if process_memory.rss_anon_bytes < min_anon_bytes {
            continue;
        }

        MEMORY_TRIM_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
        if axon_core::runtime_observability::malloc_trim_system_allocator() {
            MEMORY_TRIM_SUCCESSES_TOTAL.fetch_add(1, Ordering::Relaxed);
            info!(
                "Memory reclaimer trimmed system allocator after idle period (rss_anon={} MiB).",
                process_memory.rss_anon_bytes / 1024 / 1024
            );
        }
    });
}

pub(crate) fn spawn_autonomous_ingestor(store: Arc<GraphStore>, queue: Arc<QueueStore>) {
    tokio::spawn(async move {
        info!("Autonomous Ingestor: Ignition. Monitoring DuckDB for work...");
        let memory_limit = memory_limit_bytes();
        let mut last_mode: Option<ClaimMode> = None;
        loop {
            let policy = claim_policy(
                queue.common_len(),
                queue.memory_budget_snapshot().exhaustion_ratio,
                current_rss_bytes(),
                memory_limit,
                service_guard::current_pressure(),
            );
            if last_mode != Some(policy.mode) {
                record_claim_mode_transition(policy.mode);
                info!(
                    "Autonomous Ingestor claim mode={} claim_count={} sleep_ms={} queue_len={} service_pressure={:?}",
                    policy.mode.label(),
                    policy.claim_count,
                    policy.sleep.as_millis(),
                    queue.common_len(),
                    service_guard::current_pressure(),
                );
                last_mode = Some(policy.mode);
            }
            if policy.claim_count > 0 {
                if let Ok(candidates) = store.fetch_pending_candidates(
                    policy.claim_count.saturating_mul(4).max(policy.claim_count),
                ) {
                    let plan = plan_admissions(&queue, candidates, policy.claim_count);

                    if !plan.deferred.is_empty() {
                        let deferred_paths = plan
                            .deferred
                            .iter()
                            .map(|file| file.path.clone())
                            .collect::<Vec<_>>();
                        if let Err(err) = store.mark_pending_files_deferred(&deferred_paths) {
                            warn!(
                                "Autonomous Ingestor failed to record deferred fairness debt: {}",
                                err
                            );
                        }
                    }

                    for oversized in &plan.oversized {
                        record_oversized_refusal();
                        if let Err(err) =
                            store.mark_file_oversized_for_current_budget(&oversized.path)
                        {
                            warn!(
                                "Autonomous Ingestor failed to mark {} as oversized: {}",
                                oversized.path, err
                            );
                        }
                    }

                    let selected_modes = plan
                        .selected
                        .iter()
                        .map(|selection| (selection.file.path.clone(), selection.mode))
                        .collect::<std::collections::HashMap<_, _>>();

                    if let Ok(files) = store.claim_pending_paths(
                        &plan
                            .selected
                            .iter()
                            .map(|selection| selection.file.path.clone())
                            .collect::<Vec<_>>(),
                    ) {
                        if !files.is_empty() {
                            debug!(
                                "Autonomous Ingestor: Feeding {} tasks to workers.",
                                files.len()
                            );
                            enqueue_claimed_files(&store, &queue, files, &selected_modes);
                        }
                    } else if !plan.selected.is_empty() {
                        warn!("Autonomous Ingestor failed to claim selected pending files.");
                    }
                }
            }
            tokio::time::sleep(policy.sleep).await;
        }
    });
}

pub(crate) fn runtime_telemetry_snapshot(
    store: &GraphStore,
    queue: &QueueStore,
    ingress_buffer: &SharedIngressBuffer,
) -> RuntimeTelemetrySnapshot {
    let budget = queue.memory_budget_snapshot();
    let queue_depth = queue.common_len();
    let service_pressure = service_guard::current_pressure();
    let policy = claim_policy(
        queue_depth,
        budget.exhaustion_ratio,
        current_rss_bytes(),
        memory_limit_bytes(),
        service_pressure,
    );
    let host_pressure = sample_host_pressure();
    let guard_metrics = guard_metrics_snapshot();
    let ingress_metrics = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .metrics_snapshot();
    let process_memory = process_memory_snapshot();
    let storage = duckdb_storage_snapshot(store);
    let duckdb_memory = duckdb_memory_snapshot(store);

    RuntimeTelemetrySnapshot {
        budget_bytes: budget.budget_bytes,
        reserved_bytes: budget.reserved_bytes,
        exhaustion_ratio: budget.exhaustion_ratio,
        queue_depth,
        claim_mode: policy.mode.label().to_string(),
        service_pressure: service_pressure_label(service_pressure).to_string(),
        oversized_refusals_total: OVERSIZED_REFUSALS_TOTAL.load(Ordering::Relaxed),
        degraded_mode_entries_total: DEGRADED_MODE_ENTRIES_TOTAL.load(Ordering::Relaxed),
        guard_hits: guard_metrics.hits,
        guard_misses: guard_metrics.misses,
        guard_bypassed_total: guard_metrics.bypassed_total,
        guard_hydrated_entries: guard_metrics.hydrated_entries,
        guard_hydration_duration_ms: guard_metrics.hydration_duration_ms,
        ingress_enabled: ingress_metrics.enabled,
        ingress_buffered_entries: ingress_metrics.buffered_entries,
        ingress_subtree_hints: ingress_metrics.subtree_hints,
        ingress_collapsed_total: ingress_metrics.collapsed_total,
        ingress_flush_count: ingress_metrics.flush_count,
        ingress_last_flush_duration_ms: ingress_metrics.last_flush_duration_ms,
        ingress_last_promoted_count: ingress_metrics.last_promoted_count,
        cpu_load: host_pressure.cpu_load,
        ram_load: host_pressure.ram_load,
        io_wait: host_pressure.io_wait,
        host_state: host_state_label(policy.mode, budget.exhaustion_ratio, service_pressure)
            .to_string(),
        host_guidance_slots: policy.claim_count,
        rss_bytes: process_memory.rss_bytes,
        rss_anon_bytes: process_memory.rss_anon_bytes,
        rss_file_bytes: process_memory.rss_file_bytes,
        rss_shmem_bytes: process_memory.rss_shmem_bytes,
        db_file_bytes: storage.db_file_bytes,
        db_wal_bytes: storage.db_wal_bytes,
        db_total_bytes: storage.db_total_bytes,
        duckdb_memory_bytes: duckdb_memory.memory_usage_bytes,
        duckdb_temporary_bytes: duckdb_memory.temporary_storage_bytes,
    }
}

fn should_flush_ingress_buffer(metrics: &IngressMetricsSnapshot, elapsed: Duration) -> bool {
    if metrics.buffered_entries == 0 && metrics.subtree_hints == 0 {
        return false;
    }

    if metrics.buffered_entries >= INGRESS_FORCE_BATCH_SIZE {
        return true;
    }

    if metrics.hot_entries > 0 && elapsed >= Duration::from_millis(INGRESS_HOT_FLUSH_WINDOW_MS) {
        return true;
    }

    if metrics.subtree_hints > 0 && elapsed >= Duration::from_millis(INGRESS_HINT_FLUSH_WINDOW_MS) {
        return true;
    }

    metrics.scan_entries > 0 && elapsed >= Duration::from_millis(INGRESS_BULK_FLUSH_WINDOW_MS)
}

fn flush_ingress_buffer_once(
    store: Arc<GraphStore>,
    projects_root: &str,
    file_ingress_guard: &SharedFileIngressGuard,
    ingress_buffer: &SharedIngressBuffer,
) -> anyhow::Result<usize> {
    let metrics = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .metrics_snapshot();
    if !metrics.enabled || (metrics.buffered_entries == 0 && metrics.subtree_hints == 0) {
        return Ok(0);
    }

    let batch_size = if metrics.hot_entries > 0 {
        INGRESS_MAX_BATCH_SIZE.min(256)
    } else {
        INGRESS_MAX_BATCH_SIZE
    };
    let batch = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .drain_batch(batch_size);
    if batch.files.is_empty() && batch.tombstones.is_empty() && batch.subtree_hints.is_empty() {
        return Ok(0);
    }

    let started_at = Instant::now();
    let promoted = store.promote_ingress_batch(&batch)?;

    if !batch.files.is_empty() {
        let paths = batch
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        if let Ok(rows) = store.fetch_file_ingress_rows(&paths) {
            let mut locked = file_ingress_guard
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for row in rows {
                locked.record_committed_row(row);
            }
        }
    }

    if !batch.tombstones.is_empty() {
        let mut locked = file_ingress_guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in &batch.tombstones {
            locked.record_tombstone(Path::new(path));
        }
    }

    if !batch.subtree_hints.is_empty() {
        let scanner = Scanner::new(projects_root);
        for hint in &batch.subtree_hints {
            scanner.scan_subtree_with_guard_and_ingress(
                store.clone(),
                Path::new(&hint.path),
                Some(file_ingress_guard),
                Some(ingress_buffer),
            );
        }
    }

    let promoted_count = promoted.promoted_files + promoted.promoted_tombstones;
    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    record_ingress_flush(elapsed_ms, promoted_count);
    watcher_probe::record(
        "ingress.promoted",
        None,
        format!(
            "files={} tombstones={} subtree_hints={} duration_ms={}",
            promoted.promoted_files,
            promoted.promoted_tombstones,
            batch.subtree_hints.len(),
            elapsed_ms
        ),
    );

    Ok(promoted_count + batch.subtree_hints.len())
}

fn sample_host_pressure() -> HostPressureSnapshot {
    let cpu_sample = read_proc_stat_sample();
    let ram_load = read_ram_load_percent();

    match HOST_PRESSURE_SAMPLER.lock() {
        Ok(mut sampler) => {
            let previous = sampler.previous;
            sampler.previous = cpu_sample;

            let (cpu_load, io_wait) = match (previous, cpu_sample) {
                (Some(previous), Some(current)) => compute_cpu_and_io_percent(previous, current),
                _ => (0.0, 0.0),
            };

            HostPressureSnapshot {
                cpu_load,
                ram_load,
                io_wait,
            }
        }
        Err(_) => HostPressureSnapshot {
            cpu_load: 0.0,
            ram_load,
            io_wait: 0.0,
        },
    }
}

fn read_proc_stat_sample() -> Option<ProcStatSample> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().find(|line| line.starts_with("cpu "))?;
    let mut values = line.split_whitespace().skip(1);
    let user = values.next()?.parse::<u64>().ok()?;
    let nice = values.next()?.parse::<u64>().ok()?;
    let system = values.next()?.parse::<u64>().ok()?;
    let idle = values.next()?.parse::<u64>().ok()?;
    let iowait = values.next()?.parse::<u64>().ok()?;
    let irq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let softirq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let steal = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let total = user + nice + system + idle + iowait + irq + softirq + steal;

    Some(ProcStatSample {
        total,
        idle,
        iowait,
    })
}

fn compute_cpu_and_io_percent(previous: ProcStatSample, current: ProcStatSample) -> (f64, f64) {
    let total_delta = current.total.saturating_sub(previous.total);
    if total_delta == 0 {
        return (0.0, 0.0);
    }

    let idle_delta = current.idle.saturating_sub(previous.idle);
    let iowait_delta = current.iowait.saturating_sub(previous.iowait);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    let cpu_load = ((busy_delta as f64) / (total_delta as f64) * 100.0).clamp(0.0, 100.0);
    let io_wait = ((iowait_delta as f64) / (total_delta as f64) * 100.0).clamp(0.0, 100.0);

    (cpu_load, io_wait)
}

fn read_ram_load_percent() -> f64 {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => content,
        Err(_) => return 0.0,
    };

    let mut total_kb = None;
    let mut available_kb = None;
    let mut free_kb = None;
    let mut buffers_kb = None;
    let mut cached_kb = None;

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or_default();
        let value = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        match key {
            "MemTotal:" => total_kb = Some(value),
            "MemAvailable:" => available_kb = Some(value),
            "MemFree:" => free_kb = Some(value),
            "Buffers:" => buffers_kb = Some(value),
            "Cached:" => cached_kb = Some(value),
            _ => {}
        }
    }

    let total_kb = total_kb.unwrap_or(0);
    if total_kb == 0 {
        return 0.0;
    }

    let available_kb = available_kb
        .unwrap_or(free_kb.unwrap_or(0) + buffers_kb.unwrap_or(0) + cached_kb.unwrap_or(0));
    let used_kb = total_kb.saturating_sub(available_kb);

    ((used_kb as f64) / (total_kb as f64) * 100.0).clamp(0.0, 100.0)
}

fn plan_admissions(
    queue: &QueueStore,
    candidates: Vec<PendingFile>,
    max_count: usize,
) -> AdmissionPlan {
    if max_count == 0 || candidates.is_empty() {
        return AdmissionPlan::default();
    }

    let mut remaining_budget = queue.remaining_budget_bytes();
    let mut plan = AdmissionPlan::default();
    let mut hot_candidates = Vec::new();
    let mut normal_candidates = Vec::new();

    for candidate in candidates {
        if candidate.priority >= HOT_PRIORITY {
            hot_candidates.push(candidate);
        } else {
            normal_candidates.push(candidate);
        }
    }

    fill_admission_plan(
        queue,
        &mut remaining_budget,
        max_count,
        &mut plan,
        hot_candidates,
    );
    if plan.selected.len() < max_count {
        fill_admission_plan(
            queue,
            &mut remaining_budget,
            max_count,
            &mut plan,
            normal_candidates,
        );
    } else {
        plan.deferred.extend(normal_candidates);
    }

    plan
}

fn fill_admission_plan(
    queue: &QueueStore,
    remaining_budget: &mut u64,
    max_count: usize,
    plan: &mut AdmissionPlan,
    mut candidates: Vec<PendingFile>,
) {
    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| fairness_bucket(right).cmp(&fairness_bucket(left)))
            .then_with(|| right.defer_count.cmp(&left.defer_count))
            .then_with(|| {
                left.last_deferred_at_ms
                    .unwrap_or(i64::MAX)
                    .cmp(&right.last_deferred_at_ms.unwrap_or(i64::MAX))
            })
            .then_with(|| left.size_bytes.cmp(&right.size_bytes))
            .then_with(|| left.path.cmp(&right.path))
    });

    for candidate in candidates {
        if plan.selected.len() >= max_count {
            plan.deferred.push(candidate);
            continue;
        }

        let estimated_cost = queue.estimate_cost_for_path_in_mode(
            &candidate.path,
            candidate.size_bytes,
            ProcessingMode::Full,
        );
        let degraded_cost = queue.estimate_cost_for_path_in_mode(
            &candidate.path,
            candidate.size_bytes,
            ProcessingMode::StructureOnly,
        );

        if !queue.can_fit_alone_in_mode(&candidate.path, candidate.size_bytes, ProcessingMode::Full)
        {
            if queue.can_fit_alone_in_mode(
                &candidate.path,
                candidate.size_bytes,
                ProcessingMode::StructureOnly,
            ) && candidate.defer_count >= OVERSIZED_PROBATION_DEFER_THRESHOLD
                && degraded_cost <= *remaining_budget
            {
                *remaining_budget = remaining_budget.saturating_sub(degraded_cost);
                plan.degraded.push(candidate.path.clone());
                plan.selected.push(AdmissionSelection {
                    file: candidate,
                    mode: ProcessingMode::StructureOnly,
                });
            } else if candidate.defer_count < OVERSIZED_PROBATION_DEFER_THRESHOLD {
                plan.deferred.push(candidate);
            } else if queue.can_fit_alone_in_mode(
                &candidate.path,
                candidate.size_bytes,
                ProcessingMode::StructureOnly,
            ) {
                plan.deferred.push(candidate);
            } else {
                plan.oversized.push(candidate);
            }
        } else if estimated_cost <= *remaining_budget {
            *remaining_budget = remaining_budget.saturating_sub(estimated_cost);
            plan.selected.push(AdmissionSelection {
                file: candidate,
                mode: ProcessingMode::Full,
            });
        } else {
            plan.deferred.push(candidate);
        }
    }
}

fn enqueue_claimed_files(
    store: &GraphStore,
    queue: &QueueStore,
    files: Vec<PendingFile>,
    selected_modes: &std::collections::HashMap<String, ProcessingMode>,
) {
    for file in files {
        let mut mode = selected_modes
            .get(&file.path)
            .copied()
            .unwrap_or(ProcessingMode::Full);

        if !queue.can_fit_alone_in_mode(&file.path, file.size_bytes, mode) {
            if mode == ProcessingMode::Full
                && queue.can_fit_alone_in_mode(
                    &file.path,
                    file.size_bytes,
                    ProcessingMode::StructureOnly,
                )
            {
                mode = ProcessingMode::StructureOnly;
            } else {
                record_oversized_refusal();
                warn!(
                    "Autonomous Ingestor marked {} as oversized for current budget (priority={}, size={}).",
                    file.path,
                    file.priority,
                    file.size_bytes
                );
                if let Err(err) = store.mark_file_oversized_for_current_budget(&file.path) {
                    error!(
                        "Autonomous Ingestor failed to mark oversized claimed file {}: {}",
                        file.path, err
                    );
                }
                continue;
            }
        }

        let is_hot = file.priority >= HOT_PRIORITY;
        if matches!(mode, ProcessingMode::StructureOnly) {
            record_structure_only_admission();
        }

            if let Err(err) = queue.push_with_mode(&file.path, 0, &file.trace_id, 0, 0, is_hot, mode) {
            record_oversized_refusal();
            warn!(
                "Autonomous Ingestor failed to enqueue {} (priority={}, mode={:?}): {}. Requeueing claim.",
                file.path,
                file.priority,
                mode,
                err
            );
            if let Err(requeue_err) =
                store.requeue_claimed_file_with_reason(&file.path, "requeued_after_queue_push_failure")
            {
                error!(
                    "Autonomous Ingestor failed to requeue claimed file {} after queue pressure: {}",
                    file.path,
                    requeue_err
                );
            }
        }
    }
}

pub(crate) fn spawn_initial_scan(
    store: Arc<GraphStore>,
    projects_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || {
        info!("🚀 Auto-Ignition: Beginning initial workspace mapping...");
        let scanner = axon_core::scanner::Scanner::new(&projects_root);
        if let Ok(preferred_project_root) = std::env::var("AXON_PROJECT_ROOT") {
            let preferred_path = PathBuf::from(preferred_project_root);
            if preferred_path.starts_with(&projects_root) && preferred_path.is_dir() {
                scanner.scan_subtree_with_guard_and_ingress(
                    store.clone(),
                    &preferred_path,
                    Some(&file_ingress_guard),
                    Some(&ingress_buffer),
                );
            }
        }
        scanner.scan_with_guard_and_ingress(
            store,
            Some(&file_ingress_guard),
            Some(&ingress_buffer),
        );
        info!("✅ Auto-Ignition: Initial mapping sequence complete.");
    });
}

pub(crate) fn spawn_hot_delta_watcher(
    store: Arc<GraphStore>,
    projects_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || {
        let watch_root = PathBuf::from(projects_root);
        let preferred_project_root = std::env::var("AXON_PROJECT_ROOT").ok().map(PathBuf::from);
        let watcher_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            info!(
                "Rust FS watcher preparing targets under {}",
                watch_root.display()
            );

            let callback_root = watch_root.clone();
            let callback_store = store.clone();
            let callback_guard = file_ingress_guard.clone();
            let callback_ingress = ingress_buffer.clone();
            let callback_active_project_root = preferred_project_root.clone();
            let rescan_guard = Arc::new(AtomicBool::new(false));
            let callback_rescan_guard = rescan_guard.clone();
            let cold_arm_completed_at = Arc::new(Mutex::new(None));
            let callback_cold_arm_completed_at = cold_arm_completed_at.clone();
            let watcher_started_at = Instant::now();

            let mut debouncer = match new_debouncer(
                Duration::from_millis(750),
                None,
                move |result: DebounceEventResult| {
                    handle_watcher_events(
                        callback_store.clone(),
                        callback_root.clone(),
                        callback_guard.clone(),
                        callback_ingress.clone(),
                        callback_active_project_root.clone(),
                        callback_rescan_guard.clone(),
                        callback_cold_arm_completed_at.clone(),
                        watcher_started_at,
                        result,
                    );
                },
            ) {
                Ok(debouncer) => debouncer,
                Err(err) => {
                    error!("Rust FS watcher initialization failed: {}", err);
                    return;
                }
            };

            let targets = watch_targets(&watch_root, preferred_project_root.as_deref());
            let mut hot_targets = active_project_hot_targets(preferred_project_root.as_deref());
            let (_, cold_targets) = split_watch_targets(targets, preferred_project_root.as_deref());
            hot_targets.insert(
                0,
                WatchTarget {
                    path: watch_root.clone(),
                    recursive: false,
                },
            );

            let mut armed = 0usize;
            let hot_started_at = Instant::now();
            for target in hot_targets {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match debouncer.watch(&target.path, mode) {
                    Ok(_) => {
                        armed += 1;
                        info!(
                            "Rust FS watcher armed hot target {} ({}) after {} ms",
                            target.path.display(),
                            if target.recursive {
                                "recursive"
                            } else {
                                "non-recursive"
                            },
                            hot_started_at.elapsed().as_millis()
                        );
                    }
                    Err(err) => {
                        warn!(
                            "Rust FS watcher skipped target {}: {}",
                            target.path.display(),
                            err
                        );
                    }
                }
            }

            if armed > 0 {
                info!(
                    "Rust FS watcher armed hot set on {} target(s) under {}",
                    armed,
                    watch_root.display()
                );
            }

            for target in cold_targets {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match debouncer.watch(&target.path, mode) {
                    Ok(_) => {
                        armed += 1;
                        debug!(
                            "Rust FS watcher armed target {} ({})",
                            target.path.display(),
                            if target.recursive {
                                "recursive"
                            } else {
                                "non-recursive"
                            }
                        );
                    }
                    Err(err) => {
                        warn!(
                            "Rust FS watcher skipped target {}: {}",
                            target.path.display(),
                            err
                        );
                    }
                }
            }

            if armed == 0 {
                error!(
                    "Rust FS watcher failed to arm any target under {}",
                    watch_root.display()
                );
                return;
            }

            info!(
                "Rust FS watcher armed on {} target(s) under {}",
                armed,
                watch_root.display()
            );
            if let Ok(mut armed_at) = cold_arm_completed_at.lock() {
                *armed_at = Some(Instant::now());
            }

            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }));

        if let Err(payload) = watcher_result {
            let reason = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            error!("Rust FS watcher thread panicked: {}", reason);
        }
    });
}

pub(crate) fn spawn_ingress_promoter(
    store: Arc<GraphStore>,
    projects_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || {
        let mut last_flush = Instant::now();

        loop {
            let metrics = ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .metrics_snapshot();

            if !metrics.enabled {
                std::thread::sleep(Duration::from_millis(INGRESS_PROMOTER_POLL_INTERVAL_MS));
                continue;
            }

            if !should_flush_ingress_buffer(&metrics, last_flush.elapsed()) {
                std::thread::sleep(Duration::from_millis(INGRESS_PROMOTER_POLL_INTERVAL_MS));
                continue;
            }

            match flush_ingress_buffer_once(
                store.clone(),
                &projects_root,
                &file_ingress_guard,
                &ingress_buffer,
            ) {
                Ok(promoted) if promoted > 0 => {
                    last_flush = Instant::now();
                }
                Ok(_) => {
                    std::thread::sleep(Duration::from_millis(INGRESS_PROMOTER_POLL_INTERVAL_MS));
                }
                Err(err) => {
                    warn!("Ingress promoter flush failed: {}", err);
                    std::thread::sleep(Duration::from_millis(INGRESS_BULK_FLUSH_WINDOW_MS));
                }
            }
        }
    });
}

fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
}

fn current_rss_bytes() -> Option<u64> {
    let page_size = 4096;
    let content = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = parse_rss_from_statm(&content)?;
    Some(rss_pages * page_size)
}

fn memory_reclaimer_enabled() -> bool {
    std::env::var("AXON_ENABLE_MEMORY_RECLAIMER")
        .ok()
        .map(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"))
        .unwrap_or(true)
}

fn memory_reclaimer_min_anon_bytes() -> u64 {
    std::env::var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1024 * 1024))
        .unwrap_or(4 * 1024 * 1024 * 1024)
}

fn memory_limit_bytes() -> u64 {
    let gb = std::env::var("AXON_MEMORY_LIMIT_GB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 2)
        .unwrap_or(14);
    gb * 1024 * 1024 * 1024
}

fn watch_targets(root: &Path, preferred_root: Option<&Path>) -> Vec<WatchTarget> {
    let mut targets = vec![WatchTarget {
        path: root.to_path_buf(),
        recursive: false,
    }];

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return targets,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if std::fs::read_dir(&path).is_err() {
            continue;
        }
        targets.push(WatchTarget {
            path,
            recursive: true,
        });
    }

    if let Some(preferred_root) = preferred_root {
        if let Some(index) = targets
            .iter()
            .position(|target| target.recursive && target.path == preferred_root)
        {
            let preferred = targets.remove(index);
            targets.insert(1, preferred);
        }
    }

    targets
}

fn active_project_hot_targets(preferred_root: Option<&Path>) -> Vec<WatchTarget> {
    let Some(preferred_root) = preferred_root else {
        return Vec::new();
    };

    let mut targets = vec![WatchTarget {
        path: preferred_root.to_path_buf(),
        recursive: false,
    }];

    let entries = match std::fs::read_dir(preferred_root) {
        Ok(entries) => entries,
        Err(_) => return targets,
    };

    let mut child_targets = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|segment| segment.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if std::fs::read_dir(&path).is_err() {
            continue;
        }
        child_targets.push(WatchTarget {
            path,
            recursive: true,
        });
    }

    child_targets.sort_by_key(|target| project_hot_target_rank(&target.path));
    targets.extend(child_targets);
    targets
}

fn project_hot_target_rank(path: &Path) -> (u8, String) {
    let name = path
        .file_name()
        .and_then(|segment| segment.to_str())
        .unwrap_or_default()
        .to_string();

    let rank = match name.as_str() {
        "src" => 0,
        "lib" => 1,
        "test" | "tests" => 2,
        "docs" => 3,
        "scripts" => 4,
        _ => 10,
    };

    (rank, name)
}

fn split_watch_targets(
    targets: Vec<WatchTarget>,
    preferred_root: Option<&Path>,
) -> (Vec<WatchTarget>, Vec<WatchTarget>) {
    let mut hot_targets = Vec::new();
    let mut cold_targets = Vec::new();

    for target in targets {
        if !target.recursive {
            hot_targets.push(target);
            continue;
        }

        if preferred_root.is_some_and(|preferred| target.path == preferred) {
            continue;
        } else {
            cold_targets.push(target);
        }
    }

    (hot_targets, cold_targets)
}

#[derive(Debug, Clone, Copy)]
struct ClaimPolicy {
    mode: ClaimMode,
    claim_count: usize,
    sleep: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimMode {
    Fast,
    Slow,
    Guarded,
    Paused,
}

impl ClaimMode {
    fn label(self) -> &'static str {
        match self {
            ClaimMode::Fast => "fast",
            ClaimMode::Slow => "slow",
            ClaimMode::Guarded => "guarded",
            ClaimMode::Paused => "paused",
        }
    }
}

fn claim_policy(
    queue_len: usize,
    budget_exhaustion_ratio: f64,
    rss_bytes: Option<u64>,
    memory_limit: u64,
    service_pressure: ServicePressure,
) -> ClaimPolicy {
    let rss_ratio = rss_bytes
        .map(|rss| rss as f64 / memory_limit.max(1) as f64)
        .unwrap_or(0.0);
    let queue_pressure = (queue_len as f64 / 6_000.0).clamp(0.0, 1.0);
    let service_pressure_score = match service_pressure {
        ServicePressure::Healthy => 0.0,
        ServicePressure::Recovering => 0.35,
        ServicePressure::Degraded => 0.70,
        ServicePressure::Critical => 1.0,
    };
    let dynamic_pressure = ((queue_pressure * 0.35)
        + (budget_exhaustion_ratio.clamp(0.0, 1.0) * 0.25)
        + (rss_ratio.clamp(0.0, 1.0) * 0.30)
        + (service_pressure_score * 0.40))
        .clamp(0.0, 1.0);

    if service_pressure == ServicePressure::Critical
        || rss_ratio >= 0.92
        || budget_exhaustion_ratio >= 0.98
        || queue_len >= 6_000
    {
        return ClaimPolicy {
            mode: ClaimMode::Paused,
            claim_count: 0,
            sleep: std::time::Duration::from_millis(1_000),
        };
    }

    if service_pressure == ServicePressure::Degraded
        || rss_ratio >= 0.82
        || budget_exhaustion_ratio >= 0.88
        || queue_len >= 3_000
    {
        return ClaimPolicy {
            mode: ClaimMode::Guarded,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Guarded),
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Guarded),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow),
        };
    }

    if budget_exhaustion_ratio >= 0.72 || queue_len >= 1_500 {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow),
        };
    }

    ClaimPolicy {
        mode: ClaimMode::Fast,
        claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Fast),
        sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Fast),
    }
}

fn dynamic_claim_count(pressure: f64, mode: ClaimMode) -> usize {
    let base =
        ((2_000.0 * (1.0 - pressure.clamp(0.0, 1.0)).powi(2)).round() as usize).clamp(25, 2_000);

    match mode {
        ClaimMode::Fast => base,
        ClaimMode::Slow => ((base as f64) * 0.60).round() as usize,
        ClaimMode::Guarded => ((base as f64) * 0.20).round() as usize,
        ClaimMode::Paused => 0,
    }
    .clamp(25, 2_000)
}

fn service_pressure_label(service_pressure: ServicePressure) -> &'static str {
    match service_pressure {
        ServicePressure::Healthy => "healthy",
        ServicePressure::Recovering => "recovering",
        ServicePressure::Degraded => "degraded",
        ServicePressure::Critical => "critical",
    }
}

fn host_state_label(
    mode: ClaimMode,
    exhaustion_ratio: f64,
    service_pressure: ServicePressure,
) -> &'static str {
    if matches!(mode, ClaimMode::Paused)
        || matches!(service_pressure, ServicePressure::Critical)
        || exhaustion_ratio >= 1.0
    {
        "constrained"
    } else if matches!(mode, ClaimMode::Slow | ClaimMode::Guarded)
        || matches!(
            service_pressure,
            ServicePressure::Degraded | ServicePressure::Recovering
        )
        || exhaustion_ratio >= 0.75
    {
        "watch"
    } else {
        "healthy"
    }
}

fn record_oversized_refusal() {
    OVERSIZED_REFUSALS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn record_structure_only_admission() {
    DEGRADED_MODE_ENTRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn record_claim_mode_transition(mode: ClaimMode) {
    let code = claim_mode_code(mode);
    let previous = LAST_REPORTED_CLAIM_MODE.swap(code, Ordering::Relaxed);

    if previous != code && matches!(mode, ClaimMode::Guarded | ClaimMode::Paused) {
        DEGRADED_MODE_ENTRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

fn claim_mode_code(mode: ClaimMode) -> u8 {
    match mode {
        ClaimMode::Fast => 0,
        ClaimMode::Slow => 1,
        ClaimMode::Guarded => 2,
        ClaimMode::Paused => 3,
    }
}

fn fairness_bucket(candidate: &PendingFile) -> u8 {
    if candidate.defer_count >= FAIRNESS_PROMOTION_DEFER_THRESHOLD {
        1
    } else {
        0
    }
}

fn dynamic_claim_sleep(pressure: f64, mode: ClaimMode) -> std::time::Duration {
    let pressure = pressure.clamp(0.0, 1.0);
    let sleep_ms = match mode {
        ClaimMode::Fast => 100 + (pressure * 200.0).round() as u64,
        ClaimMode::Slow => 250 + (pressure * 300.0).round() as u64,
        ClaimMode::Guarded => 500 + (pressure * 400.0).round() as u64,
        ClaimMode::Paused => 1_000,
    };
    std::time::Duration::from_millis(sleep_ms)
}

fn handle_watcher_events(
    store: Arc<GraphStore>,
    watch_root: std::path::PathBuf,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
    active_project_root: Option<PathBuf>,
    rescan_guard: Arc<AtomicBool>,
    cold_arm_completed_at: Arc<Mutex<Option<Instant>>>,
    watcher_started_at: Instant,
    result: DebounceEventResult,
) {
    match result {
        Ok(events) => {
            let mut paths = Vec::new();
            let mut rescan_requested = false;

            for event in events {
                if event.need_rescan() {
                    rescan_requested = true;
                }
                paths.extend(event.paths.iter().cloned());
            }

            let cold_arm_completed_at = cold_arm_completed_at.lock().ok().and_then(|guard| *guard);

            if should_suppress_bootstrap_event_storm(
                paths.len(),
                watcher_started_at,
                cold_arm_completed_at,
            ) {
                let salvaged = bootstrap_salvage_paths(&paths, active_project_root.as_deref());
                warn!(
                    "Rust FS watcher suppressed bootstrap event storm ({} path(s)) under {}",
                    paths.len(),
                    watch_root.display()
                );
                watcher_probe::record(
                    "watcher.storm_suppressed",
                    None,
                    format!("paths={} salvaged={}", paths.len(), salvaged.len()),
                );
                if !salvaged.is_empty() {
                    match fs_watcher::enqueue_hot_deltas_with_guard(
                        &watch_root,
                        salvaged.clone(),
                        HOT_PRIORITY,
                        &file_ingress_guard,
                        &ingress_buffer,
                    ) {
                        Ok(staged) if staged > 0 => {
                            info!(
                                "Rust FS watcher buffered {} hot delta(s) from bootstrap storm.",
                                staged
                            );
                            watcher_probe::record(
                                "watcher.storm_salvaged",
                                None,
                                format!("buffered={}", staged),
                            );
                        }
                        Ok(_) => {
                            watcher_probe::record(
                                "watcher.storm_salvaged_none",
                                None,
                                format!("candidates={}", salvaged.len()),
                            );
                        }
                        Err(err) => {
                            warn!("Rust FS watcher failed to salvage hot delta(s): {}", err);
                            watcher_probe::record(
                                "watcher.storm_salvage_failed",
                                None,
                                err.to_string(),
                            );
                        }
                    }
                }
                return;
            }

            if !paths.is_empty() {
                info!(
                    "Rust FS watcher received {} path event(s) under {}",
                    paths.len(),
                    watch_root.display()
                );
                watcher_probe::record("watcher.received", None, format!("paths={}", paths.len()));
            }

            if rescan_requested {
                if !rescan_guard.swap(true, Ordering::SeqCst) {
                    watcher_probe::record(
                        "watcher.rescan_requested",
                        None,
                        format!("paths={}", paths.len()),
                    );
                    let rescan_store = store.clone();
                    let rescan_root = watch_root.clone();
                    let rescan_guard_state = file_ingress_guard.clone();
                    let rescan_ingress = ingress_buffer.clone();
                    let rescan_guard_release = rescan_guard.clone();
                    std::thread::spawn(move || {
                        let _guard_reset = RescanGuardReset::new(rescan_guard_release);
                        warn!(
                            "Rust FS watcher requested a safety rescan on {}",
                            rescan_root.display()
                        );
                        watcher_probe::record(
                            "watcher.rescan_started",
                            Some(&rescan_root),
                            "reason=notify_rescan",
                        );
                        Scanner::new(rescan_root.to_string_lossy().as_ref())
                            .scan_with_guard_and_ingress(
                                rescan_store,
                                Some(&rescan_guard_state),
                                Some(&rescan_ingress),
                            );
                        watcher_probe::record(
                            "watcher.rescan_completed",
                            Some(&rescan_root),
                            "status=ok",
                        );
                    });
                } else {
                    watcher_probe::record("watcher.rescan_skipped", None, "reason=guard_active");
                }
            }

            match fs_watcher::enqueue_hot_deltas_with_guard(
                &watch_root,
                paths,
                HOT_PRIORITY,
                &file_ingress_guard,
                &ingress_buffer,
            ) {
                Ok(staged) if staged > 0 => {
                    info!("Rust FS watcher buffered {} hot delta(s).", staged);
                    watcher_probe::record(
                        "watcher.buffered_batch",
                        None,
                        format!("buffered={}", staged),
                    );
                }
                Ok(_) => {
                    info!("Rust FS watcher received event(s) but buffered no hot delta.");
                    watcher_probe::record(
                        "watcher.buffered_none",
                        None,
                        "reason=no_eligible_delta",
                    );
                }
                Err(err) => {
                    watcher_probe::record("watcher.buffering_failed", None, err.to_string());
                    warn!("Rust FS watcher failed to buffer hot delta(s): {}", err)
                }
            }
        }
        Err(errors) => {
            for err in errors {
                watcher_probe::record("watcher.error", None, err.to_string());
                warn!("Rust FS watcher event error: {}", err);
            }
        }
    }
}

struct RescanGuardReset {
    guard: Arc<AtomicBool>,
}

impl RescanGuardReset {
    fn new(guard: Arc<AtomicBool>) -> Self {
        Self { guard }
    }
}

impl Drop for RescanGuardReset {
    fn drop(&mut self) {
        self.guard.store(false, Ordering::SeqCst);
    }
}

fn should_suppress_bootstrap_event_storm(
    path_count: usize,
    watcher_started_at: Instant,
    cold_arm_completed_at: Option<Instant>,
) -> bool {
    if watcher_started_at.elapsed() <= Duration::from_secs(120) && path_count >= 5_000 {
        return true;
    }

    cold_arm_completed_at
        .map(|armed_at| armed_at.elapsed() <= Duration::from_secs(30) && path_count >= 1_000)
        .unwrap_or(false)
}

fn bootstrap_salvage_paths(paths: &[PathBuf], active_project_root: Option<&Path>) -> Vec<PathBuf> {
    let Some(active_project_root) = active_project_root else {
        return Vec::new();
    };

    paths
        .iter()
        .filter_map(|path| {
            let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let metadata = std::fs::metadata(&absolute).ok()?;
            if metadata.is_file() && absolute.starts_with(active_project_root) {
                Some(absolute)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        active_project_hot_targets, bootstrap_salvage_paths, claim_policy, enqueue_claimed_files,
        flush_ingress_buffer_once, handle_watcher_events, memory_limit_bytes,
        memory_reclaimer_enabled, memory_reclaimer_min_anon_bytes, plan_admissions,
        should_suppress_bootstrap_event_storm, split_watch_targets, watch_targets,
        RescanGuardReset, OVERSIZED_PROBATION_DEFER_THRESHOLD,
    };
    use axon_core::file_ingress_guard::FileIngressGuard;
    use axon_core::graph::{GraphStore, PendingFile};
    use axon_core::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
    use axon_core::queue::QueueStore;
    use axon_core::service_guard::ServicePressure;
    use axon_core::watcher_probe;
    use notify_debouncer_full::notify::event::{Flag, ModifyKind};
    use notify_debouncer_full::notify::Error;
    use notify_debouncer_full::notify::{Event, EventKind};
    use notify_debouncer_full::DebouncedEvent;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn test_file_ingress_guard() -> Arc<Mutex<FileIngressGuard>> {
        Arc::new(Mutex::new(FileIngressGuard::default()))
    }

    fn test_ingress_buffer() -> SharedIngressBuffer {
        Arc::new(Mutex::new(IngressBuffer::default()))
    }

    #[test]
    fn test_memory_limit_uses_default_when_env_missing() {
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
        assert_eq!(memory_limit_bytes(), 14 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_memory_limit_uses_env_when_valid() {
        unsafe {
            std::env::set_var("AXON_MEMORY_LIMIT_GB", "10");
        }
        assert_eq!(memory_limit_bytes(), 10 * 1024 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
    }

    #[test]
    fn test_memory_reclaimer_enabled_defaults_to_true() {
        unsafe {
            std::env::remove_var("AXON_ENABLE_MEMORY_RECLAIMER");
        }
        assert!(memory_reclaimer_enabled());
    }

    #[test]
    fn test_memory_reclaimer_can_be_disabled_with_env() {
        unsafe {
            std::env::set_var("AXON_ENABLE_MEMORY_RECLAIMER", "false");
        }
        assert!(!memory_reclaimer_enabled());
        unsafe {
            std::env::remove_var("AXON_ENABLE_MEMORY_RECLAIMER");
        }
    }

    #[test]
    fn test_memory_reclaimer_min_anon_bytes_uses_env_override() {
        unsafe {
            std::env::set_var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB", "2048");
        }
        assert_eq!(
            memory_reclaimer_min_anon_bytes(),
            2_048 * 1024 * 1024
        );
        unsafe {
            std::env::remove_var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB");
        }
    }

    #[test]
    fn test_claim_policy_is_fast_when_system_is_healthy() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
        assert!(policy.claim_count > 1_500);
        assert!(policy.sleep <= std::time::Duration::from_millis(200));
    }

    #[test]
    fn test_claim_policy_slows_when_queue_grows() {
        let policy = claim_policy(
            2_000,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "slow");
        assert!(policy.claim_count < 1_500);
        assert!(policy.sleep > std::time::Duration::from_millis(200));
    }

    #[test]
    fn test_claim_policy_reduces_work_progressively_before_mode_switch() {
        let lighter = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        let heavier = claim_policy(
            1_200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );

        assert_eq!(lighter.mode.label(), "fast");
        assert_eq!(heavier.mode.label(), "fast");
        assert!(
            heavier.claim_count < lighter.claim_count,
            "claim count should decrease progressively as pressure rises, even before switching modes"
        );
        assert!(
            heavier.sleep > lighter.sleep,
            "sleep should increase progressively as pressure rises"
        );
    }

    #[test]
    fn test_claim_policy_enters_guard_mode_when_queue_is_high() {
        let policy = claim_policy(
            3_500,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "guarded");
        assert!(policy.claim_count < 600);
        assert!(policy.sleep >= std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_claim_policy_pauses_claiming_when_pressure_is_critical() {
        let policy = claim_policy(
            500,
            0.10,
            Some(95 * 1024 * 1024),
            100 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_claim_policy_enters_guarded_mode_when_service_is_degraded() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Degraded,
        );
        assert_eq!(policy.mode.label(), "guarded");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 1_000);
        assert!(policy.sleep > std::time::Duration::from_millis(400));
    }

    #[test]
    fn test_claim_policy_pauses_when_live_service_is_critical() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Critical,
        );
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_claim_policy_recovers_gradually_after_service_pressure() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Recovering,
        );
        assert_eq!(policy.mode.label(), "slow");
        assert!(policy.claim_count > 500);
        assert!(policy.claim_count < 1_500);
        assert!(policy.sleep > std::time::Duration::from_millis(250));
    }

    #[test]
    fn test_claim_policy_reports_fast_mode() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
    }

    #[test]
    fn test_claim_policy_reports_guarded_mode() {
        let policy = claim_policy(
            3_500,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "guarded");
    }

    #[test]
    fn test_claim_policy_reports_paused_mode() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Critical,
        );
        assert_eq!(policy.mode.label(), "paused");
    }

    #[test]
    fn test_claim_policy_slows_when_memory_budget_is_warming_up() {
        let policy = claim_policy(
            200,
            0.75,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "slow");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 900);
    }

    #[test]
    fn test_claim_policy_guards_when_memory_budget_is_nearly_full() {
        let policy = claim_policy(
            200,
            0.90,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "guarded");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 250);
    }

    #[test]
    fn test_claim_policy_pauses_when_memory_budget_is_exhausted() {
        let policy = claim_policy(
            200,
            0.99,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "paused");
        assert_eq!(policy.claim_count, 0);
    }

    #[test]
    fn test_handle_watcher_events_stages_modified_file_as_hot_delta() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let guard = test_file_ingress_guard();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Modify(ModifyKind::Data(
                    notify_debouncer_full::notify::event::DataChange::Any,
                )),
                paths: vec![file_path.clone()],
                attrs: Default::default(),
            },
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store.clone(),
            root.to_path_buf(),
            guard.clone(),
            ingress_buffer.clone(),
            Some(project.clone()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );
        flush_ingress_buffer_once(
            store.clone(),
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"));
        assert!(row.contains("900"));
    }

    #[test]
    fn test_bootstrap_storm_still_salvages_active_project_delta() {
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let guard = test_file_ingress_guard();
        let mut events = Vec::new();
        for idx in 0..5_100 {
            let path = if idx == 0 {
                file_path.clone()
            } else {
                root.join(format!("cold-{idx}.tmp"))
            };
            events.push(DebouncedEvent::new(
                Event {
                    kind: EventKind::Modify(ModifyKind::Data(
                        notify_debouncer_full::notify::event::DataChange::Any,
                    )),
                    paths: vec![path],
                    attrs: Default::default(),
                },
                std::time::Instant::now(),
            ));
        }

        handle_watcher_events(
            store.clone(),
            root.to_path_buf(),
            guard.clone(),
            ingress_buffer.clone(),
            Some(project.clone()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(events),
        );
        flush_ingress_buffer_once(
            store.clone(),
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"));
        assert!(row.contains("900"));

        let events = watcher_probe::recent();
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.storm_suppressed")));
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.storm_salvaged")));
    }

    #[test]
    fn test_bootstrap_salvage_paths_keeps_only_active_project_candidates() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let file_path = project.join("src").join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();
        let outside = root.join("other").join("cold.tmp");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, "x").unwrap();

        let salvaged = bootstrap_salvage_paths(
            &[file_path.clone(), outside.clone()],
            Some(project.as_path()),
        );

        assert_eq!(salvaged.len(), 1);
        assert_eq!(salvaged[0], std::fs::canonicalize(file_path).unwrap());
    }

    #[test]
    fn test_bootstrap_salvage_paths_ignores_directories() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let src_dir = project.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let file_path = src_dir.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let salvaged = bootstrap_salvage_paths(
            &[src_dir.clone(), file_path.clone()],
            Some(project.as_path()),
        );

        assert_eq!(salvaged.len(), 1);
        assert_eq!(salvaged[0], std::fs::canonicalize(file_path).unwrap());
    }

    #[test]
    fn test_handle_watcher_events_records_staged_none_reason_for_ineligible_delta() {
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("ignored.png");
        std::fs::write(&file_path, "not parsable").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Modify(ModifyKind::Data(
                    notify_debouncer_full::notify::event::DataChange::Any,
                )),
                paths: vec![file_path.clone()],
                attrs: Default::default(),
            },
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let events = watcher_probe::recent();
        assert!(events.iter().any(|line| line.contains("watcher.filtered")));
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.buffered_none")));
    }

    #[test]
    fn test_handle_watcher_events_records_rescan_request() {
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Other,
                paths: vec![file_path],
                attrs: Default::default(),
            }
            .set_flag(Flag::Rescan),
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let events = watcher_probe::recent();
            let requested = events
                .iter()
                .any(|line| line.contains("watcher.rescan_requested"));
            let completed = events
                .iter()
                .any(|line| line.contains("watcher.rescan_completed"));
            if requested && completed {
                break;
            }
            if Instant::now() >= deadline {
                panic!("watcher rescan checkpoints not observed in time");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn test_handle_watcher_events_records_rescan_skipped_when_guard_active() {
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Other,
                paths: vec![file_path],
                attrs: Default::default(),
            }
            .set_flag(Flag::Rescan),
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(true)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let events = watcher_probe::recent();
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.rescan_skipped")));
    }

    #[test]
    fn test_handle_watcher_events_records_watcher_errors() {
        watcher_probe::clear();

        handle_watcher_events(
            Arc::new(GraphStore::new(":memory:").unwrap()),
            PathBuf::from("/tmp"),
            test_file_ingress_guard(),
            test_ingress_buffer(),
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Err(vec![Error::generic("boom")]),
        );

        let events = watcher_probe::recent();
        assert!(events.iter().any(|line| line.contains("watcher.error")));
        assert!(events.iter().any(|line| line.contains("boom")));
    }

    #[test]
    fn test_enqueue_claimed_files_requeues_work_when_common_lane_is_full() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("bulk_overflow.ex");
        std::fs::write(&file_path, "defmodule BulkOverflow do\nend\n").unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let claimed = store.fetch_pending_batch(10).unwrap();
        assert_eq!(claimed.len(), 1);

        let queue = QueueStore::new(3);
        for idx in 0..2 {
            let fill_bulk = temp.path().join(format!("fill-bulk-{}.ex", idx));
            std::fs::write(&fill_bulk, "defmodule FillBulk do\nend\n").unwrap();
            queue
                .push(
                    fill_bulk.to_string_lossy().as_ref(),
                    0,
                    &format!("fill-bulk-{}", idx),
                    0,
                    0,
                    false,
                )
                .unwrap();
        }

        enqueue_claimed_files(&store, &queue, claimed, &std::collections::HashMap::new());

        let row = store
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("pending"));
        assert!(row.contains("null"));
    }

    #[test]
    fn test_plan_admissions_prefers_packable_candidates_over_single_blocking_large_file() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let medium = temp.path().join("medium.txt");
        let small = temp.path().join("small.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&medium, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small, vec![b'x'; 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = vec![
            PendingFile {
                path: large.to_string_lossy().to_string(),
                trace_id: "large".to_string(),
                priority: 100,
                size_bytes: 8 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: medium.to_string_lossy().to_string(),
                trace_id: "medium".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: small.to_string_lossy().to_string(),
                trace_id: "small".to_string(),
                priority: 100,
                size_bytes: 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
        ];

        let plan = plan_admissions(&queue, candidates, 3);
        assert_eq!(
            plan.selected.iter().map(|selection| selection.file.trace_id.as_str()).collect::<Vec<_>>(),
            vec!["small", "medium"],
            "the scheduler should admit the better-fitting small+medium pair instead of blocking on the large candidate"
        );
        assert!(plan.deferred.iter().any(|file| file.trace_id == "large"));
    }

    #[test]
    fn test_plan_admissions_marks_candidate_oversized_when_it_cannot_fit_even_alone() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 2 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(plan.selected.is_empty());
        assert_eq!(plan.oversized.len(), 1);
        assert_eq!(plan.oversized[0].trace_id, "oversized");
    }

    #[test]
    fn test_plan_admissions_prefers_structure_only_degradation_before_oversized_refusal() {
        let temp = tempdir().unwrap();
        let candidate = temp.path().join("candidate.rs");
        std::fs::write(&candidate, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: candidate.to_string_lossy().to_string(),
            trace_id: "candidate".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(
            plan.oversized.is_empty(),
            "a file that fits the degraded envelope should not be marked oversized"
        );
        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.selected[0].file.trace_id, "candidate");
        assert_eq!(
            plan.selected[0].mode,
            axon_core::queue::ProcessingMode::StructureOnly
        );
    }

    #[test]
    fn test_plan_admissions_gives_probation_to_cold_oversized_candidate() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 2 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: 0,
            last_deferred_at_ms: None,
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(plan.selected.is_empty());
        assert!(plan.oversized.is_empty(), "a cold oversized candidate should first be deferred while the estimator is still conservative");
        assert_eq!(plan.deferred.len(), 1);
        assert_eq!(plan.deferred[0].trace_id, "oversized");
    }

    #[test]
    fn test_plan_admissions_uses_degraded_mode_before_final_oversized_refusal() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 4_500_000);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert_eq!(
            plan.selected.len(),
            1,
            "the candidate should still be admitted through the degraded envelope"
        );
        assert_eq!(plan.selected[0].file.trace_id, "oversized");
        assert_eq!(
            plan.degraded.len(),
            1,
            "the degraded admission should be recorded explicitly"
        );
        assert_eq!(plan.degraded[0], oversized.to_string_lossy());
        assert!(
            plan.oversized.is_empty(),
            "degraded admission must win before definitive oversized refusal"
        );
    }

    #[test]
    fn test_plan_admissions_eventually_ages_deferred_large_candidate_into_selection() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let medium = temp.path().join("medium.txt");
        let small = temp.path().join("small.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&medium, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small, vec![b'x'; 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = |large_defer_count: u32| {
            vec![
                PendingFile {
                    path: large.to_string_lossy().to_string(),
                    trace_id: "large".to_string(),
                    priority: 100,
                    size_bytes: 8 * 1024,
                    defer_count: large_defer_count,
                    last_deferred_at_ms: (large_defer_count > 0).then_some(1),
                },
                PendingFile {
                    path: medium.to_string_lossy().to_string(),
                    trace_id: "medium".to_string(),
                    priority: 100,
                    size_bytes: 2 * 1024,
                    defer_count: 0,
                    last_deferred_at_ms: None,
                },
                PendingFile {
                    path: small.to_string_lossy().to_string(),
                    trace_id: "small".to_string(),
                    priority: 100,
                    size_bytes: 1024,
                    defer_count: 0,
                    last_deferred_at_ms: None,
                },
            ]
        };

        let plan = plan_admissions(&queue, candidates(0), 2);
        assert_eq!(
            plan.selected
                .iter()
                .map(|selection| selection.file.trace_id.as_str())
                .collect::<Vec<_>>(),
            vec!["small", "medium"],
            "before aging kicks in, the scheduler should keep picking the better-fitting pair"
        );
        assert!(plan.deferred.iter().any(|file| file.trace_id == "large"));

        let aged = plan_admissions(&queue, candidates(3), 2);
        assert_eq!(
            aged.selected
                .first()
                .map(|selection| selection.file.trace_id.as_str()),
            Some("large"),
            "after repeated deferrals, the large file should gain enough fairness to pass first"
        );
    }

    #[test]
    fn test_plan_admissions_promotes_repeatedly_deferred_large_file_before_smaller_new_work() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let small_a = temp.path().join("small-a.txt");
        let small_b = temp.path().join("small-b.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&small_a, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small_b, vec![b'x'; 2 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = vec![
            PendingFile {
                path: large.to_string_lossy().to_string(),
                trace_id: "large".to_string(),
                priority: 100,
                size_bytes: 8 * 1024,
                defer_count: 3,
                last_deferred_at_ms: Some(1),
            },
            PendingFile {
                path: small_a.to_string_lossy().to_string(),
                trace_id: "small-a".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: small_b.to_string_lossy().to_string(),
                trace_id: "small-b".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
        ];

        let plan = plan_admissions(&queue, candidates, 2);
        assert!(
            plan.selected.iter().any(|selection| selection.file.trace_id == "large"),
            "a repeatedly deferred large file should eventually be promoted ahead of newer packable work"
        );
    }

    #[test]
    fn test_enqueue_claimed_files_marks_oversized_when_file_cannot_fit_alone() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("oversized.rs");
        std::fs::write(&file_path, vec![b'x'; 16 * 1024]).unwrap();

        let store = GraphStore::new(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                16 * 1024,
                1,
            )])
            .unwrap();

        let claimed = store.fetch_pending_batch(10).unwrap();
        let queue = QueueStore::with_memory_budget(10, 2 * 1024 * 1024);

        enqueue_claimed_files(&store, &queue, claimed, &std::collections::HashMap::new());

        let row = store
            .query_json(&format!(
                "SELECT status, last_error_reason, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("oversized"));
        assert!(row.contains("current budget"));
        assert!(row.contains("null"));
    }

    #[test]
    fn test_rescan_guard_reset_releases_guard_on_drop() {
        let guard = Arc::new(AtomicBool::new(true));
        {
            let _reset = RescanGuardReset::new(guard.clone());
            assert!(guard.load(Ordering::SeqCst));
        }
        assert!(!guard.load(Ordering::SeqCst));
    }

    #[test]
    fn test_watch_targets_split_root_and_accessible_projects() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("proj_a")).unwrap();
        std::fs::create_dir_all(root.join("proj_b")).unwrap();
        std::fs::write(root.join("README.md"), "# root").unwrap();

        let targets = watch_targets(root, None);
        let rendered: Vec<(String, bool)> = targets
            .into_iter()
            .map(|target| (target.path.to_string_lossy().to_string(), target.recursive))
            .collect();

        assert!(
            rendered
                .iter()
                .any(
                    |(path, recursive): &(String, bool)| path == &root.to_string_lossy()
                        && !*recursive
                ),
            "La racine doit etre surveillee en non-recursif"
        );
        assert!(
            rendered
                .iter()
                .any(|(path, recursive): &(String, bool)| path.ends_with("proj_a") && *recursive),
            "Chaque projet accessible doit etre surveille recursivement"
        );
        assert!(
            rendered
                .iter()
                .any(|(path, recursive): &(String, bool)| path.ends_with("proj_b") && *recursive),
            "Chaque projet accessible doit etre surveille recursivement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_watch_targets_skip_unreadable_projects() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let root = temp.path();
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let targets = watch_targets(root, None);
        let rendered: Vec<String> = targets
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();

        assert!(
            !rendered
                .iter()
                .any(|path: &String| path.ends_with("locked")),
            "Un sous-arbre illisible ne doit pas bloquer l'armement global du watcher"
        );

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_watch_targets_prioritize_active_project() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let proj_a = root.join("proj_a");
        let proj_b = root.join("proj_b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();

        let targets = watch_targets(root, Some(proj_b.as_path()));
        let rendered: Vec<String> = targets
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            rendered[0],
            root.to_string_lossy(),
            "La racine doit rester observee en premier"
        );
        assert_eq!(
            rendered[1],
            proj_b.to_string_lossy(),
            "Le projet actif doit etre arme avant les autres"
        );
    }

    #[test]
    fn test_split_watch_targets_keeps_root_and_active_project_hot() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let proj_a = root.join("proj_a");
        let proj_b = root.join("proj_b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();

        let targets = watch_targets(root, Some(proj_b.as_path()));
        let (hot, cold) = split_watch_targets(targets, Some(proj_b.as_path()));

        let hot_paths: Vec<String> = hot
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();
        let cold_paths: Vec<String> = cold
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            hot_paths.len(),
            1,
            "Le split universel ne garde que la racine chaude; le projet actif est detaille a part"
        );
        assert_eq!(hot_paths[0], root.to_string_lossy());
        assert!(cold_paths
            .iter()
            .any(|path| path == &proj_a.to_string_lossy()));
        assert!(!cold_paths
            .iter()
            .any(|path| path == &proj_b.to_string_lossy()));
    }

    #[test]
    fn test_split_watch_targets_without_active_project_keeps_only_root_hot() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let proj_a = root.join("proj_a");
        std::fs::create_dir_all(&proj_a).unwrap();

        let targets = watch_targets(root, None);
        let (hot, cold) = split_watch_targets(targets, None);

        assert_eq!(
            hot.len(),
            1,
            "Sans projet actif, seul le watcher de racine doit etre chaud"
        );
        assert_eq!(hot[0].path, root);
        assert!(cold.iter().any(|target| target.path == proj_a));
    }

    #[test]
    fn test_active_project_hot_targets_expand_visible_child_subtrees() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let targets = active_project_hot_targets(Some(root));
        let rendered: Vec<(String, bool)> = targets
            .into_iter()
            .map(|target| (target.path.to_string_lossy().to_string(), target.recursive))
            .collect();

        assert_eq!(rendered[0].0, root.to_string_lossy());
        assert!(!rendered[0].1);
        assert!(rendered
            .iter()
            .any(|(path, recursive)| path.ends_with("/src") && *recursive));
        assert!(rendered
            .iter()
            .any(|(path, recursive)| path.ends_with("/docs") && *recursive));
        assert!(!rendered.iter().any(|(path, _)| path.ends_with("/.git")));
    }

    #[test]
    fn test_bootstrap_event_storm_is_suppressed_early() {
        let started = Instant::now();
        assert!(should_suppress_bootstrap_event_storm(6_000, started, None));
    }

    #[test]
    fn test_bootstrap_event_storm_is_not_suppressed_late_or_small() {
        let started = Instant::now() - Duration::from_secs(180);
        assert!(!should_suppress_bootstrap_event_storm(6_000, started, None));
        assert!(!should_suppress_bootstrap_event_storm(
            100,
            Instant::now(),
            None
        ));
    }

    #[test]
    fn test_bootstrap_event_storm_is_suppressed_right_after_cold_arm() {
        let started = Instant::now() - Duration::from_secs(180);
        let cold_arm_completed_at = Some(Instant::now());
        assert!(should_suppress_bootstrap_event_storm(
            2_000,
            started,
            cold_arm_completed_at,
        ));
    }

    #[test]
    fn test_bootstrap_event_storm_is_not_suppressed_long_after_cold_arm() {
        let started = Instant::now() - Duration::from_secs(180);
        let cold_arm_completed_at = Some(Instant::now() - Duration::from_secs(45));
        assert!(!should_suppress_bootstrap_event_storm(
            2_000,
            started,
            cold_arm_completed_at,
        ));
    }
}
