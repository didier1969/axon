use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const AXON_ENABLE_INGRESS_BUFFER: &str = "AXON_ENABLE_INGRESS_BUFFER";
pub type SharedIngressBuffer = Arc<Mutex<IngressBuffer>>;

static INGRESS_BUFFERED_ENTRIES: AtomicUsize = AtomicUsize::new(0);
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum BufferedIngress {
    File(IngressFileEvent),
    Tombstone { path: String, source: IngressSource },
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
}

#[derive(Debug)]
pub struct IngressBuffer {
    enabled: bool,
    by_path: HashMap<String, BufferedIngress>,
    subtree_hints: HashMap<String, BufferedSubtreeHint>,
    collapsed_events: u64,
}

impl IngressBuffer {
    pub fn new() -> Self {
        Self {
            enabled: ingress_buffer_enabled(),
            by_path: HashMap::new(),
            subtree_hints: HashMap::new(),
            collapsed_events: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn record_file(&mut self, event: IngressFileEvent) {
        let key = event.path.clone();
        match self.by_path.get_mut(&key) {
            Some(BufferedIngress::File(existing)) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                merge_file_event(existing, event);
            }
            Some(BufferedIngress::Tombstone { .. }) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
            }
            None => {
                self.by_path.insert(key, BufferedIngress::File(event));
            }
        }
        self.sync_metrics();
    }

    pub fn record_tombstone(&mut self, path: impl Into<String>, source: IngressSource) {
        let path = path.into();
        if self
            .by_path
            .insert(
                path.clone(),
                BufferedIngress::Tombstone {
                    path: path.clone(),
                    source,
                },
            )
            .is_some()
        {
            self.collapsed_events += 1;
            INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        self.sync_metrics();
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
                BufferedIngress::File(file) => match file.source {
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
                BufferedIngress::File(file) => files.push(file),
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
        INGRESS_BUFFERED_ENTRIES.store(self.by_path.len(), Ordering::Relaxed);
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

fn compare_buffered(left: &BufferedIngress, right: &BufferedIngress) -> std::cmp::Ordering {
    buffered_priority(right)
        .cmp(&buffered_priority(left))
        .then_with(|| buffered_path(left).cmp(buffered_path(right)))
}

fn buffered_priority(entry: &BufferedIngress) -> i64 {
    match entry {
        BufferedIngress::File(file) => file.priority,
        BufferedIngress::Tombstone { .. } => i64::MAX,
    }
}

fn buffered_path(entry: &BufferedIngress) -> &str {
    match entry {
        BufferedIngress::File(file) => &file.path,
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
        hot_entries: 0,
        scan_entries: 0,
        collapsed_total: INGRESS_COLLAPSED_TOTAL.load(Ordering::Relaxed),
        flush_count: INGRESS_FLUSH_COUNT.load(Ordering::Relaxed),
        last_flush_duration_ms: INGRESS_LAST_FLUSH_DURATION_MS.load(Ordering::Relaxed),
        last_promoted_count: INGRESS_LAST_PROMOTED_COUNT.load(Ordering::Relaxed),
    }
}

pub fn record_ingress_flush(duration_ms: u64, promoted_count: usize) {
    INGRESS_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
    INGRESS_LAST_FLUSH_DURATION_MS.store(duration_ms, Ordering::Relaxed);
    INGRESS_LAST_PROMOTED_COUNT.store(promoted_count as u64, Ordering::Relaxed);
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
    std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(512)
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
