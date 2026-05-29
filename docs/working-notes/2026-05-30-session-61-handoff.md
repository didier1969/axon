# Session 61 — Handoff (2026-05-30)

**Branche fin de session :** `feature/pipeline-sq-reorder-point`
**Dernier commit main :** `7d07f81e` (MIL-AXO-028 cat A+B+C+F partial + bugs 901798+901807)
**Working note canonique (audit-only)** — état vivant : SOLL CPT-AXO-052.

## Livraisons commitables (non commité encore)

### MIL-AXO-028 cat F dashboard finalize (F1-F6 complets)

| Sub | Scope | État |
|---|---|---|
| F1-F4 | Brain compose_dashboard_state_v1 + PG composite + telemetry emit | ✅ session 60 + extension session 61 |
| F5 | LiveViews migrate vers `%DashboardState{}` typed struct + `Phoenix.LiveView.stream/3` per_project + dedicated topic `"dashboard:state"` + `local_broadcast` | ✅ session 61 |
| F6 | IndexerHeartbeat + McpPoller modules + supervisor entries + tests + config refs supprimés | ✅ session 61 |

### Phoenix-idiomatic refactor (Phase 12)

- `lib/axon_dashboard/dashboard_state.ex` : 9 sous-structs (Runtime / Embedder / Telemetry / Filesystem / Lifecycle / Totals / PerProjectEntry / PipelineConfig / RuntimeConfig) avec `from_map/1` conversion explicite
- `lib/axon_dashboard/bridge_client.ex` : topic dédié `dashboard:state`, `local_broadcast`, struct conversion before broadcast, TCP buffer accumulator (REQ-AXO-901826 fix fragmentation)
- 3 LiveViews refactored : `pipeline_live.ex` / `mcp_live.ex` / `projects_live.ex` — subscribe topic dédié, pattern match `{:dashboard_state, %DashboardState{}}`, struct atom-keys accessors nil-safe, `Phoenix.LiveView.stream/3` pour per_project table
- Bandeau rouge persistant "Valeurs non validées — REQ-AXO-901827" sur Pipeline + Projects pages
- Valeurs entières brutes partout (suppression de `humanize_int` k/M abrégés)
- `mix compile --warnings-as-errors --no-deps-check` exit 0

### Brain Rust + DDL

- `db/ddl/08_dashboard_state.sql` : `runtime_config_snapshot` table + `dashboard_state_full(ttl)` PG function. **Fix structural** : `08_dashboard_state.sql` ajouté à `postgres/ddl.rs::FILES include_str!` list (omis session 60)
- `src/axon-core/src/runtime_config.rs` : nouveau module — indexer write UPSERT au boot via `write_indexer_config_snapshot/1`. Utilise `LISTEN_CHANNEL` const exposé pub + `env_alias::read_with_alias` + `A3_TO_B1_BUFFER_CAP_DEFAULT` (zero hardcoded duplicate)
- `src/axon-core/src/dashboard_state.rs` : refactor compose lit `axon_runtime.dashboard_state_full(5)` + `latest_lifecycle_heartbeat` + `cached_fs_counters` + `LiveMetrics` struct (vs 30 args). **Fix bug latent session 60** : `extract_first_cell` décode jsonb-as-JSON-string (PG FFI wrap dans `Value::String`)
- `src/axon-core/src/pipeline_v2/notify_listener.rs` : `LISTEN_CHANNEL` exposé `pub const` (canonical)
- `src/axon-core/src/runtime_boot.rs` : call `runtime_config::write_indexer_config_snapshot` au boot indexer
- `src/axon-core/src/main_telemetry.rs` : call site refactor avec `LiveMetrics` struct
- 4 TDD tests inline `dashboard_state::tests` : `extract_first_cell_decodes_jsonb_string_payload` (reproduit le bug latent), `extract_first_cell_passes_native_object_through`, `extract_first_cell_returns_null_on_malformed_shape`, `compose_dashboard_state_v1_envelope_shape`, `compose_dashboard_state_v1_falls_back_gracefully_on_null_pg_state`. **BLOCKED par REQ-AXO-901825** (17 pré-existantes lib test errors)
- Cargo build release brain + indexer exit 0

### Process-compose fix env identity

- `process-compose.dev.yaml` brain block : `AXON_RUNTIME_IDENTITY=axon-dev-axon-brain` + `SHADOW_ROLE=brain` + `STATE_FILE` + `RUN_ROOT` explicits
- `process-compose.dev.yaml` indexer block : symétrique avec `axon-dev-axon-indexer`
- `process-compose.live.yaml` brain + indexer blocks : symétriques avec `axon-live-axon-{brain,indexer}`
- **Fix REQ-AXO-901821** (env leak parent shell)

## SOLL délivrances session 61

| SOLL ID | Type | Statut | Description |
|---|---|---|---|
| MIL-AXO-032 | Milestone | created | Indexer truth integrity restoration — fix 30× sous-indexation + chain session-61 bugs |
| REQ-AXO-901821 | Requirement | RESOLVED | Brain env mis-config (yaml dev+live fix) |
| REQ-AXO-901822 | Requirement | open P1 | Centraliser sources de vérité config runtime (ports, instance_kind) |
| REQ-AXO-901824 | Requirement | open P1 | Brain DDL bootstrap fragile fn_notify_chunk_pending |
| REQ-AXO-901825 | Requirement | open P1 | cargo test --lib bloqué par 17 erreurs préexistantes |
| REQ-AXO-901826 | Requirement | RESOLVED | BridgeClient nested config + TCP fragmentation buffer |
| REQ-AXO-901827 | Requirement | open P0 BLOCKER | Indexer sous-indexe 30× (3 root causes) |

## Bug critique découvert (P0 commercial blocker)

**Indexer sous-indexe massivement**. PG truth confirmée :
- files=14855, symbols=7775, chunks=9267, embeds=15685, edges=117220
- 8 projets sur 25 ont des symbols (17 à zéro)
- AXO project : 860 files → 150 symbols (kind: function:136 + module:14, **AUCUN struct/trait/impl**) vs ~4000 attendus
- Code source AXO mesuré : 3096 fn + 266 struct + 324 trait/enum/impl

**3 root causes empilées** :

1. **Parser Rust extract_struct/trait/impl ne persiste pas** (P0) — `src/axon-core/src/parser/rust.rs:54-61` appelle les extracts mais résultat n'arrive jamais en PG
2. **fd exhaustion** (P0) — `Too many open files (os 24)` × 10+ dans `.axon-dev/run-indexer/errors.2026-05-29-21.log`, 17 projets jamais touchés
3. **Parsers tree-sitter manquants** (P1) — .py .sql .yaml .ts .exs skipped

Bandeau rouge "Valeurs non validées — REQ-AXO-901827" affiché en haut du dashboard tant que ce milestone n'est pas livré.

## Runtime state fin de session 61

| Surface | PID | État |
|---|---|---|
| Live brain | 11054 | Running (`bin/axon-brain`, build_id v0.8.0-757-g6b75d7f7, install_generation live-20260528T221633Z) |
| Live dashboard | 11223 | Running (axon_nexus@127.0.0.1, port 44127) |
| Dev brain | 26488 | Running release binary (post audit fixes), env identity correcte `axon-dev-axon-brain` |
| Dev indexer | 6800 | Running mais sous-indexe (REQ-AXO-901827 ouvert) |
| Dev dashboard | dernier PID | Running, BridgeClient connected, dashboard_state populated |
| PG | 12846 | Running 127.0.0.1:44144 |

`runtime_config_snapshot` row écrite par indexer dev au boot (vérifiée via psql).

## Branch + commit state

- main HEAD : `7d07f81e` (inchangé depuis session 60)
- Branch active : `feature/pipeline-sq-reorder-point`
- Working tree : ~15 fichiers modifiés (cf. git status)
- Pas encore commit — opérateur décide ordre handoff vs commit vs attaque REQ-AXO-901827

## Next-session actions prioritaires

1. **(P0)** Attaque REQ-AXO-901827 — soit `parser Rust persist bug` d'abord (cause #1 : 30× sous-indexation AXO), soit `ulimit fd` d'abord (cause #2 : débloque 17 projets). Recommandé : ulimit d'abord (5 min), puis parser Rust (1-2h).
2. **(P1)** Commit session 61 livraisons : `git add` les 15 fichiers, message structuré REQ-AXO-901826 + MIL-AXO-028 cat F closure + MIL-AXO-032 ouverture
3. **(P1)** REQ-AXO-901825 : déblocage cargo test --lib (17 erreurs préexistantes dans stage_a3/stage_b3/mcp/etc.). Permet aux 4 TDD tests inline `dashboard_state::tests` de s'exécuter
4. **(P2)** REQ-AXO-901822 : centraliser ports config (DRY suppression duplication YAML/runtime.exs/Rust defaults)
5. **(P2)** REQ-AXO-901824 : investiger DDL bootstrap race fn_notify_chunk_pending

## Blockers / known issues

- **Dashboard validation refusée** tant que REQ-AXO-901827 ouvert (bandeau rouge persistant)
- **cargo test --lib bloqué** par REQ-AXO-901825 (P1, dette pré-existante)
- **Live brain non rebuild** depuis fix session 61 — encore old binary v0.8.0-757-g6b75d7f7. Si promote-live requis, faudra repromote.

## Mémoires feedback ajoutées session 61

- `feedback_no_duplicated_values_use_env_vars.md` — opérateur a flag duplication ports + identity dans YAML/runtime.exs/Rust defaults

## Phoenix-idiomatic audit retenu

Mon dashboard pipeline_live.ex / mcp_live.ex / projects_live.ex respecte maintenant :
- ✅ Typed struct `%DashboardState{}` avec atom keys (compile-time validation)
- ✅ Phoenix.PubSub.local_broadcast (in-VM, pas cluster RPC)
- ✅ Dedicated topic `"dashboard:state"` (séparé du legacy `bridge_events`)
- ✅ Pattern match `{:dashboard_state, %DashboardState{} = state}` (struct guard)
- ✅ Phoenix.LiveView.stream/3 pour per_project (DOM stable via dom_id)
- ✅ Nil-safe accessors avec struct guards
- ✅ TCP line-buffered framing (fix REQ-AXO-901826 latent fragmentation)
- ✅ strict compile `--warnings-as-errors` exit 0

Dette restante (P1-P2, hors scope session 61) :
- Phoenix.LiveView.async_result/3 pour mount initial (vs synchrone BridgeClient.dashboard_state)
- GenStage/Broadway pour BridgeClient (vs single-process bottleneck)
- Phoenix.LiveViewTest unit tests Elixir (vs uniquement TDD Rust inline)
- DEC-PRO documenter pattern "PG canonical pour configs static + RAM pour live metrics" (cross-tenant)

## Note méthodologique

Session 61 cat F dashboard refactor a été un cas d'école **« le bug visible n'est pas le bug racine »** :
- Surface visible : dashboard affiche valeurs incohérentes
- Premier diagnostic : bug Phoenix LiveView / template wiring
- Réalité : (1) BridgeClient mismatched config key (nested vs top), (2) TCP fragmentation, (3) jsonb-string FFI wrap, (4) puis fd exhaustion indexer, (5) puis parser Rust incomplet

Chaque couche fixée a révélé la suivante. Le bandeau rouge "Valeurs non validées" est l'engagement à ne pas valider une promesse trompeuse tant que la couche la plus profonde (indexer truth) n'est pas restaurée.
