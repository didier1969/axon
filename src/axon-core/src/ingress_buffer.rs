use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::service_guard;

pub const AXON_ENABLE_INGRESS_BUFFER: &str = "AXON_ENABLE_INGRESS_BUFFER";
pub type SharedIngressBuffer = Arc<Mutex<IngressBuffer>>;

static INGRESS_BUFFERED_ENTRIES: AtomicUsize = AtomicUsize::new(0);
static INGRESS_HOT_ENTRIES: AtomicUsize = AtomicUsize::new(0);
static INGRESS_SCAN_ENTRIES: AtomicUsize = AtomicUsize::new(0);
static INGRESS_SUBTREE_HINTS: AtomicUsize = AtomicUsize::new(0);
static INGRESS_SUBTREE_HINT_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_SUBTREE_HINT_BLOCKED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_SUBTREE_HINT_PRODUCTIVE_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_SUBTREE_HINT_UNPRODUCTIVE_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_SUBTREE_HINT_DROPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_COLLAPSED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_FLUSH_COUNT: AtomicU64 = AtomicU64::new(0);
static INGRESS_LAST_FLUSH_DURATION_MS: AtomicU64 = AtomicU64::new(0);
static INGRESS_LAST_PROMOTED_COUNT: AtomicU64 = AtomicU64::new(0);
static INGRESS_PROMOTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_LAST_DURABLY_PERSISTED_COUNT: AtomicU64 = AtomicU64::new(0);
static INGRESS_DURABLY_PERSISTED_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_LAST_EXCLUDED_FROM_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
static INGRESS_EXCLUDED_FROM_PENDING_TOTAL: AtomicU64 = AtomicU64::new(0);
// REQ-AXO-901678 — drain saturation telemetry. Populated by
// `record_drain_tick` after the runtime's drain loop forwards a batch
// into pipeline A's `input_tx`. `dropped_full_total` is monotonically
// cumulative; `last_batch_*` reflects the most recent tick; `batch_size`
// reflects the effective `AXON_INGRESS_DRAIN_BATCH` cap the loop ran
// with. `heartbeat_tick` is the rolling drain-loop counter (capacity-
// independent liveness probe).
static INGRESS_DRAIN_BATCH_SIZE: AtomicUsize = AtomicUsize::new(0);
static INGRESS_DRAIN_HEARTBEAT_TICK: AtomicU64 = AtomicU64::new(0);
static INGRESS_DRAIN_LAST_BATCH_SENT: AtomicU64 = AtomicU64::new(0);
static INGRESS_DRAIN_LAST_BATCH_DROPPED_FULL: AtomicU64 = AtomicU64::new(0);
static INGRESS_DRAIN_DROPPED_FULL_TOTAL: AtomicU64 = AtomicU64::new(0);
// REQ-AXO-901677 — periodic_sweep_worker telemetry. The worker re-walks
// the watch root on a coarse interval (default 4 h, env-tunable) to
// reconcile against missed inotify events (queue overflow, mount
// changes, silent init failures). Published from
// `pipeline_v2_runtime::spawn_periodic_sweep_worker` once per tick.
//
// Surface : `axon_embedding_status.periodic_sweep` JSON block +
// `axon_diagnose_indexing` markdown section.
static PERIODIC_SWEEP_LAST_RUN_AT_MS: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_LAST_DURATION_MS: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_LAST_FILES_COMPARED: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_LAST_DELTAS_FOUND: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_RUNS_TOTAL: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_DELTAS_TOTAL: AtomicU64 = AtomicU64::new(0);
static PERIODIC_SWEEP_SKIPPED_HIGH_CPU_TOTAL: AtomicU64 = AtomicU64::new(0);
static INGRESS_ACTIVITY_SIGNAL: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IngressSource {
    Watcher,
    Scan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IngressCause {
    Discovered,
    Modified,
    Deleted,
    SubtreeHint,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngressFileEvent {
    pub path: String,
    pub project_code: String,
    pub size: i64,
    pub mtime: i64,
    pub priority: i64,
    pub source: IngressSource,
    pub cause: IngressCause,
}

impl IngressFileEvent {
    pub fn new(
        path: impl Into<String>,
        project_code: impl Into<String>,
        size: i64,
        mtime: i64,
        priority: i64,
        source: IngressSource,
        cause: IngressCause,
    ) -> Self {
        Self {
            path: path.into(),
            project_code: project_code.into(),
            size,
            mtime,
            priority,
            source,
            cause,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngressSubtreeHint {
    pub path: String,
    pub priority: i64,
    pub source: IngressSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BufferedSubtreeHint {
    hint: IngressSubtreeHint,
    first_seen_ms: u64,
    last_seen_ms: u64,
    execution_count: u64,
    cooldown_ms: u64,
    cooldown_until_ms: u64,
    in_flight: bool,
}

/// REQ-AXO-345 — wrap the buffered variants with a monotonic insertion
/// `seq` so `compare_buffered` can break priority ties FIFO instead of
/// path-ASC. Without seq, late-alphabet projects (`nanobot-loop`,
/// `nexus`, `triolingo`, `zeroclaw`, …) sit forever at the tail of the
/// sort queue while Scanner refills the head with early-alphabet entries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BufferedIngress {
    File { event: IngressFileEvent, seq: u64 },
    Tombstone {
        path: String,
        source: IngressSource,
        seq: u64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngressDrainBatch {
    pub files: Vec<IngressFileEvent>,
    pub tombstones: Vec<String>,
    pub subtree_hints: Vec<IngressSubtreeHint>,
    pub collapsed_events: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngressPromotionStats {
    pub promoted_files: usize,
    pub promoted_tombstones: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IngressMetricsSnapshot {
    pub enabled: bool,
    pub buffered_entries: usize,
    pub subtree_hints: usize,
    pub subtree_hint_in_flight: usize,
    pub subtree_hint_accepted_total: u64,
    pub subtree_hint_blocked_total: u64,
    pub subtree_hint_suppressed_total: u64,
    pub subtree_hint_productive_total: u64,
    pub subtree_hint_unproductive_total: u64,
    pub subtree_hint_dropped_total: u64,
    pub hot_entries: usize,
    pub scan_entries: usize,
    pub collapsed_total: u64,
    pub flush_count: u64,
    pub last_flush_duration_ms: u64,
    pub last_promoted_count: u64,
    pub promoted_total: u64,
    pub last_durably_persisted_count: u64,
    pub durably_persisted_total: u64,
    pub last_excluded_from_pending_count: u64,
    pub excluded_from_pending_total: u64,
    // REQ-AXO-901678 — drain saturation telemetry. Surfaces the
    // pipeline_v2 runtime drain loop's per-tick throughput so the
    // operator can detect A1 back-pressure without trawling logs.
    pub drain_batch_size: usize,
    pub drain_heartbeat_tick: u64,
    pub drain_last_batch_sent: u64,
    pub drain_last_batch_dropped_full: u64,
    pub drain_dropped_full_total: u64,
}

#[derive(Debug)]
pub struct IngressBuffer {
    enabled: bool,
    by_path: HashMap<String, BufferedIngress>,
    subtree_hints: HashMap<String, BufferedSubtreeHint>,
    collapsed_events: u64,
    /// REQ-AXO-345 — monotonic insertion counter stamped on every new
    /// `record_file` / `record_tombstone`. Used as FIFO tie-breaker in
    /// `compare_buffered` so late-alphabet projects do not starve.
    next_seq: u64,
}

impl IngressBuffer {
    pub fn new() -> Self {
        Self {
            enabled: ingress_buffer_enabled(),
            by_path: HashMap::new(),
            subtree_hints: HashMap::new(),
            collapsed_events: 0,
            next_seq: 0,
        }
    }

    fn allocate_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn record_file(&mut self, event: IngressFileEvent) {
        let key = event.path.clone();
        match self.by_path.get_mut(&key) {
            Some(BufferedIngress::File {
                event: existing, ..
            }) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                merge_file_event(existing, event);
            }
            Some(BufferedIngress::Tombstone { .. }) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            None => {
                let seq = self.allocate_seq();
                self.by_path
                    .insert(key, BufferedIngress::File { event, seq });
            }
        }
        self.sync_metrics();
        notify_ingress_activity();
    }

    pub fn record_tombstone(&mut self, path: impl Into<String>, source: IngressSource) {
        let path = path.into();
        let seq = self.allocate_seq();
        if self
            .by_path
            .insert(
                path.clone(),
                BufferedIngress::Tombstone {
                    path: path.clone(),
                    source,
                    seq,
                },
            )
            .is_some()
        {
            self.collapsed_events += 1;
            INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        self.sync_metrics();
        notify_ingress_activity();
    }

    pub fn record_subtree_hint(
        &mut self,
        path: impl Into<String>,
        priority: i64,
        source: IngressSource,
    ) {
        self.record_subtree_hint_with_cooldown(path, priority, source, subtree_hint_cooldown_ms());
    }

    pub fn record_subtree_hint_with_cooldown(
        &mut self,
        path: impl Into<String>,
        priority: i64,
        source: IngressSource,
        cooldown_ms: u64,
    ) {
        let now = current_time_ms();
        self.prune_idle_subtree_hints(now);
        let path = path.into();
        let cooldown_ms = cooldown_ms.max(1);
        match self.subtree_hints.get_mut(&path) {
            Some(existing) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                existing.last_seen_ms = now;
                if existing.cooldown_until_ms > 0 && now >= existing.cooldown_until_ms {
                    existing.cooldown_until_ms = 0;
                    existing.execution_count = 0;
                }
                existing.hint.priority = existing.hint.priority.max(priority);
                if priority >= existing.hint.priority {
                    existing.hint.source = source;
                }
                existing.cooldown_ms = existing.cooldown_ms.max(cooldown_ms);
                if existing.in_flight || existing.cooldown_until_ms > now {
                    record_blocked_subtree_hint();
                    INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                    self.sync_metrics();
                    return;
                }
                if existing.execution_count >= subtree_hint_retry_budget() {
                    record_blocked_subtree_hint();
                    INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                    self.sync_metrics();
                    return;
                }
                INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            None => {
                if source == IngressSource::Watcher
                    && self.subtree_hints.len() >= watcher_subtree_hint_budget()
                {
                    INGRESS_SUBTREE_HINT_DROPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
                    self.sync_metrics();
                    return;
                }
                INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL.fetch_add(1, Ordering::Relaxed);
                self.subtree_hints.insert(
                    path.clone(),
                    BufferedSubtreeHint {
                        hint: IngressSubtreeHint {
                            path,
                            priority,
                            source,
                        },
                        first_seen_ms: now,
                        last_seen_ms: now,
                        execution_count: 0,
                        cooldown_ms,
                        cooldown_until_ms: 0,
                        in_flight: false,
                    },
                );
            }
        }
        self.sync_metrics();
        notify_ingress_activity();
    }

    pub fn buffered_entries(&self) -> usize {
        self.by_path.len()
    }

    pub fn subtree_hint_entries(&self) -> usize {
        self.subtree_hints.len()
    }

    pub fn metrics_snapshot(&self) -> IngressMetricsSnapshot {
        let mut hot_entries = 0usize;
        let mut scan_entries = 0usize;
        let subtree_hint_in_flight = self
            .subtree_hints
            .values()
            .filter(|state| state.in_flight)
            .count();

        for entry in self.by_path.values() {
            match entry {
                BufferedIngress::File { event, .. } => match event.source {
                    IngressSource::Watcher => hot_entries += 1,
                    IngressSource::Scan => scan_entries += 1,
                },
                BufferedIngress::Tombstone { source, .. } => match source {
                    IngressSource::Watcher => hot_entries += 1,
                    IngressSource::Scan => scan_entries += 1,
                },
            }
        }

        IngressMetricsSnapshot {
            enabled: self.enabled,
            buffered_entries: self.by_path.len(),
            subtree_hints: self.subtree_hints.len(),
            subtree_hint_in_flight,
            subtree_hint_accepted_total: INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL
                .load(Ordering::Relaxed),
            subtree_hint_blocked_total: INGRESS_SUBTREE_HINT_BLOCKED_TOTAL.load(Ordering::Relaxed),
            subtree_hint_suppressed_total: INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL
                .load(Ordering::Relaxed),
            subtree_hint_productive_total: INGRESS_SUBTREE_HINT_PRODUCTIVE_TOTAL
                .load(Ordering::Relaxed),
            subtree_hint_unproductive_total: INGRESS_SUBTREE_HINT_UNPRODUCTIVE_TOTAL
                .load(Ordering::Relaxed),
            subtree_hint_dropped_total: INGRESS_SUBTREE_HINT_DROPPED_TOTAL.load(Ordering::Relaxed),
            hot_entries,
            scan_entries,
            collapsed_total: INGRESS_COLLAPSED_TOTAL.load(Ordering::Relaxed),
            flush_count: INGRESS_FLUSH_COUNT.load(Ordering::Relaxed),
            last_flush_duration_ms: INGRESS_LAST_FLUSH_DURATION_MS.load(Ordering::Relaxed),
            last_promoted_count: INGRESS_LAST_PROMOTED_COUNT.load(Ordering::Relaxed),
            promoted_total: INGRESS_PROMOTED_TOTAL.load(Ordering::Relaxed),
            last_durably_persisted_count: INGRESS_LAST_DURABLY_PERSISTED_COUNT
                .load(Ordering::Relaxed),
            durably_persisted_total: INGRESS_DURABLY_PERSISTED_TOTAL.load(Ordering::Relaxed),
            last_excluded_from_pending_count: INGRESS_LAST_EXCLUDED_FROM_PENDING_COUNT
                .load(Ordering::Relaxed),
            excluded_from_pending_total: INGRESS_EXCLUDED_FROM_PENDING_TOTAL
                .load(Ordering::Relaxed),
            drain_batch_size: INGRESS_DRAIN_BATCH_SIZE.load(Ordering::Relaxed),
            drain_heartbeat_tick: INGRESS_DRAIN_HEARTBEAT_TICK.load(Ordering::Relaxed),
            drain_last_batch_sent: INGRESS_DRAIN_LAST_BATCH_SENT.load(Ordering::Relaxed),
            drain_last_batch_dropped_full: INGRESS_DRAIN_LAST_BATCH_DROPPED_FULL
                .load(Ordering::Relaxed),
            drain_dropped_full_total: INGRESS_DRAIN_DROPPED_FULL_TOTAL.load(Ordering::Relaxed),
        }
    }

    pub fn drain_batch(&mut self, limit: usize) -> IngressDrainBatch {
        let now = current_time_ms();
        self.prune_idle_subtree_hints(now);
        let mut selected = self
            .by_path
            .iter()
            .map(|(path, entry)| (path.clone(), entry.clone()))
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| compare_buffered(&left.1, &right.1));

        let mut files = Vec::new();
        let mut tombstones = Vec::new();

        for (path, _) in selected.into_iter().take(limit.max(1)) {
            let Some(entry) = self.by_path.remove(&path) else {
                continue;
            };
            match entry {
                BufferedIngress::File { event, .. } => files.push(event),
                BufferedIngress::Tombstone { path, .. } => tombstones.push(path),
            }
        }

        let mut subtree_hints = self
            .subtree_hints
            .iter()
            .filter(|(_, state)| {
                !state.in_flight
                    && state.cooldown_until_ms <= now
                    && state.execution_count < subtree_hint_retry_budget()
            })
            .map(|(path, state)| (path.clone(), state.hint.clone()))
            .collect::<Vec<_>>();
        subtree_hints.sort_by(|left, right| {
            right
                .1
                .priority
                .cmp(&left.1.priority)
                .then_with(|| left.1.path.cmp(&right.1.path))
        });
        subtree_hints.truncate(limit.max(1));
        for (path, _) in &subtree_hints {
            if let Some(state) = self.subtree_hints.get_mut(path) {
                state.in_flight = true;
                state.last_seen_ms = now;
            }
        }

        let batch = IngressDrainBatch {
            files,
            tombstones,
            subtree_hints: subtree_hints.into_iter().map(|(_, hint)| hint).collect(),
            collapsed_events: self.collapsed_events,
        };
        self.collapsed_events = 0;
        self.sync_metrics();
        batch
    }

    pub fn complete_subtree_hint(&mut self, path: &str) {
        let now = current_time_ms();
        if let Some(state) = self.subtree_hints.get_mut(path) {
            state.in_flight = false;
            state.last_seen_ms = now;
            state.execution_count = state.execution_count.saturating_add(1);
            state.cooldown_until_ms = now.saturating_add(state.cooldown_ms.max(1));
            if state.execution_count >= subtree_hint_retry_budget() {
                INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.prune_idle_subtree_hints(now);
        self.sync_metrics();
    }

    pub fn complete_subtree_hint_with_stats(&mut self, path: &str, promoted_count: usize) {
        let now = current_time_ms();
        let mut drop_hint = false;
        if let Some(state) = self.subtree_hints.get_mut(path) {
            state.in_flight = false;
            state.last_seen_ms = now;
            if promoted_count > 0 {
                state.execution_count = 0;
                INGRESS_SUBTREE_HINT_PRODUCTIVE_TOTAL.fetch_add(1, Ordering::Relaxed);
            } else {
                state.execution_count = state.execution_count.saturating_add(1);
                INGRESS_SUBTREE_HINT_UNPRODUCTIVE_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            state.cooldown_until_ms = now.saturating_add(state.cooldown_ms.max(1));
            if promoted_count == 0 && state.execution_count >= subtree_hint_retry_budget() {
                INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                drop_hint = true;
            }
        }
        if drop_hint && self.subtree_hints.remove(path).is_some() {
            INGRESS_SUBTREE_HINT_DROPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        self.prune_idle_subtree_hints(now);
        self.sync_metrics();
    }

    pub fn shed_subtree_hints_for_memory_pressure(&mut self) -> usize {
        let dropped = self.subtree_hints.len();
        if dropped == 0 {
            return 0;
        }
        self.subtree_hints.clear();
        INGRESS_SUBTREE_HINT_DROPPED_TOTAL.fetch_add(dropped as u64, Ordering::Relaxed);
        self.sync_metrics();
        dropped
    }

    fn prune_idle_subtree_hints(&mut self, now: u64) {
        self.subtree_hints.retain(|_, state| {
            state.in_flight || state.cooldown_until_ms == 0 || now < state.cooldown_until_ms
        });
    }

    fn sync_metrics(&self) {
        let mut hot_entries = 0usize;
        let mut scan_entries = 0usize;
        for entry in self.by_path.values() {
            match entry {
                BufferedIngress::File { event, .. } => match event.source {
                    IngressSource::Watcher => hot_entries += 1,
                    IngressSource::Scan => scan_entries += 1,
                },
                BufferedIngress::Tombstone { source, .. } => match source {
                    IngressSource::Watcher => hot_entries += 1,
                    IngressSource::Scan => scan_entries += 1,
                },
            }
        }
        INGRESS_BUFFERED_ENTRIES.store(self.by_path.len(), Ordering::Relaxed);
        INGRESS_HOT_ENTRIES.store(hot_entries, Ordering::Relaxed);
        INGRESS_SCAN_ENTRIES.store(scan_entries, Ordering::Relaxed);
        INGRESS_SUBTREE_HINTS.store(self.subtree_hints.len(), Ordering::Relaxed);
        INGRESS_SUBTREE_HINT_IN_FLIGHT.store(
            self.subtree_hints
                .values()
                .filter(|state| state.in_flight)
                .count(),
            Ordering::Relaxed,
        );
    }
}

fn ingress_activity_signal() -> &'static (Mutex<u64>, Condvar) {
    INGRESS_ACTIVITY_SIGNAL.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

pub fn notify_ingress_activity() {
    let (lock, cvar) = ingress_activity_signal();
    let mut generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    *generation = generation.saturating_add(1);
    cvar.notify_all();
    service_guard::notify_runtime_work_activity();
}

pub fn wait_for_ingress_activity_or_timeout(timeout: Duration) -> bool {
    let (lock, cvar) = ingress_activity_signal();
    let generation = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    let current = *generation;
    let result = cvar
        .wait_timeout_while(generation, timeout, |observed| *observed == current)
        .unwrap_or_else(|poison| poison.into_inner());
    let (guard, _) = result;
    *guard != current
}

fn merge_file_event(existing: &mut IngressFileEvent, incoming: IngressFileEvent) {
    let replace_metadata = incoming.mtime > existing.mtime
        || (incoming.mtime == existing.mtime && incoming.size != existing.size);
    if replace_metadata {
        existing.size = incoming.size;
        existing.mtime = incoming.mtime;
        existing.project_code = incoming.project_code;
        existing.cause = incoming.cause;
    }
    if incoming.priority >= existing.priority {
        existing.priority = incoming.priority;
        existing.source = incoming.source;
        if !replace_metadata {
            existing.cause = incoming.cause;
        }
    }
}

/// REQ-AXO-345 — sort drain candidates by `(priority DESC, seq ASC)`.
/// `seq` is the monotonic insertion counter, so within the same priority
/// the buffer behaves FIFO. The previous `path ASC` tie-breaker starved
/// late-alphabet projects (`nanobot-loop`, `nexus`, `triolingo`, …) when
/// the buffer was perpetually refilled by Scanner with early-alphabet
/// entries — see REQ-AXO-345 description.
fn compare_buffered(left: &BufferedIngress, right: &BufferedIngress) -> std::cmp::Ordering {
    buffered_priority(right)
        .cmp(&buffered_priority(left))
        .then_with(|| buffered_seq(left).cmp(&buffered_seq(right)))
}

fn buffered_priority(entry: &BufferedIngress) -> i64 {
    match entry {
        BufferedIngress::File { event, .. } => event.priority,
        BufferedIngress::Tombstone { .. } => i64::MAX,
    }
}

fn buffered_seq(entry: &BufferedIngress) -> u64 {
    match entry {
        BufferedIngress::File { seq, .. } => *seq,
        BufferedIngress::Tombstone { seq, .. } => *seq,
    }
}

#[allow(dead_code)]
fn buffered_path(entry: &BufferedIngress) -> &str {
    match entry {
        BufferedIngress::File { event, .. } => &event.path,
        BufferedIngress::Tombstone { path, .. } => path,
    }
}

pub fn ingress_buffer_enabled() -> bool {
    match std::env::var(AXON_ENABLE_INGRESS_BUFFER) {
        Ok(value) => !matches!(value.as_str(), "0" | "false" | "FALSE" | "off" | "OFF"),
        Err(_) => true,
    }
}

pub fn ingress_metrics_snapshot() -> IngressMetricsSnapshot {
    IngressMetricsSnapshot {
        enabled: ingress_buffer_enabled(),
        buffered_entries: INGRESS_BUFFERED_ENTRIES.load(Ordering::Relaxed),
        subtree_hints: INGRESS_SUBTREE_HINTS.load(Ordering::Relaxed),
        subtree_hint_in_flight: INGRESS_SUBTREE_HINT_IN_FLIGHT.load(Ordering::Relaxed),
        subtree_hint_accepted_total: INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL.load(Ordering::Relaxed),
        subtree_hint_blocked_total: INGRESS_SUBTREE_HINT_BLOCKED_TOTAL.load(Ordering::Relaxed),
        subtree_hint_suppressed_total: INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL
            .load(Ordering::Relaxed),
        subtree_hint_productive_total: INGRESS_SUBTREE_HINT_PRODUCTIVE_TOTAL
            .load(Ordering::Relaxed),
        subtree_hint_unproductive_total: INGRESS_SUBTREE_HINT_UNPRODUCTIVE_TOTAL
            .load(Ordering::Relaxed),
        subtree_hint_dropped_total: INGRESS_SUBTREE_HINT_DROPPED_TOTAL.load(Ordering::Relaxed),
        hot_entries: INGRESS_HOT_ENTRIES.load(Ordering::Relaxed),
        scan_entries: INGRESS_SCAN_ENTRIES.load(Ordering::Relaxed),
        collapsed_total: INGRESS_COLLAPSED_TOTAL.load(Ordering::Relaxed),
        flush_count: INGRESS_FLUSH_COUNT.load(Ordering::Relaxed),
        last_flush_duration_ms: INGRESS_LAST_FLUSH_DURATION_MS.load(Ordering::Relaxed),
        last_promoted_count: INGRESS_LAST_PROMOTED_COUNT.load(Ordering::Relaxed),
        promoted_total: INGRESS_PROMOTED_TOTAL.load(Ordering::Relaxed),
        last_durably_persisted_count: INGRESS_LAST_DURABLY_PERSISTED_COUNT.load(Ordering::Relaxed),
        durably_persisted_total: INGRESS_DURABLY_PERSISTED_TOTAL.load(Ordering::Relaxed),
        last_excluded_from_pending_count: INGRESS_LAST_EXCLUDED_FROM_PENDING_COUNT
            .load(Ordering::Relaxed),
        excluded_from_pending_total: INGRESS_EXCLUDED_FROM_PENDING_TOTAL.load(Ordering::Relaxed),
        drain_batch_size: INGRESS_DRAIN_BATCH_SIZE.load(Ordering::Relaxed),
        drain_heartbeat_tick: INGRESS_DRAIN_HEARTBEAT_TICK.load(Ordering::Relaxed),
        drain_last_batch_sent: INGRESS_DRAIN_LAST_BATCH_SENT.load(Ordering::Relaxed),
        drain_last_batch_dropped_full: INGRESS_DRAIN_LAST_BATCH_DROPPED_FULL
            .load(Ordering::Relaxed),
        drain_dropped_full_total: INGRESS_DRAIN_DROPPED_FULL_TOTAL.load(Ordering::Relaxed),
    }
}

/// REQ-AXO-901678 — published by `pipeline_v2_runtime::spawn_pipeline_v2_indexer`
/// at the tail of each drain tick. Aggregates the per-tick `try_send`
/// outcome into the global ingress metrics so `axon_embedding_status`
/// and `axon_diagnose_indexing` can surface drain saturation without
/// tailing `journalctl`.
pub fn record_drain_tick(batch_size: usize, sent: u64, dropped_full: u64, tick: u64) {
    INGRESS_DRAIN_BATCH_SIZE.store(batch_size, Ordering::Relaxed);
    INGRESS_DRAIN_HEARTBEAT_TICK.store(tick, Ordering::Relaxed);
    INGRESS_DRAIN_LAST_BATCH_SENT.store(sent, Ordering::Relaxed);
    INGRESS_DRAIN_LAST_BATCH_DROPPED_FULL.store(dropped_full, Ordering::Relaxed);
    if dropped_full > 0 {
        INGRESS_DRAIN_DROPPED_FULL_TOTAL.fetch_add(dropped_full, Ordering::Relaxed);
    }
}

/// REQ-AXO-901677 — snapshot of the periodic sweep telemetry exposed
/// by `axon_embedding_status` (JSON block `periodic_sweep`) and the
/// `Periodic sweep` section of `axon_diagnose_indexing` markdown.
///
/// All fields default to 0 before the first sweep runs. `last_run_at_ms`
/// is a Unix epoch ms timestamp ; the reader interprets `0` as "no
/// sweep has executed in this process yet".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeriodicSweepMetricsSnapshot {
    pub last_run_at_ms: u64,
    pub last_duration_ms: u64,
    pub last_files_compared: u64,
    pub last_deltas_found: u64,
    pub runs_total: u64,
    pub deltas_total: u64,
    pub skipped_high_cpu_total: u64,
}

pub fn periodic_sweep_metrics_snapshot() -> PeriodicSweepMetricsSnapshot {
    PeriodicSweepMetricsSnapshot {
        last_run_at_ms: PERIODIC_SWEEP_LAST_RUN_AT_MS.load(Ordering::Relaxed),
        last_duration_ms: PERIODIC_SWEEP_LAST_DURATION_MS.load(Ordering::Relaxed),
        last_files_compared: PERIODIC_SWEEP_LAST_FILES_COMPARED.load(Ordering::Relaxed),
        last_deltas_found: PERIODIC_SWEEP_LAST_DELTAS_FOUND.load(Ordering::Relaxed),
        runs_total: PERIODIC_SWEEP_RUNS_TOTAL.load(Ordering::Relaxed),
        deltas_total: PERIODIC_SWEEP_DELTAS_TOTAL.load(Ordering::Relaxed),
        skipped_high_cpu_total: PERIODIC_SWEEP_SKIPPED_HIGH_CPU_TOTAL.load(Ordering::Relaxed),
    }
}

/// REQ-AXO-901677 — published by `pipeline_v2_runtime::spawn_periodic_sweep_worker`
/// after each successful sweep tick. `now_ms` is the wall-clock
/// timestamp at the END of the sweep ; `duration_ms` is how long the
/// enumerate + compare loop took. `deltas_found` is the count of paths
/// turned into subtree hints this tick. `files_compared` is the total
/// number of paths the scanner enumerated (regardless of delta outcome).
pub fn record_periodic_sweep_tick(
    now_ms: u64,
    duration_ms: u64,
    files_compared: u64,
    deltas_found: u64,
) {
    PERIODIC_SWEEP_LAST_RUN_AT_MS.store(now_ms, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_DURATION_MS.store(duration_ms, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_FILES_COMPARED.store(files_compared, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_DELTAS_FOUND.store(deltas_found, Ordering::Relaxed);
    PERIODIC_SWEEP_RUNS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if deltas_found > 0 {
        PERIODIC_SWEEP_DELTAS_TOTAL.fetch_add(deltas_found, Ordering::Relaxed);
    }
}

/// REQ-AXO-901677 — bumped when the CPU-load gate blocks a scheduled
/// sweep. Surfaced separately from `runs_total` so the operator can
/// detect a worker that's never running due to chronic load pressure.
pub fn record_periodic_sweep_skipped_high_cpu() {
    PERIODIC_SWEEP_SKIPPED_HIGH_CPU_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn reset_periodic_sweep_metrics_for_tests() {
    PERIODIC_SWEEP_LAST_RUN_AT_MS.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_DURATION_MS.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_FILES_COMPARED.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_LAST_DELTAS_FOUND.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_RUNS_TOTAL.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_DELTAS_TOTAL.store(0, Ordering::Relaxed);
    PERIODIC_SWEEP_SKIPPED_HIGH_CPU_TOTAL.store(0, Ordering::Relaxed);
}

pub fn reset_ingress_metrics_for_tests() {
    INGRESS_BUFFERED_ENTRIES.store(0, Ordering::Relaxed);
    INGRESS_HOT_ENTRIES.store(0, Ordering::Relaxed);
    INGRESS_SCAN_ENTRIES.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINTS.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_IN_FLIGHT.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_ACCEPTED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_BLOCKED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_SUPPRESSED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_PRODUCTIVE_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_UNPRODUCTIVE_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_SUBTREE_HINT_DROPPED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_COLLAPSED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_FLUSH_COUNT.store(0, Ordering::Relaxed);
    INGRESS_LAST_FLUSH_DURATION_MS.store(0, Ordering::Relaxed);
    INGRESS_LAST_PROMOTED_COUNT.store(0, Ordering::Relaxed);
    INGRESS_PROMOTED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_LAST_DURABLY_PERSISTED_COUNT.store(0, Ordering::Relaxed);
    INGRESS_DURABLY_PERSISTED_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_LAST_EXCLUDED_FROM_PENDING_COUNT.store(0, Ordering::Relaxed);
    INGRESS_EXCLUDED_FROM_PENDING_TOTAL.store(0, Ordering::Relaxed);
    INGRESS_DRAIN_BATCH_SIZE.store(0, Ordering::Relaxed);
    INGRESS_DRAIN_HEARTBEAT_TICK.store(0, Ordering::Relaxed);
    INGRESS_DRAIN_LAST_BATCH_SENT.store(0, Ordering::Relaxed);
    INGRESS_DRAIN_LAST_BATCH_DROPPED_FULL.store(0, Ordering::Relaxed);
    INGRESS_DRAIN_DROPPED_FULL_TOTAL.store(0, Ordering::Relaxed);
}

pub fn record_ingress_flush(
    duration_ms: u64,
    promoted_count: usize,
    durably_persisted_count: usize,
    excluded_from_pending_count: usize,
) {
    INGRESS_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
    INGRESS_LAST_FLUSH_DURATION_MS.store(duration_ms, Ordering::Relaxed);
    INGRESS_LAST_PROMOTED_COUNT.store(promoted_count as u64, Ordering::Relaxed);
    INGRESS_PROMOTED_TOTAL.fetch_add(promoted_count as u64, Ordering::Relaxed);
    INGRESS_LAST_DURABLY_PERSISTED_COUNT.store(durably_persisted_count as u64, Ordering::Relaxed);
    INGRESS_DURABLY_PERSISTED_TOTAL.fetch_add(durably_persisted_count as u64, Ordering::Relaxed);
    INGRESS_LAST_EXCLUDED_FROM_PENDING_COUNT
        .store(excluded_from_pending_count as u64, Ordering::Relaxed);
    INGRESS_EXCLUDED_FROM_PENDING_TOTAL
        .fetch_add(excluded_from_pending_count as u64, Ordering::Relaxed);
}

pub fn record_blocked_subtree_hint() {
    INGRESS_SUBTREE_HINT_BLOCKED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn subtree_hint_cooldown_ms() -> u64 {
    crate::config::CONFIG
        .indexing
        .subtree_hint_cooldown_ms
        .max(1)
}

fn subtree_hint_retry_budget() -> u64 {
    crate::config::CONFIG
        .indexing
        .subtree_hint_retry_budget
        .max(1)
}

fn watcher_subtree_hint_budget() -> usize {
    // AXON_WATCHER_SUBTREE_HINT_BUDGET retired (REQ-AXO-290 S3) ;
    // streaming-pipeline v2 watcher uses the constant default.
    512
}

impl Default for IngressBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{current_time_ms, IngressBuffer, IngressSource};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn test_subtree_hint_is_evicted_after_retry_budget_when_unproductive() {
        let mut buffer = IngressBuffer::default();
        let path = "/tmp/project/_build_truth";

        buffer.record_subtree_hint(path, 900, IngressSource::Watcher);
        assert_eq!(buffer.subtree_hint_entries(), 1);

        let batch = buffer.drain_batch(10);
        assert_eq!(batch.subtree_hints.len(), 1);

        for _ in 0..3 {
            buffer.complete_subtree_hint_with_stats(path, 0);
        }

        assert_eq!(
            buffer.subtree_hint_entries(),
            0,
            "Un subtree hint non productif doit etre abandonne apres epuisement du budget"
        );
    }

    #[test]
    fn test_subtree_hint_is_preserved_when_it_produces_work() {
        let mut buffer = IngressBuffer::default();
        let path = "/tmp/project/src";

        buffer.record_subtree_hint(path, 900, IngressSource::Watcher);
        let batch = buffer.drain_batch(10);
        assert_eq!(batch.subtree_hints.len(), 1);

        buffer.complete_subtree_hint_with_stats(path, 4);

        assert_eq!(
            buffer.subtree_hint_entries(),
            1,
            "Un subtree hint productif doit rester eligible pour de futures rafales"
        );
    }

    #[test]
    fn test_subtree_hint_custom_cooldown_is_preserved_on_completion() {
        let mut buffer = IngressBuffer::default();
        let path = "/tmp/project/control-scope";
        let custom_cooldown_ms = 60_000;

        buffer.record_subtree_hint_with_cooldown(
            path,
            900,
            IngressSource::Watcher,
            custom_cooldown_ms,
        );
        let batch = buffer.drain_batch(10);
        assert_eq!(batch.subtree_hints.len(), 1);

        let before_complete = current_time_ms();
        buffer.complete_subtree_hint(path);

        let state = buffer
            .subtree_hints
            .get(path)
            .expect("state must remain tracked");
        assert_eq!(state.cooldown_ms, custom_cooldown_ms);
        assert!(
            state.cooldown_until_ms.saturating_sub(before_complete)
                >= custom_cooldown_ms.saturating_sub(50),
            "Le cooldown conserve doit refleter la valeur custom du subtree hint"
        );
    }

    #[test]
    fn test_watcher_subtree_hint_is_dropped_when_budget_is_exhausted() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "2");
        }

        let mut buffer = IngressBuffer::default();
        buffer.record_subtree_hint("/tmp/project/a", 900, IngressSource::Watcher);
        buffer.record_subtree_hint("/tmp/project/b", 900, IngressSource::Watcher);
        buffer.record_subtree_hint("/tmp/project/c", 900, IngressSource::Watcher);

        assert_eq!(
            buffer.subtree_hint_entries(),
            2,
            "Le buffer watcher doit refuser les subtree hints supplémentaires quand le budget est plein"
        );
        assert!(
            buffer.metrics_snapshot().subtree_hint_dropped_total >= 1,
            "Le drop doit être comptabilisé explicitement"
        );

        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }
    }
}
