# Session 60 — Handoff (2026-05-29)

**Branch active fin de session :** `feature/pipeline-sq-reorder-point`
**Dernier commit main :** `7d07f81e` (MIL-AXO-028 cat A+B+C+F partial + bugs 901798+901807)
**Working note canonique (audit only) — voir CPT-AXO-052 pour state vivant.**

## Livraisons commitées (7d07f81e)

### MIL-AXO-028 dashboard refactor Phoenix-idiomatic

| Cat | REQ | Scope | État |
|---|---|---|---|
| A | 901801 | 10 dead modules supprimés (Watcher.{Application,Repo,Router,Endpoint,CockpitLive,Schemas,Progress,ProjectMetrics} + migration + tests) | ✅ |
| B | 901802 | Config centralisé runtime.exs (Application.env partout, 0 System.get_env lib/) | ✅ |
| C | 901803 | OTP discipline (:timer.send_interval + mtime cache + catch :exit defensive) | ✅ |
| F partial | 901806 | dashboard_state_v1 single-event PG functions + brain Rust module + dashboard wiring | 🟡 (render unchanged, dual-source migration) |

### Bug fixes

| ID | Sévérité | Scope | État |
|---|---|---|---|
| 901798 | P0 | Probe lies — GpuB2Embedder via OrtGpuFirstTextEmbedding::try_new now updates provider_runtime slot | ✅ cargo check OK, **PAS deploy** |
| 901807 G2 | P2 | orphan_embeddings count exposé dans dashboard_totals | ✅ |
| 901799 | P0 | Dashboard pointait sur live par défaut | ✅ (résolu cat B) |
| 901800 | P1 | Silent fallback cross-instance | ✅ (résolu cat B, allow_cross_instance_fallback=false) |

## Discovery + restructure SOLL session 60

**MIL-AXO-029 (s, Q) pipeline policy** créée puis **restructurée** après 2 audits sub-agents indépendants (architecture + implementation).

### Finding majeure

Le code `pipeline_v2/demand_pull.rs` implémente déjà (s, Q) sous le nom `demand_pull` :
- THRESHOLD env vars = `s` (safety_stock)
- BATCH env vars = `Q` (batch_size)
- Reorder check : `if input_tx.capacity() < threshold { fetch(batch) }` (demand_pull.rs:206-209)
- Hot path : PG NOTIFY listener (demand_pull.rs:144,331)
- Metrics : `DemandPullMetrics` struct (demand_pull.rs:29-66)
- Adaptive 1s/30s = safety net pour NOTIFY ratés, **PAS primary path**

DEC-AXO-901625 reframed : **REFINES** (pas SUPERSEDES) DEC-AXO-901620.

### MIL-AXO-029 final structure : 12 REQ children, 11 slices

| # | REQ | Slice | Priorité |
|---|---|---|---|
| 0 | 901813 | Diagnostic root cause 850 files stuck dev | **P0 FIRST** |
| 1 | 901808 (revised) | Env vars + aliasing additif (garde AXON_DEMAND_PULL_* compat) | P1 |
| 2 | 901809 (revised) | StockTracker struct injectable (PAS AtomicI64 global, PAS n_live_tup) | P1 |
| 3 | 901814 | Admission controller integration (single source-of-truth) | P1 |
| 4 | 901810 (revised) | Replenishment trigger hardening (compare_exchange + coalesce 50ms + reuse claim) | P1 |
| 5 | 901815 | FS watcher coord + max-stock invariant | P1 |
| 6 | 901816 | Observability surfacing (stock_a/b dans MCP + heartbeat + dashboard_state_v1) | **P0** |
| 7 | 901817 | Feature flag AXON_REPLENISHMENT_MODE={legacy\|sq} rollback safety | **P0** |
| 8 | 901812 (revised) | Bench harness extended (--idle-secs + --truncate-at-secs + A vs B differential) | P1 |
| 9 | 901818 | Promote-live + smoke + crash recovery test | P1 |
| 10 | 901811 (revised) | Cleanup adaptive polling — LAST, behind flag, AFTER smoke 30+ min | P1 |
| 11 | 901819 | Documentation (CLAUDE.md + CPT-AXO-054 + PIL-AXO-007 + DEC-AXO-901620 REFINES link) | P2 |

Estimate révisé : **~10.75 ed** (vs 5 ed initial).

## Bug critique empirique

**850 files stuck en `IndexedFile.status='discovered'` sur axon_dev** depuis 10+ min idle ticks (5 monitor ticks consécutifs sans Δ).

Hypothèses priorisées (à investiguer en REQ-AXO-901813 slice 0) :
1. NOTIFY channel mismatch (35%)
2. tx.capacity() voit channel non-vide → demand_pull pas trigger (25%)
3. Pipeline B back-pressure remonte (15%)
4. select_and_claim SELECT bug (15%)
5. Claim cleanup stale (10%)

## Audits sub-agents — convergence

Voir transcripts dans `/tmp/claude-1000/-home-dstadel-projects-axon/7d50fc79-a0cb-456c-a769-2fa0f7b6688d/tasks/`.

Both audits convergent sur :
- (s, Q) existe déjà comme demand_pull
- Pipeline a un bug réel empirique (850 stuck)
- MIL initial sous-estimé ~2× (5 → ~10-11 ed)
- Manque 6 slices critiques (observability + feature flag + admission + watcher + smoke + docs)
- Slice cleanup adaptive polling doit être LAST (rollback safety)

## Mémoires feedback ajoutées session 60

Lien : `/home/dstadel/.claude/projects/-home-dstadel-projects-axon/memory/`

- `feedback_always_propose_with_justification.md`
- `feedback_pro_vs_axo_namespace_distinction.md`
- `feedback_propose_with_confidence_percentages.md`
- `feedback_never_block_on_long_ops.md`

## Next session — actions prioritaires

1. **(P0)** REQ-AXO-901813 slice 0 diagnostic 850 files stuck (avant tout code)
2. Rebuild + deploy brain fix 901798 sur dev (cargo build --release + restart)
3. Cat F dashboard side : F5 render migration + F6 supprimer IndexerHeartbeat+McpPoller (post compile validation)
4. Mix compile dashboard cat A+B+C peut nécessiter complétion (monitor task bzknywzsx armé timeout 30min)

## Blockers / known issues

- 850 files stuck = empirical bug (slice 0 priority)
- Probe fix 901798 cargo check OK mais binary non-déployé (dev brain pid 6726 utilise old release)
- Live indexer DOWN (REQ-AXO-901797 superseded par REQ-AXO-901796 — indexer-graph mode missing)
- Mix compile dashboard peut-être incomplet (~25 deps phoenix LiveView+bandit+ecto+tailwind)

## Addendum post-hand-off 21:55 — diagnostic empirique REQ-AXO-901820

Investigation continuée après handoff initial. Evidences capturées :

1. **PG trigger NOTIFY** : `trg_notify_file_discovered` émet correctement `NOTIFY 'file_discovered'` quand `NEW.status='discovered'` (INSERT+UPDATE). Verified working.

2. **SELECT FOR UPDATE SKIP LOCKED EXPLAIN ANALYZE** : 100 rows returned in 1.187ms via `idx_indexedfile_discovered`. SQL parfait. Bug N'EST PAS dans la query.

3. **PG locks** : aucun lock writers (que AccessShareLock de monitoring). Pas de contention.

4. **Brain env mis-config (REQ-AXO-901821 nouveau)** :
   ```
   Brain pid 6726 :
     AXON_RUNTIME_IDENTITY=axon-dev-axon-indexer  ← WRONG
     AXON_RUNTIME_SHADOW_ROLE=indexer              ← WRONG
     AXON_RUNTIME_STATE_FILE=.axon-dev/run-indexer/runtime.env  ← Points to indexer
   ```
   Brain a hérité env vars de l'indexer (process-compose.dev.yaml ou start.sh leak).

5. **Diagnostic réduit** : éliminé NOTIFY race + claim stale + SELECT bug + PG locks + hot-loop. Bug est 100% côté Rust demand_pull worker (pas spawn / crashed / deadlock A1→A2→A3).

## Next-session quick-tests décisifs (REQ-AXO-901820 description)

```bash
# 1. Indexer log boot message
grep 'demand-pull A: active' .axon-dev/run-indexer/log/*.log

# 2. Heartbeat workers actifs
jq '.runtime_telemetry.graph_workers_active_current' .axon-dev/run-indexer/runtime-heartbeat.json

# 3. strace futex stuck
sudo strace -p 6727 -e trace=futex,epoll_wait,nanosleep -c 2>&1 & sleep 10 ; kill %1

# 4. Restart dev indexer seul
./scripts/axon-dev stop --role indexer
./scripts/axon-dev start --indexer-full
# Observer : 850 stuck se vident OR reste stuck
```

Si #4 débloque → fix simple restart pattern. Si #4 ne débloque pas → bug architectural Rust justifie MIL-AXO-029 (s, Q) refactor.
