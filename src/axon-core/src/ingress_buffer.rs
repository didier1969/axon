use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const AXON_ENABLE_INGRESS_BUFFER: &str = "AXON_ENABLE_INGRESS_BUFFER";
pub type SharedIngressBuffer = Arc<Mutex<IngressBuffer>>;

static INGRESS_BUFFERED_ENTRIES: AtomicUsize = AtomicUsize::new(0);
static INGRESS_SUBTREE_HINTS: AtomicUsize = AtomicUsize::new(0);
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
    pub project_slug: String,
    pub size: i64,
    pub mtime: i64,
    pub priority: i64,
    pub source: IngressSource,
    pub cause: IngressCause,
}

impl IngressFileEvent {
    pub fn new(
        path: impl Into<String>,
        project_slug: impl Into<String>,
        size: i64,
        mtime: i64,
        priority: i64,
        source: IngressSource,
        cause: IngressCause,
    ) -> Self {
        Self {
            path: path.into(),
            project_slug: project_slug.into(),
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
    subtree_hints: HashMap<String, IngressSubtreeHint>,
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
        let path = path.into();
        match self.subtree_hints.get_mut(&path) {
            Some(existing) => {
                self.collapsed_events += 1;
                INGRESS_COLLAPSED_TOTAL.fetch_add(1, Ordering::Relaxed);
                existing.priority = existing.priority.max(priority);
                if priority >= existing.priority {
                    existing.source = source;
                }
            }
            None => {
                self.subtree_hints.insert(
                    path.clone(),
                    IngressSubtreeHint {
                        path,
                        priority,
                        source,
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
            hot_entries,
            scan_entries,
            collapsed_total: INGRESS_COLLAPSED_TOTAL.load(Ordering::Relaxed),
            flush_count: INGRESS_FLUSH_COUNT.load(Ordering::Relaxed),
            last_flush_duration_ms: INGRESS_LAST_FLUSH_DURATION_MS.load(Ordering::Relaxed),
            last_promoted_count: INGRESS_LAST_PROMOTED_COUNT.load(Ordering::Relaxed),
        }
    }

    pub fn drain_batch(&mut self, limit: usize) -> IngressDrainBatch {
        let mut drained: Vec<BufferedIngress> =
            self.by_path.drain().map(|(_, value)| value).collect();
        drained.sort_by(|left, right| compare_buffered(left, right));

        let mut files = Vec::new();
        let mut tombstones = Vec::new();

        for entry in drained.into_iter().take(limit.max(1)) {
            match entry {
                BufferedIngress::File(file) => files.push(file),
                BufferedIngress::Tombstone { path, .. } => tombstones.push(path),
            }
        }

        let mut subtree_hints = self
            .subtree_hints
            .drain()
            .map(|(_, hint)| hint)
            .collect::<Vec<_>>();
        subtree_hints.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.path.cmp(&right.path))
        });

        let batch = IngressDrainBatch {
            files,
            tombstones,
            subtree_hints,
            collapsed_events: self.collapsed_events,
        };
        self.collapsed_events = 0;
        self.sync_metrics();
        batch
    }

    fn sync_metrics(&self) {
        INGRESS_BUFFERED_ENTRIES.store(self.by_path.len(), Ordering::Relaxed);
        INGRESS_SUBTREE_HINTS.store(self.subtree_hints.len(), Ordering::Relaxed);
    }
}

fn merge_file_event(existing: &mut IngressFileEvent, incoming: IngressFileEvent) {
    let replace_metadata = incoming.mtime > existing.mtime
        || (incoming.mtime == existing.mtime && incoming.size != existing.size);
    if replace_metadata {
        existing.size = incoming.size;
        existing.mtime = incoming.mtime;
        existing.project_slug = incoming.project_slug;
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

impl Default for IngressBuffer {
    fn default() -> Self {
        Self::new()
    }
}
