# Session 128 — DDL lock-free (REQ-AXO-902475) & Mismatch Compute Provider (REQ-AXO-902363)

**2026-09-11T19:50:00+02:00** · HEAD `aace0d1e` · Commit poussé sur `origin/main` · porte `GUI-AXO-1034` **VERTE**.

## 1. Ce que la session a fait

Livraison complète et vérification empirique des deux exigences reportées suite à la décision opérateur (« fais les deux ») :
1. **`REQ-AXO-902475`** : Élimination absolue des verrous bloquants au démarrage/rejeu DDL (lock-free catalogue guards pour tous les `CREATE [UNIQUE] INDEX IF NOT EXISTS` et `DROP INDEX/TRIGGER IF EXISTS`).
2. **`REQ-AXO-902363`** : Propagation du mismatch provider/compute GPU dans `status` (MCP) et `scripts/status.sh` (OVERALL `DEGRADED`), nettoyage devenv des variables superflues `AXON_WATCHMAN_BIN`/`AXON_PGREADY_BIN`, et clôture documentaire de l'enquête de dérive de port 44144.

## 2. Modifications structurelles & validation technique

### Volet DDL Lock-Free (`REQ-AXO-902475`)
- **Nouvelles fonctions catalogue lock-free (`db/ddl/00_extensions.sql`) :**
  - `public.drop_trigger_if_present(p_schema, p_table, p_trigger)`
  - `public.drop_index_if_present(p_schema, p_index)`
  - Ces fonctions inspectent `pg_catalog` en lecture pure avant toute émission de DDL destructif, éliminant les verrous de famine sur le chemin no-op.
- **Conversion exhaustive des index et triggers :**
  - Conversion des 4 `DROP` nus résiduels dans `03_ist_schema.sql`, `15_mailbox.sql`, `16_practice.sql`, `20_mailbox_pubsub.sql`.
  - Conversion des 45 `CREATE [UNIQUE] INDEX IF NOT EXISTS` résiduels en `SELECT public.create_index_if_absent(...)` dans l'ensemble des fichiers DDL (`01`, `02`, `03`, `10`, `11`, `12`, `14`, `15`, `16`, `17`, `18`, `19`, `20`, `21`).
- **Validation empirique (`src/axon-core/src/tests/ddl_lock_tests.rs`) :**
  - Suite de 5 tests unitaires et d'intégration validée à 100% :
    - `canonical_ddl_replay_takes_no_blocking_lock_on_hot_tables` (rejeu complet sous écriture concurrente sans famine).
    - `guards_apply_the_ddl_when_the_object_is_genuinely_absent` (création effective).
    - `every_guarded_index_and_trigger_really_exists_after_bootstrap` (présence des 55 index et 5 triggers).
    - `concurrent_bootstrap_ddl_runs_without_23505_or_deadlock` (concurrence idempotente).
    - `raw_if_not_exists_forms_do_starve_in_the_same_harness` (contrôle négatif de famine).

### Volet Mismatch Compute/Provider & Nettoyage Devenv (`REQ-AXO-902363`)
- **Surface MCP & Runtime (`src/axon-core/src/mcp/`) :**
  - `tools_system.rs` : `embed_provider_compute_mismatch` passée en `pub(crate)`.
  - `tools_framework_runtime_status.rs` :
    - Factorisation de la lecture `latest_lifecycle_heartbeat("indexer")`.
    - `compute_degraded_notes` reçoit `provider_compute_mismatch: bool` et injecte `"provider_compute_mismatch"`.
    - `derive_recovery_action` oriente vers `embedding_status` (`inspect_embedding_provider`).
    - `embedder_runtime_snapshot` expose `provider_compute_mismatch` et `effective_embed_provider`.
    - Nettoyage des avertissements de compilation (`compute_staleness_snapshot` et `ShortlistCandidate`).
  - Suite unitaire `degraded_notes_tests` : 4/4 tests verts (`un_mismatch_compute_provider_degrade_la_verite`).
- **Scripts shell (`scripts/status.sh`, `scripts/start.sh`) :**
  - `status.sh` : évaluation précoce de `runtime-heartbeat.json`, `provider_compute_mismatch` et `AXON_DEAD_BRAIN` avant l'en-tête pour aligner `OVERALL DEGRADED` sur `STATUS DEGRADED` (exit 1), affichage de `FAIL provider_compute_mismatch: ...`.
  - `start.sh` : retrait des exports superflus `AXON_WATCHMAN_BIN` et `AXON_PGREADY_BIN` (résolus par le profil devenv en tête de `PATH`).
  - `tests/shell/test_status_provider_compute_mismatch.sh` : 4/4 tests verts sous fixture.
- **Enquête port 44144 :** Confirmation de l'unification sur 44144 via `AXON_CANONICAL_PG_PORT` (`scripts/lib/axon-pg-port.sh`), `services.postgres.port` (`devenv.nix`), et purge automatique des postmaster.pid stale (`ensure-runtime.sh`).

## 3. Clôture Gouvernance & SOLL

- **`REQ-AXO-902475`** : passée à `delivered`, 7 preuves formelles attachées.
- **`REQ-AXO-902363`** : passée à `delivered`, 5 preuves formelles attachées.
- **`CPT-AXO-052`** : section Session 128 consignée avec traçabilité complète.
- **Contrôles qualité :**
  - `cargo check --lib --bins` : 0 warning.
  - `cargo fmt` & `scripts/ci/axon-fmt-check.sh` : 100% conforme.
  - `axon_pre_flight_check` : Conforme (vert).
  - `axon_commit_work` : Commit `aace0d1e` créé et poussé sur `origin/main`.
