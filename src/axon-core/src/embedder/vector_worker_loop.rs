//! Vector worker loop — extracted from embedder.rs (REQ-AXO-080 Phase 6).
//!
//! Pure structural extraction of `vector_worker_loop` (the hot path GPU
//! consumer) and its tightly-coupled helper `build_vector_embedding_model`
//! from the embedder impl block to their own submodule. Behaviour-preserving
//! move; bodies verbatim. The helper is in this same file because the loop
//! is its only caller.
//!
//! All free helpers and types from the parent module reach this submodule
//! via `use super::*`.

use super::*;

pub(super) fn vector_worker_loop(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    refill_tx: Sender<VectorRefillCommand>,
    persist_tx: Sender<VectorPersistRequest>,
    finalize_tx: Sender<VectorFinalizeRequest>,
    ready_batches: Arc<SharedPreparedBatchQueue>,
) {
    let _liveness = VectorWorkerLivenessGuard::new();
    let mut restart_window: VecDeque<i64> = VecDeque::new();
    let mut restart_attempt = 0_u64;
    let mut gpu_recycle_candidate_batches = 0_u32;
    let mut recycle_coordinator = VramRecycleCoordinator::new();
    let mut wake_idle = true;

    if let Err(e) = graph_store.ensure_embedding_model(
        SYMBOL_MODEL_ID,
        "symbol",
        MODEL_NAME,
        DIMENSION as i64,
        MODEL_VERSION,
    ) {
        error!(
            "Semantic Worker: failed to register symbol embedding model: {:?}",
            e
        );
    }
    if let Err(e) = graph_store.ensure_embedding_model(
        CHUNK_MODEL_ID,
        "chunk",
        MODEL_NAME,
        DIMENSION as i64,
        MODEL_VERSION,
    ) {
        error!(
            "Semantic Worker: failed to register chunk embedding model: {:?}",
            e
        );
    }

    info!(
        "Semantic Vector Worker [{}]: Hunting for unembedded symbols and file chunks...",
        worker_idx
    );

    'worker_lifecycle: loop {
        persist_vector_lane_state(
            &graph_store,
            VectorLaneState::Starting,
            worker_idx,
            restart_attempt,
            Some("model_init".to_string()),
            None,
        );
        info!(
            "Semantic Vector Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
            worker_idx
        );
        let Some(mut model) = build_vector_embedding_model(worker_idx) else {
            let init_reason = std::env::var("AXON_EMBEDDING_PROVIDER_INIT_ERROR")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "failed to initialize embedding model".to_string());
            schedule_vector_worker_restart(
                &graph_store,
                worker_idx,
                FatalVectorWorkerFault {
                    stage: "model_init",
                    reason_raw: init_reason,
                    fatal_class: "model_init".to_string(),
                    batch_id: None,
                    texts_count: 0,
                    input_bytes: 0,
                },
                &mut restart_window,
                &mut restart_attempt,
            );
            continue;
        };
        persist_vector_lane_state(
            &graph_store,
            VectorLaneState::Healthy,
            worker_idx,
            restart_attempt,
            Some("model_loaded".to_string()),
            None,
        );
        service_guard::record_vector_worker_heartbeat();
        let current_pressure = service_guard::current_pressure();
        let claimable_file_backlog_depth = graph_store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap_or(0);
        let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = graph_store
            .fetch_file_vectorization_queue_counts()
            .unwrap_or((0, 0));
        let (outbox_queued, outbox_inflight) = graph_store
            .fetch_vector_persist_outbox_counts()
            .unwrap_or((0, 0));
        let aggregate_vector_backlog_depth = file_vectorization_queue_queued
            + file_vectorization_queue_inflight
            + outbox_queued
            + outbox_inflight;
        let (graph_projection_queue_queued, graph_projection_queue_inflight) = graph_store
            .fetch_graph_projection_queue_counts()
            .unwrap_or((0, 0));
        let graph_backlog_depth =
            graph_projection_queue_queued + graph_projection_queue_inflight;
        let effective_graph_backlog_depth = effective_vector_lane_graph_backlog_depth(
            embedding_lane_config_from_env(),
            graph_backlog_depth,
        );
        let gpu_available = effective_embedding_provider_is_gpu();
        if gpu_available
            && !gpu_secondary_worker_allowed(worker_idx, current_gpu_memory_snapshot())
        {
            service_guard::record_vector_worker_admission_reason(
                "gpu_secondary_worker_vram_guard",
                1,
            );
            service_guard::record_vectorization_suppressed();
            thread::sleep(Duration::from_millis(
                vector_worker_non_admitted_backlog_wait_ms(aggregate_vector_backlog_depth),
            ));
            continue;
        }
        let admission = vector_worker_admission_decision(
            worker_idx,
            current_pressure,
            gpu_available,
            claimable_file_backlog_depth,
        );
        service_guard::record_vector_worker_admission_reason(
            admission.reason,
            admission.allowed_gpu_workers,
        );
        if !admission.admitted {
            if claimable_file_backlog_depth == 0 {
                let signaled = wait_for_vector_backlog_or_timeout(Duration::from_millis(
                    vector_worker_non_admitted_idle_wait_ms(claimable_file_backlog_depth),
                ));
                if signaled {
                    service_guard::record_runtime_wakeup(
                        service_guard::RuntimeWakeSource::SemanticVector,
                        graph_backlog_depth as u64,
                        aggregate_vector_backlog_depth as u64,
                    );
                }
            } else {
                thread::sleep(Duration::from_millis(
                    vector_worker_non_admitted_backlog_wait_ms(claimable_file_backlog_depth),
                ));
            }
            continue;
        }
        let policy = semantic_policy_with_graph(
            claimable_file_backlog_depth,
            effective_graph_backlog_depth,
            current_pressure,
        );
        if policy.pause {
            if claimable_file_backlog_depth == 0 {
                let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                if signaled {
                    service_guard::record_runtime_wakeup(
                        service_guard::RuntimeWakeSource::SemanticVector,
                        graph_backlog_depth as u64,
                        aggregate_vector_backlog_depth as u64,
                    );
                }
            } else {
                thread::sleep(policy.sleep);
            }
            continue;
        }

        let mut backlog_active = ready_batches.len() > 0 || claimable_file_backlog_depth > 0;
        let mut completed_works: Vec<FileVectorizationWork> = Vec::new();
        let mut completed_batch_runs: Vec<VectorBatchRun> = Vec::new();
        let mut failed: HashMap<String, Vec<FileVectorizationWork>> = HashMap::new();
        let mut inflight_persists: VecDeque<InflightPersistRequest> = VecDeque::new();
        let max_inflight_persists = configured_vector_max_inflight_persists();

        while gpu_worker_has_pending_work(
            ready_batches.len(),
            inflight_persists.len(),
            service_guard::vector_runtime_metrics().prepare_inflight_current,
            claimable_file_backlog_depth,
        ) {
            while let Some(inflight) = inflight_persists.front() {
                match inflight.reply_rx.try_recv() {
                    Ok(outcome) => {
                        inflight_persists.pop_front();
                        let requeue = apply_vector_persist_outcome(
                            outcome,
                            ready_batches.as_ref(),
                            &mut completed_works,
                            &mut completed_batch_runs,
                            &mut failed,
                        );
                        send_vector_refill_requeue(
                            &graph_store,
                            worker_idx,
                            &refill_tx,
                            requeue,
                        );
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        inflight_persists.pop_front();
                        error!(
                            "Semantic Vector Worker [{}]: persist reply disconnected before completion",
                            worker_idx
                        );
                    }
                }
            }

            // --- Unified VRAM recycle coordinator (pre-batch collection point) ---
            let recycle_metrics = service_guard::vector_runtime_metrics();
            let recycle_snapshot = current_gpu_memory_snapshot();
            let recycle_chunks_per_second =
                service_guard::vector_chunk_embeddings_per_second();
            let recycle_vram_used_mb = recycle_snapshot
                .map(|s| s.used_mb)
                .unwrap_or(0);
            let recycle_signals = RecycleSignals {
                stuck: gpu_stuck_recovery_reason(recycle_metrics, inflight_persists.len())
                    .is_some(),
                summit: gpu_recycle_immediate_required(
                    recycle_snapshot,
                    inflight_persists.len(),
                ),
                pre_batch_plateau: gpu_pre_batch_vram_recycle_reason(recycle_snapshot)
                    .is_some(),
                low_throughput: recycle_snapshot
                    .map(|s| s.used_mb >= gpu_recycle_vram_summit_mb())
                    .unwrap_or(false)
                    && recycle_chunks_per_second <= 8.0,
                vram_critical: recycle_snapshot
                    .map(|s| {
                        s.total_mb > 0
                            && s.used_mb > s.total_mb * RECYCLE_VRAM_CRITICAL_PCT / 100
                    })
                    .unwrap_or(false),
                vram_used_mb: recycle_vram_used_mb,
            };
            if let Some(reason) = recycle_coordinator.should_recycle(&recycle_signals) {
                warn!(
                    "Semantic Vector Worker [{}]: unified recycle: {}",
                    worker_idx, reason
                );
                schedule_vector_worker_restart(
                    &graph_store,
                    worker_idx,
                    FatalVectorWorkerFault {
                        stage: "unified_recycle",
                        reason_raw: reason,
                        fatal_class: "gpu_recycle".to_string(),
                        batch_id: None,
                        texts_count: recycle_metrics.embed_inflight_texts_current,
                        input_bytes: recycle_metrics.embed_inflight_text_bytes_current,
                    },
                    &mut restart_window,
                    &mut restart_attempt,
                );
                drop(model);
                continue 'worker_lifecycle;
            }

            if !completed_works.is_empty() {
                let _ =
                    graph_store.refresh_inflight_file_vectorization_claims(&completed_works);
            }

            let ready_queue_depth_at_gpu_start = ready_batches.len() as u64;
            let ready_queue_chunks_at_gpu_start = ready_batches.summary().chunk_count as u64;
            let prepare_inflight_at_gpu_start =
                service_guard::vector_runtime_metrics().prepare_inflight_current;
            let prepare_inflight_chunks_at_gpu_start =
                service_guard::vector_runtime_metrics().prepare_inflight_chunks_current;
            let Some(prepared) = ready_batches.pop_best() else {
                if gpu_worker_should_wait_for_ready(
                    ready_batches.len(),
                    inflight_persists.len(),
                    service_guard::vector_runtime_metrics().prepare_inflight_current,
                    claimable_file_backlog_depth,
                ) {
                    let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(1));
                    backlog_active = true;
                    continue;
                }
                if let Some(inflight) = inflight_persists.pop_front() {
                    if let Some(outcome) = wait_for_vector_persist_outcome(
                        &graph_store,
                        worker_idx,
                        inflight.reply_rx,
                        inflight.claimed.as_slice(),
                    ) {
                        let persist_succeeded = outcome.error_reason.is_none();
                        let requeue = apply_vector_persist_outcome(
                            outcome,
                            ready_batches.as_ref(),
                            &mut completed_works,
                            &mut completed_batch_runs,
                            &mut failed,
                        );
                        send_vector_refill_requeue(
                            &graph_store,
                            worker_idx,
                            &refill_tx,
                            requeue,
                        );
                        // VRAM summit observe: feed signal to coordinator
                        // (actual recycle decision deferred to top of loop).
                        if persist_succeeded {
                            let vram_used_mb = current_gpu_memory_snapshot()
                                .map(|snapshot| snapshot.used_mb)
                                .unwrap_or(0);
                            let chunks_per_second =
                                service_guard::vector_chunk_embeddings_per_second();
                            gpu_recycle_after_vram_summit_observe(
                                vram_used_mb,
                                chunks_per_second,
                                &mut gpu_recycle_candidate_batches,
                            );
                        } else {
                            gpu_recycle_candidate_batches = 0;
                        }
                    }
                    flush_completed_vectorization_works(
                        worker_idx,
                        &graph_store,
                        &finalize_tx,
                        &mut completed_works,
                        &mut completed_batch_runs,
                        &mut failed,
                    );
                    let ready_summary = ready_batches.summary();
                    record_ready_queue_summary(&ready_summary);
                    continue;
                }
                break;
            };
            backlog_active = true;
            let consumed_chunk_count = prepared.chunk_count().max(1);
            service_guard::notify_vector_backlog_activity();
            if service_guard::interactive_priority_active() {
                let interrupted_batches = ready_batches.retain(|batch| {
                    !batch
                        .touched_works
                        .iter()
                        .any(|work| pause_vectorization_work_if_interactive(&graph_store, work))
                });
                if !interrupted_batches.is_empty() {
                    let ready_summary = ready_batches.summary();
                    record_ready_queue_summary(&ready_summary);
                }
            }
            // Pre-batch VRAM guard: the coordinator at the top of the loop
            // already evaluates this signal.  We keep the admission-level
            // check for non-recycle VRAM gating (worker pausing).
            let gpu_memory_snapshot = current_gpu_memory_snapshot();
            if !gpu_worker_consumption_allowed(gpu_available, gpu_memory_snapshot) {
                service_guard::record_vector_worker_admission_reason(
                    "gpu_primary_worker_vram_guard",
                    (service_guard::current_allowed_gpu_workers().max(1))
                        .try_into()
                        .unwrap_or(usize::MAX),
                );
                let ready_depth = ready_batches.push_front(prepared);
                let _ = ready_depth;
                record_ready_queue_summary(&ready_batches.summary());
                thread::sleep(Duration::from_millis(
                    vector_worker_non_admitted_backlog_wait_ms(aggregate_vector_backlog_depth),
                ));
                continue;
            }
            let ready_summary = ready_batches.summary();
            record_ready_queue_summary(&ready_summary);
            service_guard::record_vector_last_consumed_batch_lane(service_guard_batch_lane(
                prepared.batch_lane(),
            ));
            if let Err(err) = refill_tx.send(VectorRefillCommand::BatchConsumed(
                consumed_chunk_count as usize,
            )) {
                error!(
                    "Semantic Vector Worker [{}]: failed to publish consumed batch event to refill worker: {}",
                    worker_idx, err
                );
                service_guard::record_vector_ready_replenishment_requested(
                    consumed_chunk_count,
                );
            }

            for (work, reason) in &prepared.failed_fetches {
                error!(
                    "Semantic Vector Worker [{}]: failed to fetch unembedded chunks for {}: {}",
                    worker_idx, work.file_path, reason
                );
                failed.entry(reason.clone()).or_default().push(work.clone());
            }
            for work in &prepared.oversized_works {
                if let Err(err) =
                    graph_store.mark_file_oversized_for_current_budget(&work.file_path)
                {
                    error!(
                        "Semantic Vector Worker [{}]: failed to mark oversized file {}: {:?}",
                        worker_idx, work.file_path, err
                    );
                    failed
                        .entry("failed to mark oversized_for_current_budget".to_string())
                        .or_default()
                        .push(work.clone());
                } else {
                    info!(
                                "Semantic Vector Worker [{}]: marked oversized file for current budget: {}",
                                worker_idx, work.file_path
                            );
                }
            }
            service_guard::record_vector_partial_file_cycles(
                prepared.partial_file_cycles as u64,
            );
            if prepared.work_items.is_empty() {
                completed_works.extend(prepared.immediate_completed.clone());
                completed_works.extend(prepared.finalize_after_success.clone());
                flush_completed_vectorization_works(
                    worker_idx,
                    &graph_store,
                    &finalize_tx,
                    &mut completed_works,
                    &mut completed_batch_runs,
                    &mut failed,
                );
                continue;
            }
            let embed_input_texts = prepared.texts.len() as u64;
            let embed_input_text_bytes = prepared
                .texts
                .iter()
                .map(|text| text.len() as u64)
                .sum::<u64>();
            let total_tokens = prepared.total_token_count();
            let max_item_tokens = prepared.max_item_tokens();
            let avg_item_tokens = prepared.avg_item_tokens();
            let micro_batch_count = prepared.micro_batch_count();
            let max_micro_batch_tokens = prepared.max_micro_batch_tokens();
            let avg_micro_batch_tokens = prepared.avg_micro_batch_tokens();
            let effective_vector_workers_admitted =
                service_guard::vector_runtime_metrics().vector_workers_active_current;
            let vector_worker_admission_reason =
                service_guard::current_vector_worker_admission_reason();
            let allowed_gpu_workers = service_guard::current_allowed_gpu_workers();
            let batch_started_at_ms = chrono::Utc::now().timestamp_millis();
            let last_gpu_finished_at_ms = service_guard::current_last_embed_finished_at_ms();
            let batch_wait_for_ready_ms = if last_gpu_finished_at_ms > 0
                && (batch_started_at_ms as u64) >= last_gpu_finished_at_ms
            {
                (batch_started_at_ms as u64).saturating_sub(last_gpu_finished_at_ms)
            } else {
                0
            };
            if let Err(err) =
                graph_store.refresh_inflight_file_vectorization_claims(&prepared.touched_works)
            {
                warn!(
                            "Semantic Vector Worker [{}]: failed to refresh inflight vectorization claims before embed: {:?}",
                            worker_idx, err
                        );
            }
            service_guard::record_vector_embed_attempt(
                embed_input_texts,
                embed_input_text_bytes,
            );
            let embed_owned_workset = merge_unique_vectorization_work_sets([
                ready_batches.touched_works_snapshot(),
                inflight_persists
                    .iter()
                    .flat_map(|request| request.claimed.clone_works())
                    .collect::<Vec<_>>(),
                prepared.touched_works.clone(),
                completed_works.clone(),
            ]);
            let _embed_lease_guard = LeaseRefreshGuard::start(
                Arc::clone(&graph_store),
                embed_owned_workset,
                "vector",
            );
            let gpu_started_at_ms = chrono::Utc::now().timestamp_millis();
            let embed_started = Instant::now();
            let _ = graph_store.mark_file_vectorization_started(&prepared.touched_works);
            let mut recreate_gpu_session_after_batch = false;
            match model.embed_prepared_batch_with_breakdown(&prepared) {
                Ok((
                    embeddings,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                )) => {
                    service_guard::record_vector_embed_inputs(embeddings.len() as u64, 0, 0);
                    service_guard::record_vector_embed_attempt_finished();
                    service_guard::record_vector_lane_success();
                    service_guard::record_vector_embed_breakdown(
                        inference_ms,
                        output_extract_ms,
                    );
                    service_guard::record_vector_embed_inputs(
                        embed_input_texts,
                        embed_input_text_bytes,
                        host_prepare_ms.saturating_add(input_copy_ms),
                    );
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::Embed,
                        embed_started.elapsed().as_millis() as u64,
                    );
                    let touched_works = prepared.touched_works.clone();
                    let next_active_after_failure = prepared.next_active_after_failure.clone();
                    let prepared_fetch_ms_total = prepared.fetch_ms_total;
                    let prepared_batch_id = prepared.batch_id.clone();
                    let prepare_started_at_ms = prepared.prepare_started_at_ms;
                    let prepare_finished_at_ms = prepared.prepare_finished_at_ms;
                    let ready_enqueued_at_ms = prepared.prepared_at_ms;
                    let prepared_batch_lane = prepared.batch_lane();
                    let prepared_mixed_fallback = prepared.mixed_fallback();
                    let prepared_lane_thresholds = prepared.lane_thresholds();
                    let persist_plan = match prepared.into_persist_envelope(
                        embeddings,
                        VectorBatchRun {
                            run_id: prepared_batch_id,
                            prepare_started_at_ms,
                            prepare_finished_at_ms,
                            ready_enqueued_at_ms,
                            started_at_ms: batch_started_at_ms,
                            finished_at_ms: chrono::Utc::now().timestamp_millis(),
                            gpu_started_at_ms,
                            gpu_finished_at_ms: chrono::Utc::now().timestamp_millis(),
                            persist_enqueued_at_ms: 0,
                            persist_started_at_ms: 0,
                            persist_finished_at_ms: 0,
                            finalize_enqueued_at_ms: 0,
                            finalize_finished_at_ms: 0,
                            provider: current_embedding_provider_diagnostics()
                                .provider_effective,
                            runner_kind: "ort_gpu_first_iobinding".to_string(),
                            model_id: CHUNK_MODEL_ID.to_string(),
                            chunk_count: 0,
                            file_count: 0,
                            input_bytes: embed_input_text_bytes,
                            total_tokens,
                            max_item_tokens,
                            avg_item_tokens,
                            micro_batch_count,
                            max_micro_batch_tokens,
                            avg_micro_batch_tokens,
                            effective_vector_workers_admitted,
                            ready_queue_depth_at_gpu_start,
                            prepare_inflight_at_gpu_start,
                            ready_queue_chunks_at_gpu_start,
                            prepare_inflight_chunks_at_gpu_start,
                            vector_worker_admission_reason,
                            allowed_gpu_workers,
                            batch_wait_for_ready_ms,
                            persist_queue_wait_ms: 0,
                            finalize_queue_wait_ms: 0,
                            batch_lane: prepared_batch_lane.as_str().to_string(),
                            batch_shape: if prepared_mixed_fallback {
                                "mixed_fallback".to_string()
                            } else {
                                "homogeneous".to_string()
                            },
                            lane_small_max_tokens: prepared_lane_thresholds.small_max_tokens
                                as u64,
                            lane_medium_max_tokens: prepared_lane_thresholds.medium_max_tokens
                                as u64,
                            fetch_ms: prepared_fetch_ms_total
                                .saturating_add(host_prepare_ms)
                                .saturating_add(input_copy_ms),
                            embed_ms: embed_started.elapsed().as_millis() as u64,
                            db_write_ms: 0,
                            mark_done_ms: 0,
                            success: true,
                            error_reason: None,
                        },
                    ) {
                        Ok(envelope) => envelope,
                        Err(err) => {
                            let reason =
                                format!("failed to build vector persist plan: {:?}", err);
                            failed.entry(reason).or_default().extend(touched_works);
                            let recovered_ready_works =
                                recover_ready_batches_to_active_works(&ready_batches);
                            let merge_target = next_active_after_failure
                                .len()
                                .saturating_add(recovered_ready_works.len())
                                .max(1);
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                merge_vectorization_work(
                                    next_active_after_failure,
                                    recovered_ready_works,
                                    merge_target,
                                ),
                            );
                            continue;
                        }
                    };
                    let mut persist_envelope = persist_plan;
                    persist_envelope.sync_batch_run_counts_from_plan();
                    while inflight_persists.len() >= max_inflight_persists {
                        let Some(inflight) = inflight_persists.pop_front() else {
                            break;
                        };
                        if let Some(outcome) = wait_for_vector_persist_outcome(
                            &graph_store,
                            worker_idx,
                            inflight.reply_rx,
                            inflight.claimed.as_slice(),
                        ) {
                            let persist_succeeded = outcome.error_reason.is_none();
                            let requeue = apply_vector_persist_outcome(
                                outcome,
                                ready_batches.as_ref(),
                                &mut completed_works,
                                &mut completed_batch_runs,
                                &mut failed,
                            );
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                requeue,
                            );
                            // VRAM summit observe: feed signal to coordinator
                            // (actual recycle decision deferred to top of loop).
                            if persist_succeeded {
                                let vram_used_mb = current_gpu_memory_snapshot()
                                    .map(|snapshot| snapshot.used_mb)
                                    .unwrap_or(0);
                                let chunks_per_second =
                                    service_guard::vector_chunk_embeddings_per_second();
                                gpu_recycle_after_vram_summit_observe(
                                    vram_used_mb,
                                    chunks_per_second,
                                    &mut gpu_recycle_candidate_batches,
                                );
                            } else {
                                gpu_recycle_candidate_batches = 0;
                            }
                        }
                    }
                    if let Err(err) = graph_store.mark_file_vectorization_persist_started(&touched_works) {
                        warn!(
                            "Semantic Vector Worker [{}]: failed to mark persist_started_at_ms: {:?}",
                            worker_idx, err
                        );
                    }
                    match dispatch_vector_persist_plan(&persist_tx, persist_envelope) {
                        Ok(reply_rx) => {
                            inflight_persists.push_back(InflightPersistRequest {
                                reply_rx,
                                claimed: ClaimedLeaseSet::new(touched_works),
                            });
                        }
                        Err(err) => {
                            let reason =
                                format!("failed to dispatch vector persist plan: {:?}", err);
                            failed.entry(reason).or_default().extend(touched_works);
                            let recovered_ready_works =
                                recover_ready_batches_to_active_works(&ready_batches);
                            let merge_target = next_active_after_failure
                                .len()
                                .saturating_add(recovered_ready_works.len())
                                .max(1);
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                merge_vectorization_work(
                                    next_active_after_failure,
                                    recovered_ready_works,
                                    merge_target,
                                ),
                            );
                        }
                    }
                    let ready_queue_summary = ready_batches.summary();
                    record_vector_pipeline_snapshot(
                        &graph_store,
                        claimable_file_backlog_depth,
                        &[],
                        &VecDeque::new(),
                        &ready_queue_summary,
                        &inflight_persists,
                        ready_queue_summary.chunk_count,
                        0,
                    );
                    flush_completed_vectorization_works(
                        worker_idx,
                        &graph_store,
                        &finalize_tx,
                        &mut completed_works,
                        &mut completed_batch_runs,
                        &mut failed,
                    );
                    recreate_gpu_session_after_batch =
                        gpu_recreate_session_every_batch_enabled();
                }
                Err(e) => {
                    service_guard::record_vector_embed_attempt_finished();
                    let reason = format!("chunk embedding failed: {:?}", e);
                    if is_gpu_recycle_immediate_error(&e) {
                        let recovered_ready_works =
                            recover_ready_batches_to_active_works(&ready_batches);
                        let merge_target = prepared
                            .touched_works
                            .len()
                            .saturating_add(prepared.next_active_after_failure.len())
                            .saturating_add(recovered_ready_works.len())
                            .max(1);
                        send_vector_refill_requeue(
                            &graph_store,
                            worker_idx,
                            &refill_tx,
                            merge_vectorization_work(
                                prepared.touched_works.clone(),
                                merge_vectorization_work(
                                    prepared.next_active_after_failure.clone(),
                                    recovered_ready_works,
                                    merge_target,
                                ),
                                merge_target,
                            ),
                        );
                        drop(model);
                        schedule_vector_worker_restart(
                            &graph_store,
                            worker_idx,
                            FatalVectorWorkerFault {
                                stage: "gpu_recycle_immediate",
                                reason_raw: reason,
                                fatal_class: "gpu_recycle".to_string(),
                                batch_id: Some(prepared.batch_id.clone()),
                                texts_count: embed_input_texts,
                                input_bytes: embed_input_text_bytes,
                            },
                            &mut restart_window,
                            &mut restart_attempt,
                        );
                        continue 'worker_lifecycle;
                    }
                    if let Some(fatal_class) = fatal_embedding_error_class(&e) {
                        error!(
                                    "Semantic Vector Worker [{}]: fatal chunk embedding error, restarting semantic lane: {:?}",
                                    worker_idx, e
                                );
                        drop(model);
                        schedule_vector_worker_restart(
                            &graph_store,
                            worker_idx,
                            FatalVectorWorkerFault {
                                stage: "embed",
                                reason_raw: reason,
                                fatal_class: fatal_class.to_string(),
                                batch_id: Some(prepared.batch_id.clone()),
                                texts_count: embed_input_texts,
                                input_bytes: embed_input_text_bytes,
                            },
                            &mut restart_window,
                            &mut restart_attempt,
                        );
                        continue 'worker_lifecycle;
                    }
                    error!(
                        "Semantic Vector Worker [{}]: Chunk embedding failed: {:?}",
                        worker_idx, e
                    );
                    let recovered_ready_works =
                        recover_ready_batches_to_active_works(&ready_batches);
                    failed
                        .entry(reason)
                        .or_default()
                        .extend(prepared.touched_works.iter().cloned());
                    let merge_target = prepared
                        .next_active_after_failure
                        .len()
                        .saturating_add(recovered_ready_works.len())
                        .max(1);
                    send_vector_refill_requeue(
                        &graph_store,
                        worker_idx,
                        &refill_tx,
                        merge_vectorization_work(
                            prepared.next_active_after_failure.clone(),
                            recovered_ready_works,
                            merge_target,
                        ),
                    );
                }
            }
            while let Some(inflight) = inflight_persists.pop_front() {
                if let Some(outcome) = wait_for_vector_persist_outcome(
                    &graph_store,
                    worker_idx,
                    inflight.reply_rx,
                    inflight.claimed.as_slice(),
                ) {
                    let persist_succeeded = outcome.error_reason.is_none();
                    let requeue = apply_vector_persist_outcome(
                        outcome,
                        ready_batches.as_ref(),
                        &mut completed_works,
                        &mut completed_batch_runs,
                        &mut failed,
                    );
                    send_vector_refill_requeue(&graph_store, worker_idx, &refill_tx, requeue);
                    // VRAM summit observe: feed signal to coordinator
                    // (actual recycle decision deferred to top of loop).
                    if persist_succeeded {
                        let vram_used_mb = current_gpu_memory_snapshot()
                            .map(|snapshot| snapshot.used_mb)
                            .unwrap_or(0);
                        let chunks_per_second =
                            service_guard::vector_chunk_embeddings_per_second();
                        gpu_recycle_after_vram_summit_observe(
                            vram_used_mb,
                            chunks_per_second,
                            &mut gpu_recycle_candidate_batches,
                        );
                    } else {
                        gpu_recycle_candidate_batches = 0;
                    }
                }
            }

            flush_completed_vectorization_works(
                worker_idx,
                &graph_store,
                &finalize_tx,
                &mut completed_works,
                &mut completed_batch_runs,
                &mut failed,
            );
            if recreate_gpu_session_after_batch {
                info!(
                    "Semantic Vector Worker [{}]: recreating GPU session after completed batch for diagnostic VRAM control",
                    worker_idx
                );
                drop(model);
                continue 'worker_lifecycle;
            }

            for (reason, works) in std::mem::take(&mut failed) {
                if let Err(err) =
                    graph_store.mark_file_vectorization_work_failed(&works, &reason)
                {
                    error!(
                            "Semantic Vector Worker [{}]: failed to persist file vector backlog failure [{}]: {:?}",
                            worker_idx, reason, err
                        );
                }
            }
        }

        if !symbol_embedding_allowed(aggregate_vector_backlog_depth, current_pressure) {
            if !backlog_active {
                if claimable_file_backlog_depth == 0 {
                    wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                } else {
                    thread::sleep(policy.idle_sleep);
                }
            }
            continue;
        }

        match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
            Ok(symbols) if !symbols.is_empty() => {
                backlog_active = true;
                if wake_idle {
                    service_guard::record_runtime_wakeup(
                        service_guard::RuntimeWakeSource::SemanticVector,
                        graph_backlog_depth as u64,
                        aggregate_vector_backlog_depth as u64,
                    );
                    wake_idle = false;
                }
                debug!(
                    "Semantic Vector Worker [{}]: Embedding {} symbols...",
                    worker_idx,
                    symbols.len()
                );

                let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                match model.embed_texts_with_breakdown(&texts) {
                    Ok((
                        embeddings,
                        host_prepare_ms,
                        input_copy_ms,
                        inference_ms,
                        output_extract_ms,
                    )) => {
                        service_guard::record_vector_embed_breakdown(
                            host_prepare_ms
                                .saturating_add(input_copy_ms)
                                .saturating_add(inference_ms),
                            output_extract_ms,
                        );
                        let updates: Vec<(String, Vec<f32>)> = symbols
                            .into_iter()
                            .zip(embeddings)
                            .map(|((id, _), emb)| (id, emb))
                            .collect();

                        if let Err(e) = graph_store.update_symbol_embeddings(&updates) {
                            error!(
                                "Semantic Vector Worker [{}]: symbol DB write error: {:?}",
                                worker_idx, e
                            );
                        }
                    }
                    Err(e) => {
                        if is_gpu_recycle_immediate_error(&e) {
                            drop(model);
                            schedule_vector_worker_restart(
                                &graph_store,
                                worker_idx,
                                FatalVectorWorkerFault {
                                    stage: "gpu_recycle_immediate",
                                    reason_raw: format!(
                                        "symbol gpu recycle immediate: {:?}",
                                        e
                                    ),
                                    fatal_class: "gpu_recycle".to_string(),
                                    batch_id: None,
                                    texts_count: texts.len() as u64,
                                    input_bytes: texts
                                        .iter()
                                        .map(|text| text.len() as u64)
                                        .sum(),
                                },
                                &mut restart_window,
                                &mut restart_attempt,
                            );
                            continue 'worker_lifecycle;
                        }
                        if let Some(fatal_class) = fatal_embedding_error_class(&e) {
                            error!(
                                "Semantic Vector Worker [{}]: fatal symbol embedding error, restarting semantic lane: {:?}",
                                worker_idx, e
                            );
                            drop(model);
                            schedule_vector_worker_restart(
                                &graph_store,
                                worker_idx,
                                FatalVectorWorkerFault {
                                    stage: "symbol_embed",
                                    reason_raw: format!("symbol embedding failed: {:?}", e),
                                    fatal_class: fatal_class.to_string(),
                                    batch_id: None,
                                    texts_count: texts.len() as u64,
                                    input_bytes: texts
                                        .iter()
                                        .map(|text| text.len() as u64)
                                        .sum(),
                                },
                                &mut restart_window,
                                &mut restart_attempt,
                            );
                            continue 'worker_lifecycle;
                        }
                        error!(
                            "Semantic Vector Worker [{}]: symbol embedding failed: {:?}",
                            worker_idx, e
                        );
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                error!(
                    "Semantic Vector Worker [{}]: symbol fetch error: {:?}",
                    worker_idx, e
                );
                if claimable_file_backlog_depth == 0 {
                    let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                    if signaled {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            graph_backlog_depth as u64,
                            aggregate_vector_backlog_depth as u64,
                        );
                    }
                } else {
                    thread::sleep(policy.idle_sleep);
                }
            }
        }

        if !backlog_active {
            wake_idle = true;
            if claimable_file_backlog_depth == 0 {
                let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                if signaled {
                    service_guard::record_runtime_wakeup(
                        service_guard::RuntimeWakeSource::SemanticVector,
                        graph_backlog_depth as u64,
                        aggregate_vector_backlog_depth as u64,
                    );
                }
            } else {
                thread::sleep(policy.idle_sleep);
            }
        }
    }
}

/// REQ-AXO-173 — Gate the GPU embed subprocess spawn on the canonical
/// embedding provider being CUDA. Without this gate, dev with `gpu=avoid`
/// (provider=cpu) still spawned the GPU subprocess (because
/// `AXON_GPU_EMBED_SERVICE_ENABLED=1` is a runtime_boot default), which
/// immediately panicked on `dlopen` of `libonnxruntime.so` — start.sh's
/// `axon-ort-runtime.sh` only configures `ORT_DYLIB_PATH` and
/// `LD_LIBRARY_PATH` when provider=cuda, so the subprocess inherited
/// no usable ORT runtime and zombified. Pre-fix: 9-14 zombies stacked
/// per dev session under `--indexer-full` + gpu=avoid. The in-process
/// CPU branch below is the canonical path for non-CUDA providers.
pub(super) fn gpu_embed_subprocess_should_spawn(
    provider_requested: &str,
    service_enabled: bool,
) -> bool {
    service_enabled && provider_requested.eq_ignore_ascii_case("cuda")
}

fn build_vector_embedding_model(worker_idx: usize) -> Option<VectorEmbeddingBackend> {
    let provider_requested = effective_provider_request_for_lane("vector");
    let cuda_requested = provider_requested.eq_ignore_ascii_case("cuda");

    if gpu_embed_subprocess_should_spawn(&provider_requested, gpu_embed_service_enabled()) {
        match gpu_embedding_service_client() {
            Ok(client) => {
                publish_embedding_provider_state(gpu_service_provider_effective_label(), None);
                return Some(VectorEmbeddingBackend::gpu_service(
                    client,
                    gpu_embed_service_prefers_tensorrt(),
                ));
            }
            Err(err) => {
                let rendered = format!("{err:?}");
                error!(
                    "❌ Semantic vector Worker [{}]: GPU embedding service init failed: {:?}",
                    worker_idx, err
                );
                publish_embedding_provider_state("unavailable", Some(&rendered));
                return None;
            }
        }
    }

    let cuda_available = std::env::var("AXON_EMBEDDING_GPU_PRESENT")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let cuda_provider_library_available = ort_cuda_provider_library_available();

    if cuda_requested && cuda_available && !cuda_provider_library_available {
        let provider_path = ort_cuda_provider_library_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        error!(
            "❌ Semantic vector Worker [{}]: CUDA requested but ONNX Runtime provider library is missing: {}",
            worker_idx, provider_path
        );
        set_embedding_provider_runtime_state("cpu_missing_cuda_provider", None);
    }

    let model_result = if cuda_requested && cuda_available && cuda_provider_library_available {
        match OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, true) {
            Ok(model) => {
                set_embedding_provider_runtime_state("cuda", None);
                Ok(model)
            }
            Err(err) => {
                let rendered = format!("{err:?}");
                error!(
                        "❌ Semantic vector Worker [{}]: ORT CUDA init failed, falling back to CPU: {:?}",
                        worker_idx, err
                    );
                set_embedding_provider_runtime_state("cpu_fallback", Some(&rendered));
                apply_cpu_fallback_ort_runtime_env();
                OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
            }
        }
    } else {
        set_embedding_provider_runtime_state(
            cpu_provider_effective_label(
                cuda_requested,
                cuda_available,
                cuda_provider_library_available,
            ),
            None,
        );
        OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
    };

    match model_result {
        Ok(model) => {
            let provider_effective = current_embedding_provider_effective();
            register_embedding_provider_diagnostics(embedding_provider_diagnostics(
                provider_effective.clone(),
            ));
            if provider_effective.starts_with("cuda") {
                Some(VectorEmbeddingBackend::cuda_in_process(model))
            } else {
                Some(VectorEmbeddingBackend::cpu_in_process(model))
            }
        }
        Err(err) => {
            let rendered = format!("{err:?}");
            error!(
                "❌ Semantic vector Worker [{}]: FATAL ORT GPU-FIRST INIT ERROR: {:?}",
                worker_idx, err
            );
            publish_embedding_provider_state("unavailable", Some(&rendered));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::gpu_embed_subprocess_should_spawn;

    #[test]
    fn extracted_function_links_to_runtime() {
        let _: fn(
            usize,
            std::sync::Arc<crate::graph::GraphStore>,
            crossbeam_channel::Sender<super::VectorRefillCommand>,
            crossbeam_channel::Sender<super::VectorPersistRequest>,
            crossbeam_channel::Sender<super::VectorFinalizeRequest>,
            std::sync::Arc<super::SharedPreparedBatchQueue>,
        ) = super::vector_worker_loop;
    }

    #[test]
    fn gpu_embed_subprocess_should_spawn_only_when_provider_is_cuda() {
        // REQ-AXO-173 — the GPU embed subprocess MUST NOT be spawned
        // when the canonical embedding provider is anything other than
        // CUDA. Pre-fix: dev with gpu=avoid (provider=cpu) still spawned
        // the subprocess via the unconditional `AXON_GPU_EMBED_SERVICE_ENABLED=1`
        // runtime_boot default; subprocess panicked on dlopen of
        // libonnxruntime.so because start.sh's axon-ort-runtime.sh only
        // configures ORT_DYLIB_PATH + LD_LIBRARY_PATH when provider=cuda.
        // 9-14 zombies stacked per session.

        // Service disabled ⇒ never spawn, regardless of provider
        assert!(!gpu_embed_subprocess_should_spawn("cuda", false));
        assert!(!gpu_embed_subprocess_should_spawn("cpu", false));
        assert!(!gpu_embed_subprocess_should_spawn("", false));

        // Service enabled + cuda ⇒ spawn (canonical happy path)
        assert!(gpu_embed_subprocess_should_spawn("cuda", true));
        assert!(
            gpu_embed_subprocess_should_spawn("CUDA", true),
            "provider compare must be case-insensitive (canonical_embedding_provider_request \
             returns lowercase but downstream callers may pass through upstream casing)"
        );

        // Service enabled + non-cuda ⇒ DO NOT spawn (the bug we are fixing)
        assert!(!gpu_embed_subprocess_should_spawn("cpu", true));
        assert!(!gpu_embed_subprocess_should_spawn("tensorrt", true),
            "tensorrt provider routes through the cuda path inside the subprocess (force_gpu=true) \
             but the canonical provider request returned by canonical_embedding_provider_request_for_mode \
             is always literally \"cuda\" or \"cpu\" — anything else is unknown and must not spawn");
        assert!(!gpu_embed_subprocess_should_spawn("", true));
    }
}
