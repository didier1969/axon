use crate::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_snapshot,
    current_gpu_utilization_snapshot, current_runtime_tuning_state,
};
use crate::embedding_contract::CHUNK_MODEL_ID;
use crate::graph::GraphStore;
use crate::runtime_observability::{
    duckdb_memory_snapshot, process_memory_snapshot, DuckDbMemorySnapshot, ProcessMemorySnapshot,
};
use crate::runtime_profile::RuntimeProfile;
use crate::runtime_tuning::RuntimeTuningState;
use crate::service_guard;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostSnapshot {
    pub captured_at_ms: i64,
    pub source: String,
    pub platform: String,
    pub is_wsl: bool,
    pub cpu_cores: usize,
    pub ram_total_bytes: u64,
    pub gpu_present: bool,
    pub gpu_name: Option<String>,
    pub vram_total_mb: u64,
    pub io_characteristics: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RuntimeSignalsWindow {
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub captured_at_ms: i64,
    pub source: String,
    pub cpu_usage_ratio: f64,
    pub ram_available_ratio: f64,
    pub io_wait_ratio: f64,
    pub process_memory: ProcessMemorySnapshot,
    pub duckdb_memory: DuckDbMemorySnapshot,
    pub vram_used_mb: u64,
    pub vram_free_mb: u64,
    pub gpu_utilization_ratio: f64,
    pub gpu_memory_utilization_ratio: f64,
    pub file_vectorization_queue_depth: usize,
    pub graph_projection_queue_depth: usize,
    pub canonical_vector_backlog_depth: usize,
    pub ready_queue_depth_current: u64,
    pub ready_queue_depth_max: u64,
    pub active_claimed_current: u64,
    pub prepare_claimed_current: u64,
    pub ready_claimed_current: u64,
    pub persist_queue_depth_current: u64,
    pub persist_queue_depth_max: u64,
    pub persist_claimed_current: u64,
    pub prepare_inflight_current: u64,
    pub prepare_inflight_max: u64,
    pub gpu_idle_wait_ms_total: u64,
    pub prepare_queue_wait_ms_total: u64,
    pub prepare_reply_wait_ms_total: u64,
    pub persist_queue_wait_ms_total: u64,
    pub oldest_ready_batch_age_ms_current: u64,
    pub oldest_ready_batch_age_ms_max: u64,
    pub latency_recent_fetch_p95_ms: u64,
    pub latency_recent_embed_p95_ms: u64,
    pub latency_recent_db_write_p95_ms: u64,
    pub latency_recent_mark_done_p95_ms: u64,
    pub mcp_latency_recent_ms: u64,
    pub vector_workers_active_current: u64,
    pub vector_worker_heartbeat_at_ms: u64,
    pub embed_inflight_started_at_ms: u64,
    pub interactive_requests_in_flight: u64,
    pub interactive_priority: String,
    pub chunk_embedding_writes_total: u64,
    pub files_completed_total: u64,
    pub canonical_chunk_embeddings_total: u64,
    pub canonical_files_embedded_total: u64,
    pub canonical_chunks_embedded_last_minute: u64,
    pub canonical_files_embedded_last_minute: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperatorPolicySnapshot {
    pub captured_at_ms: i64,
    pub max_cpu_ratio: f64,
    pub min_ram_available_ratio: f64,
    pub max_mcp_p95_ms: u64,
    pub max_vram_used_ratio: f64,
    pub max_vram_used_mb: u64,
    pub max_io_wait_ratio: f64,
    pub backlog_priority_weight: f64,
    pub interactive_priority_weight: f64,
    pub shadow_mode_enabled: bool,
    pub allowed_actuators: Vec<String>,
    pub evaluation_window_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RecentAnalyticsWindow {
    pub collected_at_ms: i64,
    pub current_hour_bucket_start_ms: i64,
    pub chunks_embedded_current_hour: u64,
    pub files_vector_ready_current_hour: u64,
    pub batches_current_hour: u64,
    pub embed_ms_total_current_hour: u64,
    pub db_write_ms_total_current_hour: u64,
    pub mark_done_ms_total_current_hour: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionProfile {
    pub id: String,
    pub label: String,
    pub target_vector_workers: usize,
    pub target_chunk_batch_size: usize,
    pub target_file_vectorization_batch_size: usize,
    pub target_ready_queue_depth: usize,
    pub target_persist_queue_bound: usize,
    pub target_max_inflight_persists: usize,
    pub target_embed_micro_batch_max_items: usize,
    pub target_embed_micro_batch_max_total_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptimizerDecision {
    pub decision_id: String,
    pub proposed_at_ms: i64,
    pub action_profile_id: String,
    pub decision_reason: String,
    pub score: f64,
    pub confidence: f64,
    pub evaluation_window_start_ms: i64,
    pub evaluation_window_end_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RewardObservation {
    pub decision_id: String,
    pub observed_at_ms: i64,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub throughput_chunks_per_hour: f64,
    pub throughput_files_per_hour: f64,
    pub reward: f64,
    pub penalty_cpu: f64,
    pub penalty_ram: f64,
    pub penalty_vram: f64,
    pub penalty_mcp: f64,
    pub penalty_io: f64,
    pub penalty_liveness: f64,
    pub penalty_stability: f64,
    pub penalty_churn: f64,
}

pub trait PolicyEngine {
    fn choose(
        &self,
        host: &HostSnapshot,
        signals: &RuntimeSignalsWindow,
        policy: &OperatorPolicySnapshot,
        analytics: &RecentAnalyticsWindow,
        action_profiles: &[ActionProfile],
    ) -> Option<OptimizerDecision>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LiveProfileStats {
    pub pulls: u64,
    pub total_reward: f64,
    pub mean_reward: f64,
    pub last_reward: f64,
    pub last_throughput_chunks_per_hour: f64,
    pub last_observed_at_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct LiveDecisionMemory {
    by_profile: BTreeMap<String, LiveProfileStats>,
    by_decision: BTreeMap<String, String>,
    total_observations: u64,
}

static LIVE_DECISION_MEMORY: OnceLock<Mutex<LiveDecisionMemory>> = OnceLock::new();

fn live_decision_memory() -> &'static Mutex<LiveDecisionMemory> {
    LIVE_DECISION_MEMORY.get_or_init(|| Mutex::new(LiveDecisionMemory::default()))
}

pub fn remember_optimizer_decision(decision: &OptimizerDecision) {
    let mut memory = live_decision_memory()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    memory.by_decision.insert(
        decision.decision_id.clone(),
        decision.action_profile_id.clone(),
    );
}

pub fn record_optimizer_outcome(reward: &RewardObservation) {
    let mut memory = live_decision_memory()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let Some(profile_id) = memory.by_decision.remove(&reward.decision_id) else {
        return;
    };
    let stats = memory.by_profile.entry(profile_id).or_default();
    stats.pulls = stats.pulls.saturating_add(1);
    stats.total_reward += reward.reward;
    stats.mean_reward = stats.total_reward / stats.pulls as f64;
    stats.last_reward = reward.reward;
    stats.last_throughput_chunks_per_hour = reward.throughput_chunks_per_hour;
    stats.last_observed_at_ms = reward.observed_at_ms;
    memory.total_observations = memory.total_observations.saturating_add(1);
}

#[cfg(test)]
pub fn reset_live_decision_memory() {
    let mut memory = live_decision_memory()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *memory = LiveDecisionMemory::default();
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicPolicyEngine;

impl PolicyEngine for HeuristicPolicyEngine {
    fn choose(
        &self,
        host: &HostSnapshot,
        signals: &RuntimeSignalsWindow,
        policy: &OperatorPolicySnapshot,
        analytics: &RecentAnalyticsWindow,
        action_profiles: &[ActionProfile],
    ) -> Option<OptimizerDecision> {
        let now_ms = now_ms();
        let eval_window_start_ms = now_ms;
        let eval_window_end_ms =
            now_ms.saturating_add(i64::try_from(policy.evaluation_window_ms).unwrap_or(i64::MAX));
        let mut best: Option<(ActionProfile, f64, String)> = None;
        let current_state = current_runtime_tuning_state();

        for profile in action_profiles {
            let score_embed_throughput_weight =
                env_f64("AXON_OPT_SCORE_EMBED_THROUGHPUT_WEIGHT", 1.0).max(0.1);
            let mut score = analytics.chunks_embedded_current_hour as f64
                * policy.backlog_priority_weight.max(0.1)
                * score_embed_throughput_weight;
            let mut reasons = Vec::new();
            let backlog_depth = signals.file_vectorization_queue_depth.max(1);
            let gpu_underutilized_ratio =
                env_f64("AXON_OPT_GPU_UNDERUTILIZED_RATIO", 0.35).clamp(0.0, 1.0);
            let gpu_headroom_margin_mb = env_u64("AXON_OPT_GPU_HEADROOM_MARGIN_MB", 512);
            let warmup_backlog_threshold =
                env_u64("AXON_OPT_WARMUP_BACKLOG_THRESHOLD", 32) as usize;
            let score_batch_gt_backlog_penalty =
                env_f64("AXON_OPT_SCORE_BATCH_GT_BACKLOG_PENALTY", 1.0);
            let score_cpu_parallelism_risk_penalty =
                env_f64("AXON_OPT_SCORE_CPU_PARALLELISM_RISK_PENALTY", 5.0);
            let score_cpu_guard_penalty = env_f64("AXON_OPT_SCORE_CPU_GUARD_PENALTY", 10.0);
            let score_cpu_headroom_bonus = env_f64("AXON_OPT_SCORE_CPU_HEADROOM_BONUS", 0.5);
            let score_ram_guard_penalty = env_f64("AXON_OPT_SCORE_RAM_GUARD_PENALTY", 10.0);
            let score_vram_guard_penalty = env_f64("AXON_OPT_SCORE_VRAM_GUARD_PENALTY", 12.0);
            let score_gpu_batch_depth_divisor =
                env_f64("AXON_OPT_SCORE_GPU_BATCH_DEPTH_DIVISOR", 64.0).max(1.0);
            let score_gpu_underutilized_open_batch_bonus =
                env_f64("AXON_OPT_SCORE_GPU_UNDERUTILIZED_OPEN_BATCH_BONUS", 4.0);
            let score_gpu_underutilized_small_batch_penalty =
                env_f64("AXON_OPT_SCORE_GPU_UNDERUTILIZED_SMALL_BATCH_PENALTY", 2.0);
            let score_gpu_underutilized_open_workers_bonus =
                env_f64("AXON_OPT_SCORE_GPU_UNDERUTILIZED_OPEN_WORKERS_BONUS", 1.0);
            let score_warmup_prefers_depth_bonus =
                env_f64("AXON_OPT_SCORE_WARMUP_PREFERS_DEPTH_BONUS", 2.0);
            let score_warmup_avoids_worker_fanout_penalty =
                env_f64("AXON_OPT_SCORE_WARMUP_AVOIDS_WORKER_FANOUT_PENALTY", 1.0);
            let score_io_wait_guard_penalty = env_f64("AXON_OPT_SCORE_IO_WAIT_GUARD_PENALTY", 4.0);
            let score_mcp_guard_penalty = env_f64("AXON_OPT_SCORE_MCP_GUARD_PENALTY", 8.0);
            let score_interactive_pressure_penalty =
                env_f64("AXON_OPT_SCORE_INTERACTIVE_PRESSURE_PENALTY", 2.0);
            let score_embed_inflight_penalty =
                env_f64("AXON_OPT_SCORE_EMBED_INFLIGHT_PENALTY", 1.5);
            let score_overly_small_batch_penalty =
                env_f64("AXON_OPT_SCORE_OVERLY_SMALL_BATCH_PENALTY", 1.0);
            let score_ready_depth_starvation_bonus =
                env_f64("AXON_OPT_SCORE_READY_DEPTH_STARVATION_BONUS", 3.0);
            let score_persist_relief_bonus = env_f64("AXON_OPT_SCORE_PERSIST_RELIEF_BONUS", 1.5);
            let score_overdeep_queue_penalty =
                env_f64("AXON_OPT_SCORE_OVERDEEP_QUEUE_PENALTY", 1.0);
            let score_bottleneck_backlog_open_batch_bonus =
                env_f64("AXON_OPT_SCORE_BOTTLENECK_BACKLOG_OPEN_BATCH_BONUS", 3.0);
            let score_bottleneck_backlog_open_file_batch_bonus = env_f64(
                "AXON_OPT_SCORE_BOTTLENECK_BACKLOG_OPEN_FILE_BATCH_BONUS",
                1.5,
            );
            let score_bottleneck_backlog_open_items_bonus =
                env_f64("AXON_OPT_SCORE_BOTTLENECK_BACKLOG_OPEN_ITEMS_BONUS", 1.5);
            let score_bottleneck_backlog_open_tokens_bonus =
                env_f64("AXON_OPT_SCORE_BOTTLENECK_BACKLOG_OPEN_TOKENS_BONUS", 1.5);
            let score_bottleneck_gpu_busy_bonus =
                env_f64("AXON_OPT_SCORE_BOTTLENECK_GPU_BUSY_BONUS", 2.0);
            let gpu_underutilized = host.gpu_present
                && backlog_depth >= profile.target_chunk_batch_size.max(16)
                && signals.gpu_utilization_ratio < gpu_underutilized_ratio
                && signals.vram_used_mb
                    < policy
                        .max_vram_used_mb
                        .saturating_sub(gpu_headroom_margin_mb);
            let warmup_active = host.gpu_present
                && backlog_depth >= warmup_backlog_threshold
                && analytics.chunks_embedded_current_hour == 0
                && signals.canonical_chunks_embedded_last_minute == 0;

            if profile.target_chunk_batch_size > backlog_depth {
                score -= score_batch_gt_backlog_penalty;
                reasons.push("batch_gt_backlog");
            }
            if profile.target_vector_workers > 1 && !host.gpu_present {
                score -= score_cpu_parallelism_risk_penalty;
                reasons.push("cpu_parallelism_risk");
            }
            if signals.cpu_usage_ratio > policy.max_cpu_ratio {
                let cpu_excess = (signals.cpu_usage_ratio - policy.max_cpu_ratio)
                    / (1.0 - policy.max_cpu_ratio).max(0.01);
                score -= score_cpu_guard_penalty * cpu_excess.clamp(0.0, 1.0);
                reasons.push("cpu_guard_active");
            } else if profile.target_vector_workers > 1 {
                score += score_cpu_headroom_bonus;
                reasons.push("cpu_headroom");
            }
            if signals.ram_available_ratio < policy.min_ram_available_ratio {
                score -= score_ram_guard_penalty;
                reasons.push("ram_guard_active");
            }
            if signals.vram_used_mb > policy.max_vram_used_mb {
                score -= score_vram_guard_penalty;
                reasons.push("vram_guard_active");
            } else if host.gpu_present && profile.target_chunk_batch_size > 0 {
                score += (profile.target_chunk_batch_size as f64) / score_gpu_batch_depth_divisor;
                reasons.push("gpu_batch_depth");
            }
            if gpu_underutilized {
                if profile.target_chunk_batch_size >= signals.file_vectorization_queue_depth.min(64)
                    || profile.target_chunk_batch_size > 48
                {
                    score += score_gpu_underutilized_open_batch_bonus;
                    reasons.push("gpu_underutilized_open_batch");
                } else {
                    score -= score_gpu_underutilized_small_batch_penalty;
                    reasons.push("gpu_underutilized_but_batch_small");
                }
                if profile.target_vector_workers > 1 {
                    score += score_gpu_underutilized_open_workers_bonus;
                    reasons.push("gpu_underutilized_open_workers");
                }
            }
            if warmup_active {
                if profile.target_chunk_batch_size >= 48 {
                    score += score_warmup_prefers_depth_bonus;
                    reasons.push("warmup_prefers_depth");
                }
                if profile.target_vector_workers > 1 {
                    score -= score_warmup_avoids_worker_fanout_penalty;
                    reasons.push("warmup_avoids_worker_fanout");
                }
            }
            if signals.io_wait_ratio > policy.max_io_wait_ratio {
                score -= score_io_wait_guard_penalty;
                reasons.push("io_wait_guard_active");
            }
            if signals.mcp_latency_recent_ms > policy.max_mcp_p95_ms {
                score -= score_mcp_guard_penalty * policy.interactive_priority_weight.max(0.1);
                reasons.push("mcp_guard_active");
            }
            if signals.interactive_requests_in_flight > 0
                || signals.interactive_priority != "background_normal"
            {
                score -= score_interactive_pressure_penalty
                    * policy.interactive_priority_weight.max(0.1);
                reasons.push("interactive_pressure");
            }
            if signals.embed_inflight_started_at_ms > 0
                && signals.vector_workers_active_current > 0
                && signals.file_vectorization_queue_depth > 0
            {
                score -= score_embed_inflight_penalty;
                reasons.push("embed_inflight");
            }
            if profile.target_chunk_batch_size < 8 {
                score -= score_overly_small_batch_penalty;
                reasons.push("overly_small_batch");
            }
            if signals.ready_queue_depth_current == 0
                && signals.file_vectorization_queue_depth >= 32
            {
                if profile.target_ready_queue_depth > current_state.vector_ready_queue_depth {
                    score += score_ready_depth_starvation_bonus;
                    reasons.push("ready_depth_relieves_starvation");
                }
                if profile.target_file_vectorization_batch_size
                    > current_state.file_vectorization_batch_size
                {
                    score += score_ready_depth_starvation_bonus * 0.5;
                    reasons.push("file_batch_relieves_starvation");
                }
            }
            if signals.persist_queue_depth_current > 0
                && profile.target_persist_queue_bound > current_state.vector_persist_queue_bound
            {
                score += score_persist_relief_bonus;
                reasons.push("persist_relief");
            }
            if signals.cpu_usage_ratio > policy.max_cpu_ratio * 0.9
                && profile.target_ready_queue_depth > current_state.vector_ready_queue_depth
            {
                score -= score_overdeep_queue_penalty;
                reasons.push("avoid_overdeep_ready_under_cpu_pressure");
            }
            if signals.vram_used_mb > policy.max_vram_used_mb.saturating_mul(9) / 10
                && profile.target_embed_micro_batch_max_total_tokens
                    > current_state.embed_micro_batch_max_total_tokens
            {
                score -= score_vram_guard_penalty * 0.5;
                reasons.push("avoid_token_budget_growth_near_vram_limit");
            }
            if backlog_depth >= 32
                && signals.cpu_usage_ratio <= policy.max_cpu_ratio
                && signals.ram_available_ratio >= policy.min_ram_available_ratio
                && signals.io_wait_ratio <= policy.max_io_wait_ratio
            {
                if profile.target_chunk_batch_size > current_state.chunk_batch_size {
                    score += score_bottleneck_backlog_open_batch_bonus;
                    reasons.push("bottleneck_prefers_chunk_batch_growth");
                }
                if profile.target_file_vectorization_batch_size
                    > current_state.file_vectorization_batch_size
                {
                    score += score_bottleneck_backlog_open_file_batch_bonus;
                    reasons.push("bottleneck_prefers_file_batch_growth");
                }
                if profile.target_embed_micro_batch_max_items
                    > current_state.embed_micro_batch_max_items
                {
                    score += score_bottleneck_backlog_open_items_bonus;
                    reasons.push("bottleneck_prefers_micro_batch_items_growth");
                }
                if profile.target_embed_micro_batch_max_total_tokens
                    > current_state.embed_micro_batch_max_total_tokens
                {
                    score += score_bottleneck_backlog_open_tokens_bonus;
                    reasons.push("bottleneck_prefers_micro_batch_tokens_growth");
                }
                if signals.ready_queue_depth_current > 0
                    && signals.gpu_utilization_ratio >= gpu_underutilized_ratio
                {
                    score += score_bottleneck_gpu_busy_bonus;
                    reasons.push("bottleneck_rewards_busy_gpu");
                }
            }

            let reason = reasons.join(",");
            if best
                .as_ref()
                .map(|(_, best_score, _)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((profile.clone(), score, reason));
            }
        }

        let (profile, score, reason) = best?;
        Some(OptimizerDecision {
            decision_id: format!("opt-{}", now_ms),
            proposed_at_ms: now_ms,
            action_profile_id: profile.id,
            decision_reason: reason,
            score,
            confidence: if action_profiles.len() <= 1 { 1.0 } else { 0.6 },
            evaluation_window_start_ms: eval_window_start_ms,
            evaluation_window_end_ms: eval_window_end_ms,
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LiveBanditPolicyEngine;

impl PolicyEngine for LiveBanditPolicyEngine {
    fn choose(
        &self,
        _host: &HostSnapshot,
        signals: &RuntimeSignalsWindow,
        policy: &OperatorPolicySnapshot,
        _analytics: &RecentAnalyticsWindow,
        action_profiles: &[ActionProfile],
    ) -> Option<OptimizerDecision> {
        let now_ms = now_ms();
        let eval_window_start_ms = now_ms;
        let eval_window_end_ms =
            now_ms.saturating_add(i64::try_from(policy.evaluation_window_ms).unwrap_or(i64::MAX));
        let current = current_runtime_tuning_state();
        let memory = live_decision_memory()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let total_observations = memory.total_observations.max(1) as f64;
        let exploration_weight = env_f64("AXON_OPT_LIVE_UCB_EXPLORATION_WEIGHT", 8.0).max(0.1);
        let backlog_weight = env_f64("AXON_OPT_LIVE_BACKLOG_WEIGHT", 6.0).max(0.0);
        let gpu_idle_penalty_weight =
            env_f64("AXON_OPT_LIVE_GPU_IDLE_PENALTY_WEIGHT", 4.0).max(0.0);
        let no_progress_penalty_weight =
            env_f64("AXON_OPT_LIVE_NO_PROGRESS_PENALTY_WEIGHT", 10.0).max(0.0);
        let anchor_fill_bonus_weight =
            env_f64("AXON_OPT_LIVE_ANCHOR_FILL_BONUS_WEIGHT", 14.0).max(0.0);
        let mut best: Option<(ActionProfile, f64, String)> = None;

        for profile in action_profiles {
            let stats = memory
                .by_profile
                .get(&profile.id)
                .cloned()
                .unwrap_or_default();
            let exploitation = stats.mean_reward;
            let exploration = exploration_weight
                * ((total_observations.ln() + 1.0) / (stats.pulls.max(1) as f64)).sqrt();

            let larger_chunk_batch = profile.target_chunk_batch_size > current.chunk_batch_size;
            let larger_file_batch = profile.target_file_vectorization_batch_size
                > current.file_vectorization_batch_size;
            let larger_ready_depth =
                profile.target_ready_queue_depth > current.vector_ready_queue_depth;
            let larger_micro_items =
                profile.target_embed_micro_batch_max_items > current.embed_micro_batch_max_items;
            let larger_micro_tokens = profile.target_embed_micro_batch_max_total_tokens
                > current.embed_micro_batch_max_total_tokens;

            let backlog_bonus = if signals.file_vectorization_queue_depth >= 32 {
                let mut bonus = 0.0;
                if larger_chunk_batch {
                    bonus += backlog_weight;
                }
                if larger_file_batch {
                    bonus += backlog_weight * 0.5;
                }
                if larger_ready_depth {
                    bonus += backlog_weight * 0.75;
                }
                if larger_micro_items {
                    bonus += backlog_weight * 0.5;
                }
                if larger_micro_tokens {
                    bonus += backlog_weight * 0.5;
                }
                bonus
            } else {
                0.0
            };

            let anchor_fill_bonus = if signals.file_vectorization_queue_depth >= 256
                && signals.ready_queue_depth_current == 0
                && profile.label.starts_with("anchor_")
            {
                let mut bonus = anchor_fill_bonus_weight;
                if profile.label == "anchor_gpu_fill" {
                    bonus += anchor_fill_bonus_weight * 0.5;
                } else if profile.label == "anchor_backlog_surge"
                    && signals.file_vectorization_queue_depth >= 4_096
                {
                    bonus += anchor_fill_bonus_weight * 0.75;
                }
                bonus
            } else {
                0.0
            };

            let gpu_idle_penalty = if signals.file_vectorization_queue_depth >= 32
                && signals.ready_queue_depth_current == 0
                && !larger_ready_depth
                && !larger_file_batch
            {
                gpu_idle_penalty_weight
            } else {
                0.0
            };

            let no_progress_penalty = if stats.pulls > 0
                && stats.last_throughput_chunks_per_hour <= 0.0
                && !larger_chunk_batch
                && !larger_file_batch
                && !larger_ready_depth
            {
                no_progress_penalty_weight
            } else {
                0.0
            };

            let score = exploitation + exploration + backlog_bonus + anchor_fill_bonus
                - gpu_idle_penalty
                - no_progress_penalty;
            let reason = format!(
                "live_bandit:mean={:.2},ucb={:.2},backlog={:.2},anchor={:.2},pulls={}",
                exploitation, exploration, backlog_bonus, anchor_fill_bonus, stats.pulls
            );

            if best
                .as_ref()
                .map(|(_, best_score, _)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((profile.clone(), score, reason));
            }
        }

        let (profile, score, reason) = best?;
        Some(OptimizerDecision {
            decision_id: format!("opt-{}", now_ms),
            proposed_at_ms: now_ms,
            action_profile_id: profile.id,
            decision_reason: reason,
            score,
            confidence: 0.8,
            evaluation_window_start_ms: eval_window_start_ms,
            evaluation_window_end_ms: eval_window_end_ms,
        })
    }
}

pub fn collect_host_snapshot() -> HostSnapshot {
    let profile = RuntimeProfile::detect();
    let gpu = current_gpu_memory_snapshot().unwrap_or(crate::embedder::GpuMemorySnapshot {
        total_mb: 0,
        used_mb: 0,
        free_mb: 0,
    });
    let provider = current_embedding_provider_diagnostics();
    HostSnapshot {
        captured_at_ms: now_ms(),
        source: "optimizer.host.detect".to_string(),
        platform: std::env::consts::OS.to_string(),
        is_wsl: detect_is_wsl(),
        cpu_cores: profile.cpu_cores,
        ram_total_bytes: profile.ram_total_gb.saturating_mul(1024 * 1024 * 1024),
        gpu_present: profile.gpu_present,
        gpu_name: if provider.provider_effective.eq_ignore_ascii_case("cuda") || gpu.total_mb > 0 {
            Some("nvidia".to_string())
        } else {
            None
        },
        vram_total_mb: gpu.total_mb,
        io_characteristics: "linux_procfs_sample".to_string(),
    }
}

pub fn collect_operator_policy_snapshot(host: &HostSnapshot) -> OperatorPolicySnapshot {
    let captured_at_ms = now_ms();
    let max_vram_used_mb =
        env_u64("AXON_OPT_MAX_VRAM_USED_MB", host.vram_total_mb).min(host.vram_total_mb.max(1));
    let max_vram_used_ratio = if host.vram_total_mb > 0 {
        (max_vram_used_mb as f64 / host.vram_total_mb as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    OperatorPolicySnapshot {
        captured_at_ms,
        max_cpu_ratio: env_f64("AXON_OPT_MAX_CPU_RATIO", 0.90).clamp(0.0, 1.0),
        min_ram_available_ratio: env_f64("AXON_OPT_MIN_RAM_AVAILABLE_RATIO", 0.33).clamp(0.0, 1.0),
        max_mcp_p95_ms: env_u64("AXON_OPT_MAX_MCP_P95_MS", 300),
        max_vram_used_ratio,
        max_vram_used_mb,
        max_io_wait_ratio: env_f64("AXON_OPT_MAX_IO_WAIT_RATIO", 0.20).clamp(0.0, 1.0),
        backlog_priority_weight: env_f64("AXON_OPT_BACKLOG_PRIORITY_WEIGHT", 1.0).max(0.0),
        interactive_priority_weight: env_f64("AXON_OPT_INTERACTIVE_PRIORITY_WEIGHT", 1.0).max(0.0),
        shadow_mode_enabled: env_bool("AXON_OPT_SHADOW_MODE_ENABLED", true),
        allowed_actuators: optimizer_allowed_actuators(),
        evaluation_window_ms: env_u64("AXON_OPT_EVALUATION_WINDOW_MS", 15_000).max(10_000),
    }
}

fn optimizer_allowed_actuators() -> Vec<String> {
    let default = || {
        vec![
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
            "vector_ready_queue_depth".to_string(),
            "vector_persist_queue_bound".to_string(),
            "vector_max_inflight_persists".to_string(),
            "embed_micro_batch_max_items".to_string(),
            "embed_micro_batch_max_total_tokens".to_string(),
        ]
    };
    match std::env::var("AXON_OPT_ALLOWED_ACTUATORS") {
        Ok(raw) => raw
            .split(',')
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .filter(|item| *item != "vector_workers")
            .map(|item| item.to_string())
            .collect::<Vec<_>>(),
        Err(_) => default(),
    }
}

pub fn collect_runtime_signals_window(store: &GraphStore) -> RuntimeSignalsWindow {
    let now_ms = now_ms();
    let memory = process_memory_snapshot();
    let duckdb_memory = duckdb_memory_snapshot(store);
    let gpu = current_gpu_memory_snapshot().unwrap_or(crate::embedder::GpuMemorySnapshot {
        total_mb: 0,
        used_mb: 0,
        free_mb: 0,
    });
    let gpu_utilization =
        current_gpu_utilization_snapshot().unwrap_or(crate::embedder::GpuUtilizationSnapshot {
            gpu_utilization_ratio: 0.0,
            memory_utilization_ratio: 0.0,
        });
    let vector_latency = service_guard::vector_runtime_latency_summaries();
    let vector_runtime = service_guard::vector_runtime_metrics();
    let (file_vectorization_queue_queued, _file_vectorization_queue_inflight) = store
        .fetch_file_vectorization_queue_counts()
        .unwrap_or((0, 0));
    let (vector_outbox_queued, vector_outbox_inflight) =
        store.fetch_vector_persist_outbox_counts().unwrap_or((0, 0));
    let (graph_projection_queue_queued, graph_projection_queue_inflight) = store
        .fetch_graph_projection_queue_counts()
        .unwrap_or((0, 0));
    let (cpu_usage_ratio, ram_available_ratio, io_wait_ratio) = read_host_pressure_ratios();
    let canonical_backlog_depth = file_vectorization_queue_queued
        .saturating_add(vector_runtime.active_claimed_current as usize)
        .saturating_add(vector_runtime.prepare_claimed_current as usize)
        .saturating_add(vector_runtime.ready_claimed_current as usize)
        .saturating_add(vector_runtime.persist_claimed_current as usize)
        .saturating_add(vector_outbox_queued)
        .saturating_add(vector_outbox_inflight);
    RuntimeSignalsWindow {
        window_start_ms: now_ms.saturating_sub(60_000),
        window_end_ms: now_ms,
        captured_at_ms: now_ms,
        source: "optimizer.runtime.window".to_string(),
        cpu_usage_ratio,
        ram_available_ratio,
        io_wait_ratio,
        process_memory: memory,
        duckdb_memory,
        vram_used_mb: gpu.used_mb,
        vram_free_mb: gpu.free_mb,
        gpu_utilization_ratio: gpu_utilization.gpu_utilization_ratio,
        gpu_memory_utilization_ratio: gpu_utilization.memory_utilization_ratio,
        file_vectorization_queue_depth: canonical_backlog_depth,
        graph_projection_queue_depth: graph_projection_queue_queued
            + graph_projection_queue_inflight,
        canonical_vector_backlog_depth: canonical_backlog_depth,
        ready_queue_depth_current: vector_runtime.ready_queue_depth_current,
        ready_queue_depth_max: vector_runtime.ready_queue_depth_max,
        active_claimed_current: vector_runtime.active_claimed_current,
        prepare_claimed_current: vector_runtime.prepare_claimed_current,
        ready_claimed_current: vector_runtime.ready_claimed_current,
        persist_queue_depth_current: vector_runtime.persist_queue_depth_current,
        persist_queue_depth_max: vector_runtime.persist_queue_depth_max,
        persist_claimed_current: vector_runtime.persist_claimed_current,
        prepare_inflight_current: vector_runtime.prepare_inflight_current,
        prepare_inflight_max: vector_runtime.prepare_inflight_max,
        gpu_idle_wait_ms_total: vector_runtime.gpu_idle_wait_ms_total,
        prepare_queue_wait_ms_total: vector_runtime.prepare_queue_wait_ms_total,
        prepare_reply_wait_ms_total: vector_runtime.prepare_reply_wait_ms_total,
        persist_queue_wait_ms_total: vector_runtime.persist_queue_wait_ms_total,
        oldest_ready_batch_age_ms_current: vector_runtime.oldest_ready_batch_age_ms_current,
        oldest_ready_batch_age_ms_max: vector_runtime.oldest_ready_batch_age_ms_max,
        latency_recent_fetch_p95_ms: vector_latency.fetch.p95_ms,
        latency_recent_embed_p95_ms: vector_latency.embed.p95_ms,
        latency_recent_db_write_p95_ms: vector_latency.db_write.p95_ms,
        latency_recent_mark_done_p95_ms: vector_latency.mark_done.p95_ms,
        mcp_latency_recent_ms: service_guard::recent_mcp_latency_ms(),
        vector_workers_active_current: vector_runtime.vector_workers_active_current,
        vector_worker_heartbeat_at_ms: vector_runtime.vector_worker_heartbeat_at_ms,
        embed_inflight_started_at_ms: vector_runtime.embed_inflight_started_at_ms,
        interactive_requests_in_flight: service_guard::interactive_requests_in_flight(),
        interactive_priority: service_guard::current_interactive_priority()
            .as_str()
            .to_string(),
        chunk_embedding_writes_total: vector_runtime.chunks_embedded_total,
        files_completed_total: vector_runtime.files_completed_total,
        canonical_chunk_embeddings_total: canonical_count(
            store,
            &format!(
                "SELECT COUNT(*) FROM ChunkEmbedding WHERE model_id = '{}'",
                CHUNK_MODEL_ID.replace('\'', "''")
            ),
        ) as u64,
        canonical_files_embedded_total: canonical_count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT c.file_path) \
             FROM ChunkEmbedding ce \
             JOIN Chunk c ON c.id = ce.chunk_id \
             WHERE ce.model_id = '{}'",
                CHUNK_MODEL_ID.replace('\'', "''")
            ),
        ) as u64,
        canonical_chunks_embedded_last_minute: canonical_count(
            store,
            &format!(
                "SELECT COUNT(*) FROM ChunkEmbedding \
             WHERE model_id = '{}' \
               AND COALESCE(embedded_at_ms, 0) >= {}",
                CHUNK_MODEL_ID.replace('\'', "''"),
                now_ms.saturating_sub(60_000)
            ),
        ) as u64,
        canonical_files_embedded_last_minute: canonical_count(
            store,
            &format!(
                "SELECT COUNT(DISTINCT c.file_path) \
             FROM ChunkEmbedding ce \
             JOIN Chunk c ON c.id = ce.chunk_id \
             WHERE ce.model_id = '{}' \
               AND COALESCE(ce.embedded_at_ms, 0) >= {}",
                CHUNK_MODEL_ID.replace('\'', "''"),
                now_ms.saturating_sub(60_000)
            ),
        ) as u64,
    }
}

pub fn collect_recent_analytics_window(store: &GraphStore) -> RecentAnalyticsWindow {
    let now_ms = now_ms();
    let bucket_start_ms = (now_ms / 3_600_000) * 3_600_000;
    let query = format!(
        "SELECT COALESCE(sum(chunks_embedded), 0), \
                COALESCE(sum(files_vector_ready), 0), \
                COALESCE(sum(batches), 0), \
                COALESCE(sum(embed_ms_total), 0), \
                COALESCE(sum(db_write_ms_total), 0), \
                COALESCE(sum(mark_done_ms_total), 0) \
         FROM HourlyVectorizationRollup \
         WHERE bucket_start_ms = {} \
           AND model_id = '{}'",
        bucket_start_ms,
        CHUNK_MODEL_ID.replace('\'', "''")
    );
    let raw = store
        .query_json(&query)
        .unwrap_or_else(|_| "[]".to_string());
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let row = rows.first().cloned().unwrap_or_default();
    RecentAnalyticsWindow {
        collected_at_ms: now_ms,
        current_hour_bucket_start_ms: bucket_start_ms,
        chunks_embedded_current_hour: value_u64(row.first()),
        files_vector_ready_current_hour: value_u64(row.get(1)),
        batches_current_hour: value_u64(row.get(2)),
        embed_ms_total_current_hour: value_u64(row.get(3)),
        db_write_ms_total_current_hour: value_u64(row.get(4)),
        mark_done_ms_total_current_hour: value_u64(row.get(5)),
    }
}

pub fn build_admissible_action_profiles(
    host: &HostSnapshot,
    signals: &RuntimeSignalsWindow,
    policy: &OperatorPolicySnapshot,
) -> Vec<ActionProfile> {
    let current = current_runtime_tuning_state();
    let mut profiles = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |label: &str, state: RuntimeTuningState| {
        let state = normalize_runtime_tuning_state(state);
        let key = (
            state.vector_workers,
            state.chunk_batch_size,
            state.file_vectorization_batch_size,
            state.vector_ready_queue_depth,
            state.vector_persist_queue_bound,
            state.vector_max_inflight_persists,
            state.embed_micro_batch_max_items,
            state.embed_micro_batch_max_total_tokens,
        );
        if !seen.insert(key) {
            return;
        }
        profiles.push(ActionProfile {
            id: runtime_tuning_profile_id(&state),
            label: label.to_string(),
            target_vector_workers: state.vector_workers,
            target_chunk_batch_size: state.chunk_batch_size,
            target_file_vectorization_batch_size: state.file_vectorization_batch_size,
            target_ready_queue_depth: state.vector_ready_queue_depth,
            target_persist_queue_bound: state.vector_persist_queue_bound,
            target_max_inflight_persists: state.vector_max_inflight_persists,
            target_embed_micro_batch_max_items: state.embed_micro_batch_max_items,
            target_embed_micro_batch_max_total_tokens: state.embed_micro_batch_max_total_tokens,
        });
    };

    push("hold", current);

    let system_under_pressure = signals.ram_available_ratio < policy.min_ram_available_ratio
        || signals.vram_used_mb > policy.max_vram_used_mb;

    for neighbor in runtime_tuning_neighbors(current, policy.allowed_actuators.as_slice()) {
        if !host.gpu_present && neighbor.chunk_batch_size > current.chunk_batch_size {
            continue;
        }
        if signals.file_vectorization_queue_depth > 0
            && neighbor.chunk_batch_size
                > signals
                    .file_vectorization_queue_depth
                    .saturating_mul(4)
                    .max(64)
        {
            continue;
        }
        if system_under_pressure && profile_increases_runtime_pressure(current, neighbor) {
            continue;
        }
        push("neighbor", neighbor);
    }

    if !system_under_pressure && signals.file_vectorization_queue_depth >= 256 {
        for (label, anchor) in aggressive_embedding_anchor_profiles(
            current,
            signals,
            policy.allowed_actuators.as_slice(),
        ) {
            push(label, anchor);
        }
    }

    profiles
}

fn aggressive_embedding_anchor_profiles(
    current: RuntimeTuningState,
    signals: &RuntimeSignalsWindow,
    allowed_actuators: &[String],
) -> Vec<(&'static str, RuntimeTuningState)> {
    let allows = |name: &str| allowed_actuators.iter().any(|item| item == name);
    let backlog_scale = signals.file_vectorization_queue_depth;
    let mut profiles = Vec::new();

    let mut push_anchor = |label: &'static str,
                           chunk_batch_size: usize,
                           file_batch_size: usize,
                           ready_depth: usize,
                           persist_bound: usize,
                           inflight_persists: usize,
                           micro_items: usize,
                           micro_tokens: usize| {
        let mut candidate = current;
        if allows("chunk_batch_size") {
            candidate.chunk_batch_size = chunk_batch_size;
        }
        if allows("file_vectorization_batch_size") {
            candidate.file_vectorization_batch_size = file_batch_size;
        }
        if allows("vector_ready_queue_depth") {
            candidate.vector_ready_queue_depth = ready_depth;
        }
        if allows("vector_persist_queue_bound") {
            candidate.vector_persist_queue_bound = persist_bound;
        }
        if allows("vector_max_inflight_persists") {
            candidate.vector_max_inflight_persists = inflight_persists;
        }
        if allows("embed_micro_batch_max_items") {
            candidate.embed_micro_batch_max_items = micro_items;
        }
        if allows("embed_micro_batch_max_total_tokens") {
            candidate.embed_micro_batch_max_total_tokens = micro_tokens;
        }
        profiles.push((label, normalize_runtime_tuning_state(candidate)));
    };

    push_anchor(
        "anchor_backlog_open",
        current.chunk_batch_size.max(96),
        current.file_vectorization_batch_size.max(16),
        current.vector_ready_queue_depth.max(8),
        current.vector_persist_queue_bound.max(3),
        current.vector_max_inflight_persists.max(3),
        current.embed_micro_batch_max_items.max(96),
        current.embed_micro_batch_max_total_tokens.max(16_384),
    );

    if backlog_scale >= 1_024 || signals.ready_queue_depth_current == 0 {
        push_anchor(
            "anchor_gpu_fill",
            current.chunk_batch_size.max(128),
            current.file_vectorization_batch_size.max(24),
            current.vector_ready_queue_depth.max(10),
            current.vector_persist_queue_bound.max(4),
            current.vector_max_inflight_persists.max(4),
            current.embed_micro_batch_max_items.max(128),
            current.embed_micro_batch_max_total_tokens.max(24_576),
        );
    }

    if backlog_scale >= 4_096 {
        push_anchor(
            "anchor_backlog_surge",
            current.chunk_batch_size.max(160),
            current.file_vectorization_batch_size.max(32),
            current.vector_ready_queue_depth.max(12),
            current.vector_persist_queue_bound.max(6),
            current.vector_max_inflight_persists.max(6),
            current.embed_micro_batch_max_items.max(160),
            current.embed_micro_batch_max_total_tokens.max(32_768),
        );
    }

    profiles
}

fn profile_increases_runtime_pressure(
    current: RuntimeTuningState,
    candidate: RuntimeTuningState,
) -> bool {
    candidate.chunk_batch_size > current.chunk_batch_size
        || candidate.file_vectorization_batch_size > current.file_vectorization_batch_size
        || candidate.vector_ready_queue_depth > current.vector_ready_queue_depth
        || candidate.vector_persist_queue_bound > current.vector_persist_queue_bound
        || candidate.vector_max_inflight_persists > current.vector_max_inflight_persists
        || candidate.embed_micro_batch_max_items > current.embed_micro_batch_max_items
        || candidate.embed_micro_batch_max_total_tokens > current.embed_micro_batch_max_total_tokens
}

pub fn observe_reward(
    decision_id: &str,
    previous: &RuntimeSignalsWindow,
    current: &RuntimeSignalsWindow,
    policy: &OperatorPolicySnapshot,
    churn_penalty: f64,
) -> RewardObservation {
    let throughput_chunks_per_hour = current.canonical_chunks_embedded_last_minute as f64 * 60.0;
    let throughput_files_per_hour = current.canonical_files_embedded_last_minute as f64 * 60.0;
    let previous_throughput_chunks_per_hour =
        previous.canonical_chunks_embedded_last_minute as f64 * 60.0;
    let warmup_gpu_underutilized_ratio =
        env_f64("AXON_OPT_GPU_UNDERUTILIZED_RATIO", 0.35).clamp(0.0, 1.0);
    let reward_cpu_penalty_scale = env_f64("AXON_OPT_REWARD_CPU_PENALTY_SCALE", 100.0);
    let reward_ram_penalty_scale = env_f64("AXON_OPT_REWARD_RAM_PENALTY_SCALE", 100.0);
    let reward_vram_penalty_divisor =
        env_f64("AXON_OPT_REWARD_VRAM_PENALTY_DIVISOR_MB", 32.0).max(1.0);
    let reward_mcp_penalty_divisor =
        env_f64("AXON_OPT_REWARD_MCP_PENALTY_DIVISOR_MS", 10.0).max(1.0);
    let reward_io_penalty_scale = env_f64("AXON_OPT_REWARD_IO_PENALTY_SCALE", 100.0);
    let reward_liveness_penalty = env_f64("AXON_OPT_REWARD_LIVENESS_PENALTY", 25.0);
    let reward_gpu_headroom_bonus = env_f64("AXON_OPT_REWARD_GPU_HEADROOM_BONUS", 5.0);
    let reward_chunks_throughput_weight =
        env_f64("AXON_OPT_REWARD_CHUNKS_THROUGHPUT_WEIGHT", 1.5).max(0.1);
    let reward_files_throughput_weight =
        env_f64("AXON_OPT_REWARD_FILES_THROUGHPUT_WEIGHT", 0.05).max(0.0);
    let reward_ready_queue_nonempty_bonus =
        env_f64("AXON_OPT_REWARD_READY_QUEUE_NONEMPTY_BONUS", 10.0);
    let reward_underfed_backlog_penalty = env_f64("AXON_OPT_REWARD_UNDERFED_BACKLOG_PENALTY", 15.0);
    let reward_stability_penalty_scale =
        env_f64("AXON_OPT_REWARD_STABILITY_PENALTY_SCALE", 20.0).max(0.0);
    let warmup_active = current.file_vectorization_queue_depth > 0
        && current.canonical_chunks_embedded_last_minute == 0
        && current.gpu_utilization_ratio < warmup_gpu_underutilized_ratio;

    let penalty_cpu = if !warmup_active && current.cpu_usage_ratio > policy.max_cpu_ratio {
        (current.cpu_usage_ratio - policy.max_cpu_ratio) * reward_cpu_penalty_scale
    } else {
        0.0
    };
    let penalty_ram = if current.ram_available_ratio < policy.min_ram_available_ratio {
        (policy.min_ram_available_ratio - current.ram_available_ratio) * reward_ram_penalty_scale
    } else {
        0.0
    };
    let penalty_vram = if current.vram_used_mb > policy.max_vram_used_mb {
        (current.vram_used_mb - policy.max_vram_used_mb) as f64 / reward_vram_penalty_divisor
    } else {
        0.0
    };
    let penalty_mcp = if current.mcp_latency_recent_ms > policy.max_mcp_p95_ms {
        (current.mcp_latency_recent_ms - policy.max_mcp_p95_ms) as f64 / reward_mcp_penalty_divisor
    } else {
        0.0
    };
    let penalty_io = if current.io_wait_ratio > policy.max_io_wait_ratio {
        (current.io_wait_ratio - policy.max_io_wait_ratio) * reward_io_penalty_scale
    } else {
        0.0
    };
    let penalty_liveness = if current.vector_workers_active_current == 0 {
        reward_liveness_penalty
    } else {
        0.0
    };
    let penalty_stability = if previous_throughput_chunks_per_hour > 0.0
        && throughput_chunks_per_hour < previous_throughput_chunks_per_hour
    {
        ((previous_throughput_chunks_per_hour - throughput_chunks_per_hour)
            / previous_throughput_chunks_per_hour)
            * reward_stability_penalty_scale
    } else {
        0.0
    };
    let gpu_starvation_bonus = if current.file_vectorization_queue_depth >= 32
        && current.gpu_utilization_ratio >= 0.45
        && current.vram_used_mb <= policy.max_vram_used_mb
    {
        reward_gpu_headroom_bonus
    } else {
        0.0
    };
    let ready_queue_continuity_bonus =
        if current.file_vectorization_queue_depth >= 32 && current.ready_queue_depth_current > 0 {
            reward_ready_queue_nonempty_bonus
        } else {
            0.0
        };
    let underfed_backlog_penalty = if current.file_vectorization_queue_depth >= 32
        && current.ready_queue_depth_current == 0
        && current.gpu_utilization_ratio < warmup_gpu_underutilized_ratio
    {
        reward_underfed_backlog_penalty
    } else {
        0.0
    };
    let reward = throughput_chunks_per_hour * reward_chunks_throughput_weight
        + throughput_files_per_hour * reward_files_throughput_weight
        + gpu_starvation_bonus
        + ready_queue_continuity_bonus
        - penalty_cpu
        - penalty_ram
        - penalty_vram
        - penalty_mcp
        - penalty_io
        - penalty_liveness
        - penalty_stability
        - underfed_backlog_penalty
        - churn_penalty;

    RewardObservation {
        decision_id: decision_id.to_string(),
        observed_at_ms: now_ms(),
        window_start_ms: previous.window_end_ms,
        window_end_ms: current.window_end_ms,
        throughput_chunks_per_hour,
        throughput_files_per_hour,
        reward,
        penalty_cpu,
        penalty_ram,
        penalty_vram,
        penalty_mcp,
        penalty_io,
        penalty_liveness,
        penalty_stability,
        penalty_churn: churn_penalty,
    }
}

fn runtime_tuning_profile_id(state: &RuntimeTuningState) -> String {
    format!(
        "vw{}-cb{}-fb{}-rq{}-pq{}-ip{}-mi{}-mt{}",
        state.vector_workers,
        state.chunk_batch_size,
        state.file_vectorization_batch_size,
        state.vector_ready_queue_depth,
        state.vector_persist_queue_bound,
        state.vector_max_inflight_persists,
        state.embed_micro_batch_max_items,
        state.embed_micro_batch_max_total_tokens
    )
}

fn normalize_runtime_tuning_state(mut state: RuntimeTuningState) -> RuntimeTuningState {
    state.vector_workers = state.vector_workers.max(1);
    state.chunk_batch_size = state.chunk_batch_size.clamp(16, 256);
    state.file_vectorization_batch_size = state.file_vectorization_batch_size.clamp(4, 64);
    state.vector_ready_queue_depth = state.vector_ready_queue_depth.clamp(1, 32);
    state.vector_persist_queue_bound = state.vector_persist_queue_bound.clamp(1, 12);
    state.vector_max_inflight_persists = state
        .vector_max_inflight_persists
        .clamp(1, state.vector_persist_queue_bound);
    state.embed_micro_batch_max_items = state.embed_micro_batch_max_items.clamp(8, 256);
    state.embed_micro_batch_max_total_tokens =
        state.embed_micro_batch_max_total_tokens.clamp(512, 65_536);
    state
}

fn mutate_step(value: usize, increase: bool) -> usize {
    let delta = ((value as f64) * 0.10).ceil() as usize;
    if increase {
        value.saturating_add(delta.max(1))
    } else {
        value.saturating_sub(delta.max(1))
    }
}

fn runtime_tuning_neighbors(
    current: RuntimeTuningState,
    allowed_actuators: &[String],
) -> Vec<RuntimeTuningState> {
    let mut neighbors = Vec::new();
    let mut emit =
        |candidate: RuntimeTuningState| neighbors.push(normalize_runtime_tuning_state(candidate));
    let allows = |name: &str| allowed_actuators.iter().any(|item| item == name);

    for increase in [false, true] {
        if allows("chunk_batch_size") {
            let mut candidate = current;
            candidate.chunk_batch_size = mutate_step(current.chunk_batch_size, increase);
            emit(candidate);
        }
        if allows("file_vectorization_batch_size") {
            let mut candidate = current;
            candidate.file_vectorization_batch_size =
                mutate_step(current.file_vectorization_batch_size, increase);
            emit(candidate);
        }
        if allows("vector_ready_queue_depth") {
            let mut candidate = current;
            candidate.vector_ready_queue_depth =
                mutate_step(current.vector_ready_queue_depth, increase);
            emit(candidate);
        }
        if allows("vector_persist_queue_bound") {
            let mut candidate = current;
            candidate.vector_persist_queue_bound =
                mutate_step(current.vector_persist_queue_bound, increase);
            emit(candidate);
        }
        if allows("vector_max_inflight_persists") {
            let mut candidate = current;
            candidate.vector_max_inflight_persists =
                mutate_step(current.vector_max_inflight_persists, increase);
            emit(candidate);
        }
        if allows("embed_micro_batch_max_items") {
            let mut candidate = current;
            candidate.embed_micro_batch_max_items =
                mutate_step(current.embed_micro_batch_max_items, increase);
            emit(candidate);
        }
        if allows("embed_micro_batch_max_total_tokens") {
            let mut candidate = current;
            candidate.embed_micro_batch_max_total_tokens =
                mutate_step(current.embed_micro_batch_max_total_tokens, increase);
            emit(candidate);
        }
    }

    neighbors
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|raw| {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn detect_is_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|raw| raw.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

fn value_u64(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(|value| value.as_u64())
        .or_else(|| {
            value
                .and_then(|value| value.as_i64())
                .map(|v| v.max(0) as u64)
        })
        .or_else(|| {
            value
                .and_then(|value| value.as_str())
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn canonical_count(store: &GraphStore, query: &str) -> i64 {
    let raw = match store.query_json(query) {
        Ok(raw) => raw,
        Err(_) => return 0,
    };

    if let Ok(rows) = serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&raw) {
        if let Some(value) = rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
        {
            return value.max(0);
        }
    }

    if let Ok(rows) = serde_json::from_str::<Vec<serde_json::Map<String, serde_json::Value>>>(&raw)
    {
        for row in rows {
            if let Some(value) = row
                .get("count(*)")
                .or_else(|| row.get("count_star()"))
                .or_else(|| row.get("count"))
                .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
            {
                return value.max(0);
            }
            if let Some(value) = row
                .values()
                .next()
                .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|v| v as i64)))
            {
                return value.max(0);
            }
        }
    }

    0
}

fn read_host_pressure_ratios() -> (f64, f64, f64) {
    let (cpu_usage_ratio, io_wait_ratio) = read_cpu_and_io_usage_ratios();
    let ram_available_ratio = read_ram_available_ratio().clamp(0.0, 1.0);
    (cpu_usage_ratio, ram_available_ratio, io_wait_ratio)
}

#[derive(Debug, Clone, Copy)]
struct ProcStatSample {
    total: u64,
    idle: u64,
    iowait: u64,
}

static HOST_PRESSURE_SAMPLER: OnceLock<Mutex<Option<ProcStatSample>>> = OnceLock::new();

fn host_pressure_sampler() -> &'static Mutex<Option<ProcStatSample>> {
    HOST_PRESSURE_SAMPLER.get_or_init(|| Mutex::new(None))
}

fn read_cpu_and_io_usage_ratios() -> (f64, f64) {
    let current = read_proc_stat_sample();
    let Some(current) = current else {
        return (0.0, 0.0);
    };
    let sampler = host_pressure_sampler();
    let previous = {
        let mut guard = sampler.lock().unwrap_or_else(|poison| poison.into_inner());
        let previous = *guard;
        *guard = Some(current);
        previous
    };
    previous
        .map(|previous| compute_cpu_and_io_usage_ratios(previous, current))
        .unwrap_or((0.0, 0.0))
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
    Some(ProcStatSample {
        total: user + nice + system + idle + iowait + irq + softirq + steal,
        idle,
        iowait,
    })
}

fn compute_cpu_and_io_usage_ratios(
    previous: ProcStatSample,
    current: ProcStatSample,
) -> (f64, f64) {
    let total_delta = current.total.saturating_sub(previous.total);
    if total_delta == 0 {
        return (0.0, 0.0);
    }
    let idle_delta = current.idle.saturating_sub(previous.idle);
    let iowait_delta = current.iowait.saturating_sub(previous.iowait);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    (
        ((busy_delta as f64) / (total_delta as f64)).clamp(0.0, 1.0),
        ((iowait_delta as f64) / (total_delta as f64)).clamp(0.0, 1.0),
    )
}

fn read_ram_available_ratio() -> f64 {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => content,
        Err(_) => return 0.0,
    };
    let mut total_kb = 0.0;
    let mut available_kb = 0.0;
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        match parts.next().unwrap_or_default() {
            "MemTotal:" => {
                total_kb = parts
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            }
            "MemAvailable:" => {
                available_kb = parts
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            }
            _ => {}
        }
    }
    if total_kb <= 0.0 {
        0.0
    } else {
        available_kb / total_kb
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_admissible_action_profiles, collect_operator_policy_snapshot,
        compute_cpu_and_io_usage_ratios, observe_reward, ActionProfile, HeuristicPolicyEngine,
        HostSnapshot, LiveBanditPolicyEngine, OperatorPolicySnapshot, PolicyEngine, ProcStatSample,
        RecentAnalyticsWindow, RuntimeSignalsWindow,
    };
    use crate::runtime_tuning::RuntimeTuningState;
    use crate::service_guard::InteractivePriority;

    fn host() -> HostSnapshot {
        HostSnapshot {
            captured_at_ms: 1,
            source: "test".to_string(),
            platform: "linux".to_string(),
            is_wsl: true,
            cpu_cores: 8,
            ram_total_bytes: 32 * 1024 * 1024 * 1024,
            gpu_present: true,
            gpu_name: Some("rtx".to_string()),
            vram_total_mb: 8192,
            io_characteristics: "test".to_string(),
        }
    }

    fn signals() -> RuntimeSignalsWindow {
        RuntimeSignalsWindow {
            window_start_ms: 0,
            window_end_ms: 60_000,
            captured_at_ms: 60_000,
            source: "test".to_string(),
            cpu_usage_ratio: 0.2,
            ram_available_ratio: 0.5,
            io_wait_ratio: 0.01,
            process_memory: Default::default(),
            duckdb_memory: Default::default(),
            vram_used_mb: 1024,
            vram_free_mb: 7168,
            gpu_utilization_ratio: 0.2,
            gpu_memory_utilization_ratio: 0.1,
            file_vectorization_queue_depth: 128,
            graph_projection_queue_depth: 4,
            canonical_vector_backlog_depth: 128,
            active_claimed_current: 0,
            prepare_claimed_current: 0,
            ready_claimed_current: 0,
            latency_recent_fetch_p95_ms: 10,
            latency_recent_embed_p95_ms: 25,
            latency_recent_db_write_p95_ms: 5,
            latency_recent_mark_done_p95_ms: 5,
            mcp_latency_recent_ms: 40,
            vector_workers_active_current: 1,
            vector_worker_heartbeat_at_ms: 59_000,
            embed_inflight_started_at_ms: 0,
            ready_queue_depth_current: 0,
            ready_queue_depth_max: 0,
            prepare_inflight_current: 0,
            prepare_inflight_max: 0,
            persist_queue_depth_current: 0,
            persist_queue_depth_max: 0,
            persist_claimed_current: 0,
            gpu_idle_wait_ms_total: 0,
            prepare_queue_wait_ms_total: 0,
            prepare_reply_wait_ms_total: 0,
            persist_queue_wait_ms_total: 0,
            oldest_ready_batch_age_ms_current: 0,
            oldest_ready_batch_age_ms_max: 0,
            interactive_requests_in_flight: 0,
            interactive_priority: InteractivePriority::BackgroundNormal.as_str().to_string(),
            chunk_embedding_writes_total: 100,
            files_completed_total: 10,
            canonical_chunk_embeddings_total: 100,
            canonical_files_embedded_total: 10,
            canonical_chunks_embedded_last_minute: 100,
            canonical_files_embedded_last_minute: 10,
        }
    }

    fn policy() -> OperatorPolicySnapshot {
        OperatorPolicySnapshot {
            captured_at_ms: 1,
            max_cpu_ratio: 0.5,
            min_ram_available_ratio: 0.33,
            max_mcp_p95_ms: 300,
            max_vram_used_ratio: 0.75,
            max_vram_used_mb: 6144,
            max_io_wait_ratio: 0.2,
            backlog_priority_weight: 1.0,
            interactive_priority_weight: 1.0,
            shadow_mode_enabled: true,
            allowed_actuators: vec![
                "vector_workers".to_string(),
                "chunk_batch_size".to_string(),
                "file_vectorization_batch_size".to_string(),
            ],
            evaluation_window_ms: 60_000,
        }
    }

    #[test]
    fn collect_operator_policy_caps_vram_to_host_limit() {
        let mut host = host();
        host.vram_total_mb = 2048;
        std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "9999");
        let policy = collect_operator_policy_snapshot(&host);
        assert_eq!(policy.max_vram_used_mb, 2048);
        std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
    }

    #[test]
    fn collect_operator_policy_defaults_live_actuators_to_runtime_safe_batch_controls() {
        std::env::remove_var("AXON_OPT_ALLOWED_ACTUATORS");
        let policy = collect_operator_policy_snapshot(&host());
        assert_eq!(
            policy.allowed_actuators,
            vec![
                "chunk_batch_size".to_string(),
                "file_vectorization_batch_size".to_string(),
                "vector_ready_queue_depth".to_string(),
                "vector_persist_queue_bound".to_string(),
                "vector_max_inflight_persists".to_string(),
                "embed_micro_batch_max_items".to_string(),
                "embed_micro_batch_max_total_tokens".to_string(),
            ]
        );
    }

    #[test]
    fn collect_operator_policy_defaults_to_short_live_evaluation_window() {
        std::env::remove_var("AXON_OPT_EVALUATION_WINDOW_MS");
        let policy = collect_operator_policy_snapshot(&host());
        assert_eq!(policy.evaluation_window_ms, 15_000);
    }

    #[test]
    fn collect_operator_policy_accepts_configured_allowed_actuators() {
        std::env::set_var(
            "AXON_OPT_ALLOWED_ACTUATORS",
            "chunk_batch_size,file_vectorization_batch_size,vector_workers",
        );
        let policy = collect_operator_policy_snapshot(&host());
        assert!(policy
            .allowed_actuators
            .iter()
            .any(|item| item == "chunk_batch_size"));
        assert!(policy
            .allowed_actuators
            .iter()
            .any(|item| item == "file_vectorization_batch_size"));
        assert!(policy
            .allowed_actuators
            .iter()
            .all(|item| item != "vector_workers"));
        std::env::remove_var("AXON_OPT_ALLOWED_ACTUATORS");
    }

    #[test]
    fn collect_operator_policy_fails_closed_when_allowlist_filters_to_empty() {
        std::env::set_var("AXON_OPT_ALLOWED_ACTUATORS", "vector_workers");
        let policy = collect_operator_policy_snapshot(&host());
        assert!(policy.allowed_actuators.is_empty());
        std::env::remove_var("AXON_OPT_ALLOWED_ACTUATORS");
    }

    #[test]
    fn heuristic_policy_returns_a_decision_from_admissible_profiles() {
        let action_profiles = vec![
            ActionProfile {
                id: "hold".to_string(),
                label: "hold".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 32,
                target_file_vectorization_batch_size: 8,
                target_ready_queue_depth: 6,
                target_persist_queue_bound: 1,
                target_max_inflight_persists: 1,
                target_embed_micro_batch_max_items: 32,
                target_embed_micro_batch_max_total_tokens: 8_192,
            },
            ActionProfile {
                id: "deepen".to_string(),
                label: "deepen".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 48,
                target_file_vectorization_batch_size: 12,
                target_ready_queue_depth: 7,
                target_persist_queue_bound: 2,
                target_max_inflight_persists: 2,
                target_embed_micro_batch_max_items: 40,
                target_embed_micro_batch_max_total_tokens: 9_216,
            },
        ];
        let decision = HeuristicPolicyEngine
            .choose(
                &host(),
                &signals(),
                &policy(),
                &RecentAnalyticsWindow {
                    chunks_embedded_current_hour: 1000,
                    ..Default::default()
                },
                &action_profiles,
            )
            .unwrap();
        assert!(action_profiles
            .iter()
            .any(|profile| profile.id == decision.action_profile_id));
    }

    #[test]
    fn compute_cpu_and_io_usage_ratios_uses_host_level_window() {
        let previous = ProcStatSample {
            total: 1_000,
            idle: 400,
            iowait: 40,
        };
        let current = ProcStatSample {
            total: 1_400,
            idle: 500,
            iowait: 60,
        };
        let (cpu, io) = compute_cpu_and_io_usage_ratios(previous, current);
        assert!((cpu - 0.75_f64).abs() < 1e-9);
        assert!((io - 0.05_f64).abs() < 1e-9);
    }

    #[test]
    fn build_admissible_action_profiles_generates_local_neighbors() {
        let mut signals = signals();
        signals.file_vectorization_queue_depth = 256;
        std::env::set_var(
            "AXON_OPT_ALLOWED_ACTUATORS",
            "chunk_batch_size,file_vectorization_batch_size,vector_ready_queue_depth",
        );
        let profiles = build_admissible_action_profiles(&host(), &signals, &policy());
        let hold = profiles
            .iter()
            .find(|profile| profile.label == "hold")
            .expect("hold profile");
        let neighbors = profiles
            .iter()
            .filter(|profile| profile.label == "neighbor")
            .collect::<Vec<_>>();
        assert!(!neighbors.is_empty());
        assert!(neighbors
            .iter()
            .all(|profile| profile.target_vector_workers == 1));
        assert!(neighbors.iter().all(|profile| {
            let mut deltas = 0;
            deltas += usize::from(profile.target_chunk_batch_size != hold.target_chunk_batch_size);
            deltas += usize::from(
                profile.target_file_vectorization_batch_size
                    != hold.target_file_vectorization_batch_size,
            );
            deltas +=
                usize::from(profile.target_ready_queue_depth != hold.target_ready_queue_depth);
            deltas == 1
        }));
        std::env::remove_var("AXON_OPT_ALLOWED_ACTUATORS");
    }

    #[test]
    fn build_admissible_action_profiles_includes_aggressive_embedding_anchors_under_backlog() {
        let mut current_signals = signals();
        current_signals.file_vectorization_queue_depth = 8_192;
        current_signals.ready_queue_depth_current = 0;
        let mut current_policy = policy();
        current_policy.allowed_actuators = vec![
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
            "vector_ready_queue_depth".to_string(),
            "vector_persist_queue_bound".to_string(),
            "vector_max_inflight_persists".to_string(),
            "embed_micro_batch_max_items".to_string(),
            "embed_micro_batch_max_total_tokens".to_string(),
        ];
        let profiles = build_admissible_action_profiles(&host(), &current_signals, &current_policy);
        assert!(profiles
            .iter()
            .any(|profile| profile.label == "anchor_backlog_open"));
        assert!(profiles
            .iter()
            .any(|profile| profile.label == "anchor_gpu_fill"));
        assert!(profiles
            .iter()
            .any(|profile| profile.label == "anchor_backlog_surge"));
    }

    #[test]
    fn live_bandit_prefers_anchor_profiles_when_backlog_is_large_and_ready_queue_is_empty() {
        let mut current_signals = signals();
        current_signals.file_vectorization_queue_depth = 8_192;
        current_signals.ready_queue_depth_current = 0;
        let mut current_policy = policy();
        current_policy.allowed_actuators = vec![
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
            "vector_ready_queue_depth".to_string(),
            "vector_persist_queue_bound".to_string(),
            "vector_max_inflight_persists".to_string(),
            "embed_micro_batch_max_items".to_string(),
            "embed_micro_batch_max_total_tokens".to_string(),
        ];
        let action_profiles =
            build_admissible_action_profiles(&host(), &current_signals, &current_policy);
        let decision = LiveBanditPolicyEngine
            .choose(
                &host(),
                &current_signals,
                &current_policy,
                &RecentAnalyticsWindow::default(),
                &action_profiles,
            )
            .expect("bandit decision");
        assert!(
            decision.action_profile_id.contains("cb128")
                || decision.action_profile_id.contains("cb160")
        );
    }

    #[test]
    fn observe_reward_does_not_penalize_cpu_during_gpu_warmup() {
        let previous = signals();
        let mut current = signals();
        current.cpu_usage_ratio = 0.90;
        current.file_vectorization_queue_depth = 512;
        current.canonical_chunks_embedded_last_minute = 0;
        current.gpu_utilization_ratio = 0.10;
        let observation = observe_reward("test", &previous, &current, &policy(), 0.0);
        assert_eq!(observation.penalty_cpu, 0.0);
    }

    #[test]
    fn reward_observation_penalizes_constraint_violations() {
        let previous = signals();
        let mut current = signals();
        current.window_end_ms = 120_000;
        current.canonical_chunks_embedded_last_minute = 200;
        current.cpu_usage_ratio = 0.9;
        let obs = observe_reward("d1", &previous, &current, &policy(), 0.0);
        assert!(obs.penalty_cpu > 0.0);
        assert!(obs.reward < obs.throughput_chunks_per_hour * 1.5);
    }

    #[test]
    fn reward_observation_prefers_chunk_throughput_and_ready_queue_continuity() {
        let previous = signals();
        let mut current = signals();
        current.window_end_ms = 120_000;
        current.canonical_chunks_embedded_last_minute = 220;
        current.canonical_files_embedded_last_minute = 12;
        current.ready_queue_depth_current = 4;
        current.gpu_utilization_ratio = 0.55;
        let obs = observe_reward("d2", &previous, &current, &policy(), 0.0);
        assert!(obs.reward > obs.throughput_chunks_per_hour);
    }

    #[test]
    fn reward_observation_penalizes_underfed_gpu_when_backlog_exists() {
        let previous = signals();
        let mut underfed = signals();
        underfed.window_end_ms = 120_000;
        underfed.canonical_chunks_embedded_last_minute = 1;
        underfed.ready_queue_depth_current = 0;
        underfed.gpu_utilization_ratio = 0.10;
        underfed.file_vectorization_queue_depth = 256;

        let mut fed = underfed.clone();
        fed.ready_queue_depth_current = 4;
        fed.gpu_utilization_ratio = 0.55;

        let underfed_obs = observe_reward("d3-underfed", &previous, &underfed, &policy(), 0.0);
        let fed_obs = observe_reward("d3-fed", &previous, &fed, &policy(), 0.0);
        assert!(underfed_obs.reward < fed_obs.reward);
    }

    #[test]
    fn reward_observation_penalizes_throughput_regression_across_windows() {
        let previous = signals();
        let mut current = signals();
        current.window_end_ms = 120_000;
        current.canonical_chunks_embedded_last_minute = 40;
        let obs = observe_reward("d4", &previous, &current, &policy(), 0.0);
        assert!(obs.penalty_stability > 0.0);
    }

    #[test]
    fn heuristic_policy_prefers_opening_embedding_throughput_under_backlog() {
        let action_profiles = vec![
            ActionProfile {
                id: "hold".to_string(),
                label: "hold".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 32,
                target_file_vectorization_batch_size: 8,
                target_ready_queue_depth: 6,
                target_persist_queue_bound: 1,
                target_max_inflight_persists: 1,
                target_embed_micro_batch_max_items: 32,
                target_embed_micro_batch_max_total_tokens: 8_192,
            },
            ActionProfile {
                id: "open-embed".to_string(),
                label: "neighbor".to_string(),
                target_vector_workers: 1,
                target_chunk_batch_size: 40,
                target_file_vectorization_batch_size: 10,
                target_ready_queue_depth: 6,
                target_persist_queue_bound: 1,
                target_max_inflight_persists: 1,
                target_embed_micro_batch_max_items: 40,
                target_embed_micro_batch_max_total_tokens: 9_216,
            },
        ];
        let decision = HeuristicPolicyEngine
            .choose(
                &host(),
                &signals(),
                &policy(),
                &RecentAnalyticsWindow {
                    chunks_embedded_current_hour: 100,
                    ..Default::default()
                },
                &action_profiles,
            )
            .unwrap();
        assert_eq!(decision.action_profile_id, "open-embed");
    }

    #[test]
    fn admissible_action_profiles_always_include_hold() {
        let profiles = build_admissible_action_profiles(&host(), &signals(), &policy());
        assert!(profiles.iter().any(|profile| profile.label == "hold"));
    }

    #[test]
    fn admissible_action_profiles_do_not_mutate_vector_workers_when_not_allowed() {
        let mut signals = signals();
        signals.file_vectorization_queue_depth = 256;
        let mut policy = policy();
        policy.allowed_actuators = vec![
            "chunk_batch_size".to_string(),
            "file_vectorization_batch_size".to_string(),
        ];
        let profiles = build_admissible_action_profiles(&host(), &signals, &policy);
        assert!(profiles
            .iter()
            .all(|profile| profile.target_vector_workers == 1));
    }

    #[test]
    fn admissible_action_profiles_hold_only_when_no_actuators_are_allowed() {
        let mut current_signals = signals();
        current_signals.file_vectorization_queue_depth = 256;
        let mut current_policy = policy();
        current_policy.allowed_actuators = vec![];
        let profiles = build_admissible_action_profiles(&host(), &current_signals, &current_policy);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].label, "hold");
    }

    #[test]
    fn admissible_action_profiles_do_not_block_more_aggressive_neighbors_on_cpu_pressure_alone() {
        let mut pressured_signals = signals();
        pressured_signals.file_vectorization_queue_depth = 256;
        pressured_signals.cpu_usage_ratio = 0.95;
        let profiles = build_admissible_action_profiles(&host(), &pressured_signals, &policy());
        let hold = profiles
            .iter()
            .find(|profile| profile.label == "hold")
            .expect("hold profile");
        assert!(profiles.iter().any(|profile| {
            profile.label != "hold"
                && super::profile_increases_runtime_pressure(
                    RuntimeTuningState {
                        vector_workers: hold.target_vector_workers,
                        chunk_batch_size: hold.target_chunk_batch_size,
                        file_vectorization_batch_size: hold.target_file_vectorization_batch_size,
                        vector_ready_queue_depth: hold.target_ready_queue_depth,
                        vector_persist_queue_bound: hold.target_persist_queue_bound,
                        vector_max_inflight_persists: hold.target_max_inflight_persists,
                        embed_micro_batch_max_items: hold.target_embed_micro_batch_max_items,
                        embed_micro_batch_max_total_tokens: hold
                            .target_embed_micro_batch_max_total_tokens,
                    },
                    RuntimeTuningState {
                        vector_workers: profile.target_vector_workers,
                        chunk_batch_size: profile.target_chunk_batch_size,
                        file_vectorization_batch_size: profile.target_file_vectorization_batch_size,
                        vector_ready_queue_depth: profile.target_ready_queue_depth,
                        vector_persist_queue_bound: profile.target_persist_queue_bound,
                        vector_max_inflight_persists: profile.target_max_inflight_persists,
                        embed_micro_batch_max_items: profile.target_embed_micro_batch_max_items,
                        embed_micro_batch_max_total_tokens: profile
                            .target_embed_micro_batch_max_total_tokens,
                    },
                )
        }));
    }

    #[test]
    fn runtime_tuning_neighbors_move_by_at_most_ten_percent_one_actuator_at_a_time() {
        let current = RuntimeTuningState {
            vector_workers: 1,
            chunk_batch_size: 64,
            file_vectorization_batch_size: 20,
            vector_ready_queue_depth: 6,
            vector_persist_queue_bound: 2,
            vector_max_inflight_persists: 2,
            embed_micro_batch_max_items: 80,
            embed_micro_batch_max_total_tokens: 8_192,
        };
        let allowed = vec![
            "chunk_batch_size".to_string(),
            "vector_ready_queue_depth".to_string(),
            "embed_micro_batch_max_total_tokens".to_string(),
        ];
        let neighbors = super::runtime_tuning_neighbors(current, &allowed);
        assert!(neighbors.iter().all(|candidate| {
            let mut deltas = 0;
            deltas += usize::from(candidate.chunk_batch_size != current.chunk_batch_size);
            deltas +=
                usize::from(candidate.vector_ready_queue_depth != current.vector_ready_queue_depth);
            deltas += usize::from(
                candidate.embed_micro_batch_max_total_tokens
                    != current.embed_micro_batch_max_total_tokens,
            );
            deltas == 1
        }));
        assert!(neighbors
            .iter()
            .any(|candidate| candidate.chunk_batch_size == 71));
        assert!(neighbors
            .iter()
            .any(|candidate| candidate.vector_ready_queue_depth == 7));
        assert!(neighbors
            .iter()
            .any(|candidate| candidate.embed_micro_batch_max_total_tokens == 9_012));
    }
}
