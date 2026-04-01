// Copyright (c) Didier Stadelmann. All rights reserved.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crossbeam_channel::{bounded, select_biased, Receiver, Sender, TrySendError};
use tracing::{info, info_span};

const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_SAFETY_MULTIPLIER: f64 = 2.0;
const STRUCTURE_ONLY_ENVELOPE_RATIO: f64 = 0.28;

#[derive(Debug, Clone)]
struct ReservedTask {
    estimated_cost_bytes: u64,
    estimation_key: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct CostModel {
    sample_count: u32,
    observed_multiplier: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryBudgetSnapshot {
    pub budget_bytes: u64,
    pub reserved_bytes: u64,
    pub exhaustion_ratio: f64,
}

#[derive(Debug)]
struct MemoryBudgetState {
    budget_bytes: u64,
    reserved_bytes: u64,
    reserved_tasks: HashMap<String, ReservedTask>,
    observed_cost_models: HashMap<String, CostModel>,
}

impl MemoryBudgetState {
    fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes: budget_bytes.max(1),
            reserved_bytes: 0,
            reserved_tasks: HashMap::new(),
            observed_cost_models: HashMap::new(),
        }
    }

    fn snapshot(&self) -> MemoryBudgetSnapshot {
        MemoryBudgetSnapshot {
            budget_bytes: self.budget_bytes,
            reserved_bytes: self.reserved_bytes,
            exhaustion_ratio: self.reserved_bytes as f64 / self.budget_bytes.max(1) as f64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLane {
    Hot,
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingMode {
    Full,
    StructureOnly,
}

impl ProcessingMode {
    fn envelope_ratio(self) -> f64 {
        match self {
            ProcessingMode::Full => 1.0,
            ProcessingMode::StructureOnly => STRUCTURE_ONLY_ENVELOPE_RATIO,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub path: String,
    pub trace_id: String,
    pub lane: TaskLane,
    pub size_bytes: u64,
    pub estimated_cost_bytes: u64,
    pub parser_key: String,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
    pub mode: ProcessingMode,
}

pub struct QueueStore {
    priority_sender: Sender<Task>,
    priority_receiver: Receiver<Task>,
    bulk_sender: Sender<Task>,
    bulk_receiver: Receiver<Task>,
    memory_budget: Mutex<MemoryBudgetState>,
}

impl QueueStore {
    pub fn new(capacity: usize) -> Self {
        Self::with_memory_budget(capacity, configured_memory_budget_bytes())
    }

    pub fn with_memory_budget(capacity: usize, memory_budget_bytes: u64) -> Self {
        let capacity = capacity.max(2);
        let prio_cap = (capacity / 5).max(1);
        let bulk_cap = (capacity - prio_cap).max(1);
        let (ps, pr) = bounded(prio_cap);
        let (bs, br) = bounded(bulk_cap);
        Self {
            priority_sender: ps,
            priority_receiver: pr,
            bulk_sender: bs,
            bulk_receiver: br,
            memory_budget: Mutex::new(MemoryBudgetState::new(memory_budget_bytes)),
        }
    }

    pub fn push(&self, path: &str, _mtime: i64, trace_id: &str, t0: i64, t1: i64, priority: bool) -> Result<(), String> {
        self.push_with_mode(path, _mtime, trace_id, t0, t1, priority, ProcessingMode::Full)
    }

    pub fn push_with_mode(
        &self,
        path: &str,
        _mtime: i64,
        trace_id: &str,
        t0: i64,
        t1: i64,
        priority: bool,
        mode: ProcessingMode,
    ) -> Result<(), String> {
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let metadata = std::fs::metadata(path)
            .map_err(|err| format!("Unable to stat file for admission: {:?}", err))?;
        let size_bytes = metadata.len();
        let parser_key = parser_key_for_path(path);
        let estimated_cost_bytes = self.reserve_memory_budget(trace_id, &parser_key, size_bytes, mode)?;
        let lane = if priority {
            TaskLane::Hot
        } else {
            TaskLane::Bulk
        };
        let task = Task {
            path: path.to_string(),
            trace_id: trace_id.to_string(),
            lane,
            size_bytes,
            estimated_cost_bytes,
            parser_key,
            t0, t1, t2,
            mode,
        };

        let send_result = match lane {
            TaskLane::Hot => self
                .priority_sender
                .try_send(task)
                .map_err(|e| format_channel_send_error("Priority", e)),
            TaskLane::Bulk => self
                .bulk_sender
                .try_send(task)
                .map_err(|e| format_channel_send_error("Bulk", e)),
        };

        if let Err(err) = send_result {
            self.release_reservation(trace_id, None);
            Err(err)
        } else {
            Ok(())
        }
    }

    pub fn pop(&self) -> Option<Task> {
        if let Ok(task) = self.priority_receiver.try_recv() {
            return Some(task);
        }
        if let Ok(task) = self.bulk_receiver.try_recv() {
            return Some(task);
        }

        select_biased! {
            recv(self.priority_receiver) -> task => task.ok(),
            recv(self.bulk_receiver) -> task => task.ok(),
        }
    }

    pub fn try_pop(&self) -> Option<Task> {
        self.priority_receiver
            .try_recv()
            .or_else(|_| self.bulk_receiver.try_recv())
            .ok()
    }

    pub fn mark_done(&self, task: &Task, observed_cost_bytes: Option<u64>) -> Result<(), String> {
        self.release_reservation(&task.trace_id, observed_cost_bytes);
        Ok(())
    }

    pub fn purge_all(&self) -> Result<(), String> {
        let _span = info_span!("queue_purge").entered();
        while let Ok(task) = self.priority_receiver.try_recv() {
            self.release_reservation(&task.trace_id, None);
        }
        while let Ok(task) = self.bulk_receiver.try_recv() {
            self.release_reservation(&task.trace_id, None);
        }
        info!("RAM Queues entirely purged for rescan.");
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.priority_sender.len() + self.bulk_sender.len()
    }

    pub fn common_len(&self) -> usize {
        self.priority_sender.len() + self.bulk_sender.len()
    }

    pub fn memory_budget_snapshot(&self) -> MemoryBudgetSnapshot {
        self.memory_budget
            .lock()
            .map(|state| state.snapshot())
            .unwrap_or(MemoryBudgetSnapshot {
                budget_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
                reserved_bytes: 0,
                exhaustion_ratio: 0.0,
            })
    }

    pub fn estimate_cost_for_path(&self, path: &str, size_bytes: u64) -> u64 {
        self.estimate_cost_for_path_in_mode(path, size_bytes, ProcessingMode::Full)
    }

    pub fn estimate_cost_for_path_in_mode(
        &self,
        path: &str,
        size_bytes: u64,
        mode: ProcessingMode,
    ) -> u64 {
        let parser_key = parser_key_for_path(path);
        self.memory_budget
            .lock()
            .map(|state| estimate_cost_bytes(&state, &parser_key, size_bytes, mode))
            .unwrap_or_else(|_| {
                let estimation_key = estimation_key_for(&parser_key, size_bytes, mode);
                let fallback_state = MemoryBudgetState::new(DEFAULT_MEMORY_BUDGET_BYTES);
                estimate_cost_bytes_with_key(&fallback_state, &estimation_key, &parser_key, size_bytes, mode)
            })
    }

    pub fn can_fit_alone(&self, path: &str, size_bytes: u64) -> bool {
        self.can_fit_alone_in_mode(path, size_bytes, ProcessingMode::Full)
    }

    pub fn can_fit_alone_in_mode(&self, path: &str, size_bytes: u64, mode: ProcessingMode) -> bool {
        let estimated = self.estimate_cost_for_path_in_mode(path, size_bytes, mode);
        self.memory_budget_snapshot().budget_bytes >= estimated
    }

    pub fn remaining_budget_bytes(&self) -> u64 {
        let snapshot = self.memory_budget_snapshot();
        snapshot.budget_bytes.saturating_sub(snapshot.reserved_bytes)
    }

    fn reserve_memory_budget(
        &self,
        trace_id: &str,
        parser_key: &str,
        size_bytes: u64,
        mode: ProcessingMode,
    ) -> Result<u64, String> {
        let mut state = self
            .memory_budget
            .lock()
            .map_err(|_| "Memory budget lock poisoned".to_string())?;
        let estimation_key = estimation_key_for(parser_key, size_bytes, mode);
        let estimated_cost_bytes =
            estimate_cost_bytes_with_key(&state, &estimation_key, parser_key, size_bytes, mode);

        if estimated_cost_bytes > state.budget_bytes {
            return Err(format!(
                "Oversized for current budget (estimate={} budget={})",
                estimated_cost_bytes, state.budget_bytes
            ));
        }
        let next_reserved = state.reserved_bytes.saturating_add(estimated_cost_bytes);

        if next_reserved > state.budget_bytes {
            return Err(format!(
                "Memory budget exhausted (reserved={} estimate={} budget={})",
                state.reserved_bytes, estimated_cost_bytes, state.budget_bytes
            ));
        }

        state.reserved_bytes = next_reserved;
        state.reserved_tasks.insert(
            trace_id.to_string(),
            ReservedTask {
                estimated_cost_bytes,
                estimation_key,
                size_bytes,
            },
        );
        Ok(estimated_cost_bytes)
    }

    fn release_reservation(&self, trace_id: &str, observed_cost_bytes: Option<u64>) {
        let Ok(mut state) = self.memory_budget.lock() else {
            return;
        };

        if let Some(reserved_task) = state.reserved_tasks.remove(trace_id) {
            state.reserved_bytes = state
                .reserved_bytes
                .saturating_sub(reserved_task.estimated_cost_bytes);

            if let Some(observed_cost_bytes) = observed_cost_bytes {
                if reserved_task.size_bytes > 0 {
                    let observed_multiplier =
                        observed_cost_bytes as f64 / reserved_task.size_bytes.max(1024) as f64;
                    let entry = state
                        .observed_cost_models
                        .entry(reserved_task.estimation_key)
                        .or_insert_with(|| CostModel {
                            sample_count: 0,
                            observed_multiplier,
                        });
                    entry.sample_count = entry.sample_count.saturating_add(1);
                    if entry.sample_count == 1 {
                        entry.observed_multiplier = observed_multiplier;
                    } else {
                        entry.observed_multiplier =
                            (entry.observed_multiplier * 0.7) + (observed_multiplier * 0.3);
                    }
                }
            }
        }
    }
}

fn format_channel_send_error(channel: &str, err: TrySendError<Task>) -> String {
    match err {
        TrySendError::Full(_) => format!("{} Channel full", channel),
        TrySendError::Disconnected(_) => format!("{} Channel disconnected", channel),
    }
}

pub fn parser_key_for_path(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn estimate_observed_cost_bytes(
    path: &str,
    size_bytes: u64,
    parse_duration: std::time::Duration,
    mode: ProcessingMode,
) -> u64 {
    let parser_key = parser_key_for_path(path);
    let base_multiplier = default_parser_multiplier(&parser_key);
    let duration_multiplier = if parse_duration.as_millis() >= 1_000 {
        4.0
    } else if parse_duration.as_millis() >= 250 {
        2.0
    } else {
        1.0
    };

    ((size_bytes.max(1) as f64) * base_multiplier * duration_multiplier * mode.envelope_ratio())
        .ceil() as u64
}

fn configured_memory_budget_bytes() -> u64 {
    if let Ok(bytes) = std::env::var("AXON_QUEUE_MEMORY_BUDGET_BYTES") {
        if let Ok(parsed) = bytes.parse::<u64>() {
            return parsed.max(DEFAULT_MEMORY_BUDGET_BYTES / 8);
        }
    }

    if let Ok(gb) = std::env::var("AXON_MEMORY_LIMIT_GB") {
        if let Ok(parsed) = gb.parse::<u64>() {
            let bytes = parsed.saturating_mul(1024 * 1024 * 1024);
            return (bytes / 3).max(DEFAULT_MEMORY_BUDGET_BYTES);
        }
    }

    DEFAULT_MEMORY_BUDGET_BYTES
}

fn estimate_cost_bytes(
    state: &MemoryBudgetState,
    parser_key: &str,
    size_bytes: u64,
    mode: ProcessingMode,
) -> u64 {
    let estimation_key = estimation_key_for(parser_key, size_bytes, mode);
    estimate_cost_bytes_with_key(state, &estimation_key, parser_key, size_bytes, mode)
}

fn estimate_cost_bytes_with_key(
    state: &MemoryBudgetState,
    estimation_key: &str,
    parser_key: &str,
    size_bytes: u64,
    mode: ProcessingMode,
) -> u64 {
    let learned_model = state.observed_cost_models.get(estimation_key);
    let learned_multiplier = learned_model
        .map(|model| model.observed_multiplier)
        .unwrap_or_else(|| default_parser_multiplier(parser_key) * mode.envelope_ratio());
    let confidence_multiplier = confidence_safety_multiplier(
        learned_model.map(|model| model.sample_count).unwrap_or(0),
    );

    let base_bytes = size_bytes.max(1) as f64;
    let estimated = base_bytes * learned_multiplier * DEFAULT_SAFETY_MULTIPLIER * confidence_multiplier;
    estimated.ceil() as u64
}

fn estimation_key_for(parser_key: &str, size_bytes: u64, mode: ProcessingMode) -> String {
    format!("{}:{}:{}", parser_key, size_bucket_for(size_bytes), mode_key(mode))
}

fn size_bucket_for(size_bytes: u64) -> &'static str {
    const TINY_MAX: u64 = 64 * 1024;
    const SMALL_MAX: u64 = 256 * 1024;
    const MEDIUM_MAX: u64 = 1024 * 1024;

    match size_bytes {
        0..=TINY_MAX => "tiny",
        65_537..=SMALL_MAX => "small",
        262_145..=MEDIUM_MAX => "medium",
        _ => "large",
    }
}

fn confidence_safety_multiplier(sample_count: u32) -> f64 {
    match sample_count {
        0 => 1.75,
        1 => 1.60,
        2 => 1.45,
        3 => 1.30,
        4 => 1.15,
        _ => 1.0,
    }
}

fn mode_key(mode: ProcessingMode) -> &'static str {
    match mode {
        ProcessingMode::Full => "full",
        ProcessingMode::StructureOnly => "structure_only",
    }
}

fn default_parser_multiplier(parser_key: &str) -> f64 {
    match parser_key {
        "rs" | "ex" | "exs" | "ts" | "tsx" | "js" | "jsx" => 192.0,
        "md" | "markdown" | "html" | "htm" | "sql" | "java" => 128.0,
        "yaml" | "yml" | "css" | "scss" | "txt" | "conf" | "ini" => 96.0,
        _ => 128.0,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{estimate_observed_cost_bytes, parser_key_for_path, ProcessingMode, QueueStore, TaskLane};

    #[test]
    fn test_hot_lane_never_starves_behind_bulk_work() {
        let temp = tempfile::tempdir().unwrap();
        let bulk_a = temp.path().join("bulk_a.ex");
        let bulk_b = temp.path().join("bulk_b.ex");
        let hot = temp.path().join("hot.ex");
        std::fs::write(&bulk_a, "defmodule BulkA do end").unwrap();
        std::fs::write(&bulk_b, "defmodule BulkB do end").unwrap();
        std::fs::write(&hot, "defmodule Hot do end").unwrap();
        let queue = QueueStore::new(10);
        queue.push(bulk_a.to_string_lossy().as_ref(), 0, "bulk-a", 0, 0, false).unwrap();
        queue.push(bulk_b.to_string_lossy().as_ref(), 0, "bulk-b", 0, 0, false).unwrap();
        queue.push(hot.to_string_lossy().as_ref(), 0, "hot", 0, 0, true).unwrap();

        let first = queue.pop().expect("hot lane should be served first");
        assert_eq!(first.trace_id, "hot");
    }

    #[test]
    fn test_bulk_lane_hits_backpressure_before_hot_lane() {
        let temp = tempfile::tempdir().unwrap();
        let queue = QueueStore::new(10);

        for idx in 0..8 {
            let path = temp.path().join(format!("bulk_{}.ex", idx));
            std::fs::write(&path, "defmodule Bulk do end").unwrap();
            queue
                .push(path.to_string_lossy().as_ref(), 0, &format!("bulk-{}", idx), 0, 0, false)
                .unwrap();
        }

        let overflow_path = temp.path().join("bulk_overflow.ex");
        std::fs::write(&overflow_path, "defmodule BulkOverflow do end").unwrap();
        let overflow =
            queue.push(overflow_path.to_string_lossy().as_ref(), 0, "bulk-overflow", 0, 0, false);
        assert!(overflow.is_err(), "bulk lane should saturate before borrowing hot capacity");

        let hot_reserved = temp.path().join("hot_reserved.ex");
        std::fs::write(&hot_reserved, "defmodule HotReserved do end").unwrap();
        queue
            .push(hot_reserved.to_string_lossy().as_ref(), 0, "hot-reserved", 0, 0, true)
            .expect("hot lane must retain reserved capacity under bulk pressure");
    }

    #[test]
    fn test_large_non_hot_files_stay_in_common_lane_and_are_budget_governed() {
        let temp = tempfile::tempdir().unwrap();
        let large_path = temp.path().join("large.rs");
        std::fs::write(&large_path, vec![b'x'; 2 * 1024 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(10, 2 * 1024 * 1024 * 1024);
        queue
            .push(large_path.to_string_lossy().as_ref(), 0, "large", 0, 0, false)
            .unwrap();

        let task = queue
            .pop()
            .expect("large file should still be admitted through the common lane when budget allows it");
        assert_eq!(task.trace_id, "large");
        assert_eq!(task.lane, TaskLane::Bulk);
    }

    #[test]
    fn test_memory_budget_allows_many_small_files_until_budget_is_full() {
        let temp = tempfile::tempdir().unwrap();
        let queue = QueueStore::with_memory_budget(10, 2_500_000);

        for idx in 0..3 {
            let path = temp.path().join(format!("small_{}.rs", idx));
            std::fs::write(&path, vec![b'x'; 1024]).unwrap();
            queue
                .push(path.to_string_lossy().as_ref(), 0, &format!("small-{}", idx), 0, 0, false)
                .unwrap();
        }

        let fourth = temp.path().join("small_3.rs");
        std::fs::write(&fourth, vec![b'x'; 1024]).unwrap();
        let overflow = queue.push(fourth.to_string_lossy().as_ref(), 0, "small-overflow", 0, 0, false);

        assert!(overflow.is_err(), "memory budget should stop over-admission even with free channel slots");
        assert!(queue.memory_budget_snapshot().reserved_bytes > 0);
    }

    #[test]
    fn test_memory_budget_large_file_reduces_parallelism() {
        let temp = tempfile::tempdir().unwrap();
        let queue = QueueStore::with_memory_budget(10, 10_000_000);
        let large = temp.path().join("large.rs");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        queue
            .push(large.to_string_lossy().as_ref(), 0, "large", 0, 0, false)
            .unwrap();

        let small = temp.path().join("small.rs");
        std::fs::write(&small, vec![b'x'; 1024]).unwrap();
        queue
            .push(small.to_string_lossy().as_ref(), 0, "small", 0, 0, false)
            .unwrap();

        let second_large = temp.path().join("second_large.rs");
        std::fs::write(&second_large, vec![b'x'; 8 * 1024]).unwrap();
        let blocked = queue.push(second_large.to_string_lossy().as_ref(), 0, "second-large", 0, 0, false);

        assert!(blocked.is_err(), "a second large file should wait until budget is released");
    }

    #[test]
    fn test_memory_budget_resumes_after_task_completion() {
        let temp = tempfile::tempdir().unwrap();
        let queue = QueueStore::with_memory_budget(10, 6_000_000);
        let large = temp.path().join("large.rs");
        std::fs::write(&large, vec![b'x'; 4 * 1024]).unwrap();
        queue
            .push(large.to_string_lossy().as_ref(), 0, "large", 0, 0, false)
            .unwrap();

        let task = queue.pop().expect("queued task should be available");
        let snapshot_before = queue.memory_budget_snapshot();
        assert!(snapshot_before.reserved_bytes > 0);

        queue
            .mark_done(
                &task,
                Some(estimate_observed_cost_bytes(
                    &task.path,
                    task.size_bytes,
                    Duration::from_millis(400),
                    task.mode,
                )),
            )
            .unwrap();

        let snapshot_after = queue.memory_budget_snapshot();
        assert_eq!(snapshot_after.reserved_bytes, 0);

        let next = temp.path().join("next.rs");
        std::fs::write(&next, vec![b'x'; 4 * 1024]).unwrap();
        queue
            .push(next.to_string_lossy().as_ref(), 0, "next", 0, 0, false)
            .expect("admission should resume after reservation release");
    }

    #[test]
    fn test_confident_parser_bucket_admits_more_than_cold_start() {
        let temp = tempfile::tempdir().unwrap();
        let queue = QueueStore::with_memory_budget(10, 800_000);

        let cold_a = temp.path().join("cold_a.ex");
        let cold_b = temp.path().join("cold_b.ex");
        std::fs::write(&cold_a, vec![b'x'; 1024]).unwrap();
        std::fs::write(&cold_b, vec![b'x'; 1024]).unwrap();

        queue
            .push(cold_a.to_string_lossy().as_ref(), 0, "cold-a", 0, 0, false)
            .unwrap();
        let cold_overflow =
            queue.push(cold_b.to_string_lossy().as_ref(), 0, "cold-b", 0, 0, false);
        assert!(
            cold_overflow.is_err(),
            "cold-start admission should stay conservative before this parser bucket is known"
        );

        let first = queue.pop().unwrap();
        queue
            .mark_done(
                &first,
                Some(estimate_observed_cost_bytes(
                    &first.path,
                    first.size_bytes,
                    Duration::from_millis(20),
                    first.mode,
                )),
            )
            .unwrap();

        for idx in 0..5 {
            let path = temp.path().join(format!("warm_{}.ex", idx));
            std::fs::write(&path, vec![b'x'; 1024]).unwrap();
            queue
                .push(path.to_string_lossy().as_ref(), 0, &format!("warm-{}", idx), 0, 0, false)
                .unwrap();
            let task = queue.pop().unwrap();
            queue
                .mark_done(
                    &task,
                    Some(estimate_observed_cost_bytes(
                        &task.path,
                        task.size_bytes,
                        Duration::from_millis(20),
                        task.mode,
                    )),
                )
                .unwrap();
        }

        let known_a = temp.path().join("known_a.ex");
        let known_b = temp.path().join("known_b.ex");
        std::fs::write(&known_a, vec![b'x'; 1024]).unwrap();
        std::fs::write(&known_b, vec![b'x'; 1024]).unwrap();

        queue
            .push(known_a.to_string_lossy().as_ref(), 0, "known-a", 0, 0, false)
            .unwrap();
        queue
            .push(known_b.to_string_lossy().as_ref(), 0, "known-b", 0, 0, false)
            .expect("known parser bucket should admit more work after warm observations");
    }

    #[test]
    fn test_parser_key_for_path_uses_extension_or_unknown() {
        assert_eq!(parser_key_for_path("/tmp/file.rs"), "rs");
        assert_eq!(parser_key_for_path("/tmp/file"), "unknown");
    }

    #[test]
    fn test_estimate_observed_cost_penalizes_slow_parses() {
        let fast =
            estimate_observed_cost_bytes("/tmp/file.rs", 1024, Duration::from_millis(50), ProcessingMode::Full);
        let slow =
            estimate_observed_cost_bytes("/tmp/file.rs", 1024, Duration::from_millis(1200), ProcessingMode::Full);
        assert!(slow > fast);
    }

    #[test]
    fn test_structure_only_estimate_is_lower_than_full_estimate() {
        let queue = QueueStore::with_memory_budget(10, 16 * 1024 * 1024);
        let full = queue.estimate_cost_for_path_in_mode("/tmp/example.rs", 16 * 1024, ProcessingMode::Full);
        let structure_only = queue
            .estimate_cost_for_path_in_mode("/tmp/example.rs", 16 * 1024, ProcessingMode::StructureOnly);

        assert!(structure_only < full);
    }

    #[test]
    fn test_unknown_parser_bucket_starts_conservative_then_relaxes_with_observations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sample.ex");
        std::fs::write(&path, "defmodule Sample do\nend\n").unwrap();

        let queue = QueueStore::with_memory_budget(10, 1024 * 1024 * 1024);
        let initial = queue.estimate_cost_for_path(path.to_string_lossy().as_ref(), 4096);

        for idx in 0..6 {
            let trace_id = format!("sample-{}", idx);
            queue
                .push(path.to_string_lossy().as_ref(), 0, &trace_id, 0, 0, false)
                .unwrap();
            let task = queue.pop().unwrap();
            queue
                .mark_done(&task, Some(32 * 1024))
                .unwrap();
        }

        let learned = queue.estimate_cost_for_path(path.to_string_lossy().as_ref(), 4096);
        assert!(
            learned < initial,
            "after repeated low-cost observations, the estimate should relax for this parser bucket"
        );
    }
}
