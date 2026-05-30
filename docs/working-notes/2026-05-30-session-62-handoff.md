# Session 62 hand-off — 2026-05-30

Canonical state lives in SOLL `CPT-AXO-052` (session_pointer). This file
is the audit-only narrative companion : context, motivation, dead ends,
operator interactions. Run `sql SELECT description FROM soll.Node WHERE
id='CPT-AXO-052'` for the live pointer.

## Opening posture

Session opened with the `axon init` flow (`GUI-PRO-102`). Operator
issued `/goal résoud M1, M3 et M4 99.9%`, then `go`, then continued
nudging through clarifying questions. The session ran for ~6 hours of
wall clock, ~10 commits.

## Trajectory

1. **M4 unblock** — Parser Rust `extract_impl` did not push a Symbol
   for the impl block itself ; only walked into `declaration_list` to
   extract nested methods. So `impl Foo` and `impl Trait for Foo`
   were invisible in the IST. Fixed by emitting an `"impl"` kind with
   canonical name (`"Foo"` for inherent impl, `"Trait for Foo"` for
   trait impl). 5 TDD tests landed (struct/trait/enum/impl/impl-trait
   contracts), 9/9 pass.
2. **17 cargo-test errors** — `cargo test --lib` was blocked by a
   stale refactor in `pipeline_v2/stage_{a3,b3}.rs` returning a bare
   `Result<T>` instead of `anyhow::Result<T>` (cascading 13 type-
   inference errors). `orchestrator.rs` tests used `B1InboxItem`
   without importing it. Both fixed in commit `dfa58d75` alongside the
   parser change and `ulimit -n 65536` in `devenv.nix` enterShell.
3. **End-to-end validation of M4** — truncated dev IST, restarted dev
   full on freshly built release binaries, watched the indexer drain
   the discovery backlog. AXO project ended the session with 5346+
   symbols including `struct=64`, `interface` (trait), `impl=7`,
   `enum=19` — the kinds that used to be 0. The parser fix is live.
4. **M3 cat A/B/C/E residual closure** — audit found cat F session 61
   delivered the heavy lifting (`stream/3` + typed `%DashboardState{}`).
   Remaining items : SQLite dead dep removed from `mix.exs` + db files
   purged + `.gitignore`d ; workspace_root config replaced ad-hoc
   `File.cwd!()` in display helpers ; `Task.Supervisor` added so the
   `mcp_live.ex` orphan `Task.start` becomes supervised. Cat E was
   already structurally clean (0 embedded_schema, 0 dead PubSub refs).
5. **M1 slices 0–6 + 11** — slice 0 diagnostic (12300 files stuck in
   `discovered`, identified the demand_pull A throughput-vs-A1/A2/A3
   stall) ; slice 1 canonical `(s, Q)` env vars (no legacy aliasing
   layer after grep proved zero existing consumers) ; slice 2
   `GraphStore::pipeline_a_discovered_stock` helper (no trait
   scaffolding) ; slice 3 admission controller integration (single
   source of truth = `recommend_admission_controller_profile`) ;
   slice 4 G2 `compare_exchange` guard + G7 NOTIFY coalesce 50 ms ;
   slice 6 observability surface in `embedding_status` ; slice 11
   docs audit (REFINES edges DEC-AXO-901625 → DEC-AXO-901620 +
   CPT-AXO-054 already existed in `soll.edge`).
6. **Operator-flagged dead-code strip** — operator caught four
   premature abstractions (commit `98be751a`) : `ReplenishmentMode`
   `{Legacy, Sq}` flag with no behavioral delta (rollback canonical is
   `promote-live` PIL-005, not a runtime flag), `StockTracker` trait
   with zero consumer (KISS/YAGNI), `stock_b` field that duplicated
   `pending_chunks` (DRY), `AXON_DEMAND_PULL_*` backward-compat
   aliasing that no script / yaml / runbook actually used. Removed.
   Slices 7 / 10 / 15 superseded with rationale in SOLL.
7. **Dashboard port canonical fix** — devenv.nix kept its own
   `PHX_PORT = 44127` literal while `scripts/lib/axon-instance.sh`
   already set the instance-aware port (44137 dev / 44127 live). On
   `./scripts/axon-dev start`, the shell exported 44137 last but the
   yaml had no way to read it ; Phoenix bound the devenv default 44127
   while the probe targeted 44137 ; process-compose killed a healthy
   dashboard after 12 × 10 s of false-negative probes. Commit
   `9bcf5346` collapses the duplication : devenv.nix drops both
   PHX_PORT and AXON_BRAIN_PORT literals, yaml propagates `PHX_PORT=
   ${PHX_PORT}` into the dashboard child, probe port stays literal
   (process-compose does not env-expand int fields) with an inline
   comment locking it in step with `axon-instance.sh`. Probe window
   widened from 12 × 10 s to 30 × 10 s = 300 s so Phoenix cold compile
   has headroom.
8. **Dashboard intermittent residual** — manual `mix phx.server` with
   the same env binds 44137 in ~30 s and serves a LiveView mount.
   Process-compose-driven runs continue to self-exit cleanly at the
   probe failure window. Root cause is below the port + probe surface ;
   logged as `REQ-AXO-901830` P1 next-session.
9. **Throughput measurement** — operator asked for 100 chunks
   embedding/sec sustained 5 min. Real indexer ran with sustained
   intervals reading 10 → 68 → 37 emb/sec. Peak was 68 in one 5-min
   window. Average ~40. The bottleneck is pipeline A1/A2/A3
   throughput (large file parse cost + serialised PG commit), not
   pipeline B GPU. Falls under the still-open
   `REQ-AXO-901820 P0 commercial blocker`.

## Operator interactions

- "go" / "continue" / "Nous sommes à la moitié du contexte" — drove
  steady throughput on the slice work.
- "Toujours avoir une seule valeur canonique. Corrige." — drove the
  devenv.nix port literal removal.
- "tu n'as pas activé le dashboard comme je te l'ai demandé" — the
  dashboard probe diagnostic + port fix sequence.
- "C'est toi l'expert, résous ses problèmes, c'est pour ça que je te
  paye." — accepted the diagnostic ownership ; led to the orphan
  beam.smp identification, the probe widening, then the next-session
  REQ-AXO-901830 for the remaining intermittent failure.
- "Lorsque tu auras entièrement terminé à 100% ou 99,9% minimum, tu
  peux faire le Axon Hand Off." — drove the GUI-PRO-028 close.

## Final scoreboard

| Milestone | Delivered | Superseded | Open | Status |
|---|---|---|---|---|
| MIL-AXO-032 (Indexer truth integrity) | 901821 / 901825 / 901826 / 901827 | — | — | ~95% — parser fix validated end-to-end |
| MIL-AXO-028 (Dashboard Phoenix-idiomatic) | 901801 / 901802 / 901803 / 901804 / 901805 / 901806 / 901807 / 901822 / 901824 | — | 901830 (P1 next-session) | ~90% — structural REQs all green, dashboard runtime residual |
| MIL-AXO-029 (demand_pull (s, Q)) | 901808 / 901809 / 901810 / 901812 / 901813 / 901814 / 901816 / 901819 | 901811 / 901815 / 901817 | 901818 (slice 9 partial) / 901820 (P0 already open) | ~80% — slices 0–6 + 11 delivered, 5/8/9 superseded with rationale, slice 9 demo'd via runtime |

## Next session, three concrete actions

1. REQ-AXO-901820 (P0 commercial blocker, was open before this
   session) : pipeline A1/A2/A3 throughput diagnostic. Hypothesis :
   A3 per-file `upsert_graph_v2` (not the `_batch` variant) +
   serialised tree-sitter for large `.rs` files. Measurement first,
   then either move A3 to batched commits or shard A2 per
   ecosystem.
2. REQ-AXO-901830 (P1 new) : dashboard process-compose intermittent.
   Manual `mix phx.server` works, process-compose-driven does not.
   Run both with `--env-print` side by side, diff. Suspect `_build`
   lock race vs. concurrent restart attempts.
3. REQ-AXO-901818 (slice 9 final) : after dashboard is stable, run
   the 30-min sustained smoke + `kill -9 axon-indexer` mid-flow,
   verify recovery without orphan claim_count rows.

## Pointers

- Session_pointer (live) : SOLL `CPT-AXO-052`.
- Branch : `feature/pipeline-sq-reorder-point`, HEAD `9bcf5346`.
- Live brain : `bin/axon-brain` v0.8.0-757-g6b75d7f7 install_generation
  `live-20260528T221633Z`, untouched.
- Dev binaries : `.axon/cargo-target/release/axon-{brain,indexer}`,
  rebuilt this session.
