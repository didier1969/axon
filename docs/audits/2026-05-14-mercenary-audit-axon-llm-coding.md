# Audit mercenaire — Axon (proposition de valeur LLM-coding-only)

**Date :** 2026-05-14
**Cible :** `/home/dstadel/projects/axon` — Axon, Structural Intelligence MCP server.
**Client réel du produit :** agents LLM de coding (Claude Code, Codex, Gemini CLI, agents tiers).
**Objectif propriétaire :** maximisation du revenu par maximisation de la valeur ajoutée client.
**Périmètre :** vision → architecture cible → code → runtime → docs → méthodologie → contrats MCP → onboarding LLM → économie tokens → fiabilité.
**Méthode :** 5 sous-agents file/git/PG en parallèle (A docs, B runtime, C git, D MCP+SOLL, E pipeline) + MCP Axon main-thread (status, soll_validate, soll_verify_requirements, anomalies, health, truth_check).
**Contrainte :** aucune solution. Faiblesse + preuve + pourquoi + impact revenu.

---

## 0. Fiabilité de l'audit lui-même (signal pré-audit)

| Signal | Valeur observée | Implication |
|---|---|---|
| `status.trust_boundary` | **degraded** | Brain sert un snapshot ; `inspect`/`query`/`impact` non garantis canoniques (CPT-AXO-029) |
| `status.ist_projection_freshness` | **stale** | Indexeur non-actif au moment du probe |
| `health.files` (scope global) | **0** | Projet apparaît unindexé pour santé globale (mode runtime = `brain_only`) |
| `truth_check.Symbol delta` | 0 (aligné) | Symboles aligned writer/reader malgré File=0 — cohérence interne préservée |
| `anomalies` | 8 heuristic intent gaps, 0 cycles, 0 god objects | Posture structurelle clean |
| `soll_validate` | 35 violations (11 orphans, 22 missing criteria/evidence, 2 decisions unlinked) | Graphe SOLL imparfait |
| `soll_verify_requirements` | done=242, partial=80, missing=15 | 28 % des requirements non clôturés |

**Verdict pré-audit :** l'audit s'exécute sur un snapshot **dégradé** dans le mode opératoire de référence d'Axon (`brain_only`). C'est une faiblesse à part entière (cf. MACRO-2) — l'opérateur lit "trust=degraded" comme état normal, donc accepte un signal dégradé comme baseline.

---

## MACRO — Faiblesses systémiques (vision / proposition / méta-méthodologie)

### MACRO-1 — Le moteur tourne, le tableau de bord n'est pas câblé [P0]

**Angle :** B (contrat MCP) · E (pipeline) · F (proposition commerciale)

**Preuve :**
- `db/ddl/04_graph_functions.sql:287` : `CREATE OR REPLACE FUNCTION public.retrieve_context_v2(...)` — fonction SQL hybride FTS+vector+graph RRF, shippée slice 4 commit `a958ac65`, perf p95 25ms warm (SOLL_EXPORT 2026-05-14).
- `grep -rn "retrieve_context_v2" src/axon-core/src/` → **0 résultat Rust**.
- `mcp/catalog.rs` n'enregistre PAS `retrieve_context_v2` ni `code_search`.
- `~/projects/axon/CLAUDE.md:47` liste pourtant `retrieve_context_v2 (MIL-AXO-017 slice 4 / REQ-AXO-298)` dans la Tool Routing officielle.
- `docs/plans/2026-05-12-axon-hybrid-retrieval-fts-plan.md` : 800 LoC de spec `code_search`, statut "**Proposition** — à logger comme REQ-AXO-NNN après revue".

**Pourquoi ça affaiblit le produit :** le différentiateur stratégique d'Axon vis-à-vis des concurrents (Sourcegraph, Cody, Cursor index) est la **fusion FTS+vector+graph en une seule fonction PG bench-validée**. Mais aucun tool MCP ne l'expose. Un LLM-client qui suit `CLAUDE.md` à la lettre appelle `retrieve_context_v2` → `MethodNotFound`. La proposition de valeur la plus défendable du produit est **invisible** depuis l'interface client.

**Impact revenu :** P0. Le pitch commercial ne survit pas à un test live avec un prospect. Le LLM tombe en `grep -rn` (coût mesuré ~3-8K tokens/appel par le plan §2.2 lui-même), invalidant la promesse "Axon réduit le coût d'exploration".

---

### MACRO-2 — Le produit ne pratique pas sa propre méthodologie [P0]

**Angle :** A (cognition LLM) · C (SOLL) · G (méta-méthodologie)

**Preuve :**
- **CPT-AXO-019 ("documente" → SOLL canonique)** : `~/.claude/CLAUDE.md:26` mappe `documente` → `soll_manager`. `docs/skills/axon-engineering-protocol/SKILL.md:133` mappe la même phrase → `document_intent` (REQ-AXO-141, le différentiateur). `document_intent` est absent de TOUS les 4 CLAUDE.md + MEMORY.md.
- **CPT-AXO-019 originator log** : sur 941 nodes SOLL, **98 portent `Originator:` ou metadata.originator (10,4 %)**. La règle d'auto-log est appliquée sur 1 node sur 10.
- **GUI-PRO-028 step 5 (working-notes audit)** : 23 working-notes accumulés en 13 jours (2026-05-02 → 2026-05-14), 10 nommés `*handoff*`, **0 pruning visible**.
- **`MEMORY.md:14` ↔ `MEMORY.md:32`** se contredisent dans le même fichier (`working-notes = canonical` vs `working-notes = audit-only`).
- **CLAUDE.md axon stale d'au moins 24h** : déclare "Slices 6B + 7 pending" alors que commits `28cf9533`, `a4964284`, `d1e54419` (2026-05-13) ont shippé tout 6B + Gate 7. Pas de hook de mise à jour doc post-merge.
- **`scripts/axon` expose 23 sous-commandes** quand `CLAUDE.md:62` et SKILL.md affichent "4-verb canonical : `start|stop|status|qualify`" — vision marketing contredite par le code.

**Pourquoi ça affaiblit le produit :** Axon se vend comme "Structural Intelligence" et "memory-as-graph". Un audit LLM-client de cohérence vs claims **pénalise** sa propre documentation. Le projet n'honore pas ses propres contrats sur son propre repo de référence. L'opérateur cite explicitement dans MEMORY.md (`feedback_probe_soll_live_before_memory.md`) "très grave d'avoir une information d'aussi mauvaise qualité" — incident vécu, récurrent.

**Impact revenu :** P0. Tout prospect technique qui auditera Axon avec son propre agent LLM verra (a) la contradiction MEMORY.md sur 2 lignes adjacentes, (b) le 90 % de nodes sans signature autonome alors que c'est l'argument commercial principal, (c) la doc d'onboarding désynchronisée du code. Inversion totale de la proposition de valeur perçue.

---

### MACRO-3 — Onboarding coûte 10-35K tokens sur un produit "réducteur de coût LLM" [P0]

**Angle :** A (cognition LLM) · F (commercial)

**Preuve (Agent A.2) :**
| Doc obligatoire | chars | tokens (÷4) |
|---|---|---|
| `~/.claude/CLAUDE.md` global | 4 352 | ~1 088 |
| `~/projects/CLAUDE.md` Nexus | 955 | ~238 |
| `~/projects/axon/CLAUDE.md` | 5 023 | ~1 255 |
| `MEMORY.md` (auto-loaded) | 4 701 | ~1 175 |
| `SKILL.md axon-engineering-protocol` | 26 675 | ~6 668 |
| **Sous-total Tier-1 (pré-MCP)** | **41 706** | **~10 426** |
| + 16 feedback memories ("load BEFORE acting" MEMORY.md:3) | 34 842 | ~8 710 |
| **Sous-total respect mémoire** | **76 548** | **~19 137** |
| + `kickoff_bundle` + 10 CPTs canoniques pull-loaded | ~6-15K | **~25-35K total** |

**Pourquoi ça affaiblit le produit :** Axon est positionné par construction comme "réduction de coût LLM" (SKILL.md, CPT-AXO-018, CPT-AXO-024). Son propre onboarding consomme 10-35K tokens **avant la première action utile**. La procédure GUI-PRO-028 Hand Off mandate des éditions à chaque fin de session, ce qui **invalide le prompt cache 5-min** et oblige un re-prefill plein tarif à la session suivante. Le produit se sabote son propre cache.

**Impact revenu :** P0. Sur Opus 4.x à ~$15/M input non-cached, une session cold-start coûte 0,15-0,50 $ rien qu'en bootstrap. À 100 sessions × 10 développeurs × 22 jours = 22 000 cold-starts/mois → 3 300-11 000 $/mois de surcoût client incompressible **avant** la moindre tâche de coding. Concurrents au boot léger (Sourcegraph Cody, Cursor) plafonnent Axon en TCO perçu.

---

### MACRO-4 — Surface MCP volatile et obèse [P0]

**Angle :** B (contrat MCP) · A (LLM friction)

**Preuve :**
- **59 tools** exposés au LLM-client (catalog.rs).
- **31 commits `fix(mcp)` en 30 jours** (Agent C.7).
- **Rename `cypher` → `sql`** mergé le 2026-05-13 (commit `a4964284`) — surface MCP renommée la veille de l'audit.
- **`retrieve_context` vs `retrieve_context_layered`** : même paramètres, layered = wrapper sur retrieve_context, 1 implémentation, 2 tools.
- **5 chemins d'écriture SOLL** sans matrice de décision : `soll_manager(create)` · `soll_apply_plan` · `document_intent` · `infer_soll_mutation` · `entrench_nuance`.
- **Tools fantômes** dans CLAUDE.md mais absents du catalog : `retrieve_context_v2`, `code_search`.
- **Exemples in-catalog** : 2/60 tools couverts par `tools_help.rs:287-344`. Ratio doc/surface = **3,3 %**.
- **`problem_class`** structuré employé pour 2 sites (`invalid_arguments`, `tool_unavailable`) sur ≥10 nécessaires.

**Pourquoi ça affaiblit le produit :** un LLM-client neuf voit 59 tools dont 7-10 recouvrent les mêmes besoins. Sans exemples, il tâtonne. À chaque round-trip raté, il paie 500-2K tokens. Le `parameter_repair` corrige **après** échec — coût garanti d'au moins 1 retry sur 96,7 % des tools. La rotation rapide des noms (cypher→sql, project_slug→project_code) invalide les mémoires LLM apprises.

**Impact revenu :** P0. Multiplicateur ×2-3 sur les tokens client pendant les 20 premières tâches. NPS LLM-client négatif silencieux. Aucun "wow moment" possible parce que la friction overshoot la valeur dans la fenêtre où le prospect décide.

---

### MACRO-5 — Bugs critiques `current` sur les mutateurs SOLL canoniques [P0]

**Angle :** C (intégrité SOLL) · F (commercial)

**Preuve (Agent D.5) :**
- **REQ-AXO-323** : *"silent UPSERT — data loss without revision trail"* sur `soll_manager.create`. Tag `axon-bug`. Status `current`.
- **REQ-AXO-254** : *"763+ PG errors + soll_apply_plan deadlocks via FFI"*. Tag `axon-bug`. Status `current`.

**Pourquoi ça affaiblit le produit :** `soll_manager.create` et `soll_apply_plan` sont les **deux tools d'écriture SOLL canoniques**. Le pitch "graphe d'intent fiable" repose sur leur intégrité. Tag commercial `commercial-value` confirmé sur ces REQ par les LLM-loggers eux-mêmes. Pas de date d'engagement fix.

**Impact revenu :** P0. Tout PoC enterprise qui pousse de la donnée SOLL à charge réelle plante. Bloque les deals > 50K $. Pire : la "data loss without revision trail" est exactement le scénario qui rend Axon non-conforme aux audits internes SOC2/ISO chez un client enterprise (perte silencieuse = non-traçable = red flag).

---

### MACRO-6 — Dépendance Nix/devenv structurelle exclut le marché enterprise [P0]

**Angle :** D (runtime) · F (commercial)

**Preuve (Agent B.2) :**
- `scripts/setup.sh:78,88,143,150,153` : 5 sous-commandes wrappées dans `devenv shell`.
- `scripts/start.sh:572-593` : peut appeler `sudo systemctl start nix-daemon` ou `sudo /nix/var/nix/profiles/default/bin/nix-daemon --daemon &` **automatiquement, sans confirmation**.
- `scripts/lib/ensure-runtime.sh:35,37,66,68,101,104` : PostgreSQL résolu via glob `/nix/store/*-postgresql-and-plugins-17.*` — chemin Nix store dur dans le boot DB.
- `devenv.nix:79` : `LIBCLANG_PATH = "${pkgs.llvmPackages_18.libclang.lib}/lib"` — toolchain Rust dépend de paths Nix résolus en enter-shell.
- `feedback_build_inside_devenv_shell.md` (MEMORY.md) : "ORT/CUDA dlopen-fail outside devenv shell".

**Pourquoi ça affaiblit le produit :** Nix n'est pas un détail de packaging, c'est partie du contrat d'exécution. `start.sh` lance `sudo nix-daemon` sans interaction — interdit en CI, conteneurs immutables, hôtes managed, image distroless. Customer enterprise sur AlmaLinux/Ubuntu durci avec SecOps : impossible.

**Impact revenu :** P0. TAM amputé. Vente bloquée immédiatement sur tout segment enterprise hors-Nix. Positionnement "MCP server plug-and-play" faux. Concurrents en image OCI standard ramassent les clients allergiques à Nix.

---

### MACRO-7 — Pipeline v2 (north-star ingestion) toujours bloqué WSL2 DXG [P0]

**Angle :** E (pipeline) · F (commercial)

**Preuve (Agent C.2) :**
- **REQ-AXO-289** = 24 commits en 30j, slices S1→S7. **S6b reste bloqué sur WSL2 DXG deadlock** (session-20 handoff).
- 27 CSV `dev-bench-*` / `dev-probe-*` au repo root, non commités, non liés via `soll_attach_evidence`, datés 2026-05-10→05-13.
- Claims perf (`+11% mean ch/s ORT memory_pattern`) non reproductibles depuis ces CSV résiduels.

**Pourquoi ça affaiblit le produit :** sans pipeline v2 stable en live, **aucune fiche de performance défendable**. Aucun bench sustained ingéré, archivé, lié à un REQ. Les benchmarks ne deviennent jamais argumentaire commercial.

**Impact revenu :** P0. Le produit ne peut pas publier sa courbe `chunks/s @ N files @ M chars` comme un acquéreur l'attend. La preuve de valeur "real-time structural intelligence" reste un slogan tant que S6b n'est pas débloqué.

---

### MACRO-8 — Discipline opérationnelle visible faible (artefacts résiduels) [P1]

**Angle :** D (runtime) · F (perception qualité)

**Preuve :**
- `git status` racine repo (2026-05-14) : **31 fichiers `dev-bench-*` / `dev-probe-*` untracked** + 10 working-notes untracked + `.devenv/state/postgres/` untracked + `docs/archive/db-backups/`.
- `.gitignore` racine ne couvre pas ces patterns.
- 3 fichiers `M` tracked : repo **non-releasable** au moment de l'audit (pre-flight `preflight.sh:82-87` refuse).
- `bin/axon-core` (62MB) coexiste avec `bin/axon-brain` + `bin/axon-indexer` = 189 MB pour 3 binaires.
- `.axon/live-release/` : `pending.aborted-*.json` × 3 en 11 jours (taux abort ~27 % extrapolé sur le repo de référence).
- `docs/plans/` : **180 fichiers**, 178 antérieurs au 2026-04-30, jamais purgés alors que CLAUDE.md:58 mandate "Planning → SOLL tools. **Never** standalone markdown plans".
- `docs/architecture/` : 26 fichiers, dernier 2026-04-18, **antécèdent toutes les architectures actives** (pipeline v2, AGE retirement, DuckDB purge).
- `docs/adr/` : **vide** (0 fichier).

**Pourquoi ça affaiblit le produit :** un LLM-client qui fait `ls` ou `tree` au boot voit 31 CSV cryptiques + 180 plans périmés. Signal qualité agrégé : projet pas mature. Bench-bots qui font `git add -A` accidentellement committent l'arsenal. Aucune politique de rotation `.axon/live-release/history/` (71 manifests).

**Impact revenu :** P1. Trust faible visuel dès le premier contact. NPS prospect technique fortement négatif sur l'hygiène avant même l'évaluation fonctionnelle.

---

### MACRO-9 — Méta-méthodologie : règles non-enforcables [P1]

**Angle :** A (cognition LLM) · G (méta-méthodologie)

**Preuve (Agent A.7) :**
- **"No sub-agents in Axon"** = règle dure répétée 4× (CLAUDE.md axon:54-58, MEMORY.md:44, SKILL.md:222, feedback memory). Coût documenté : 100-200K tokens par invocation.
- **Skills installés par défaut qui invoquent sub-agents** : `dispatching-parallel-agents`, `subagent-driven-development`, `idea-to-delivery` (+ 4 alias `feature-delivery`, `consensus-driven-delivery`, `concept-to-delivery`), `multi-agent-patterns`, `executing-plans`. **Aucun n'a de disclaimer "DO NOT use in Axon repo".**
- **`cargo build --release` outside devenv** : `~/projects/axon/CLAUDE.md:13-18` montre la commande **sans** wrap devenv. MEMORY.md `feedback_build_inside_devenv_shell.md` mandate le wrap. LLM copie-colle → build cassé.
- **DuckDB workspace removal** et **AGE_READ default flip** cités en stop-list `~/.claude/CLAUDE.md:5` alors que DuckDB est purged (REQ-AXO-271) et AGE est retiré (MIL-AXO-017 Gate 7 closure).

**Pourquoi ça affaiblit le produit :** les règles vitales vivent en prose dans 3-4 docs concurrents au lieu d'être encodées dans le contrat exécutoire (skill descriptions, tool gates, env vars). Un opérateur qui tape `/idea-to-delivery` sur Axon brûle 100-200K tokens **comme prévu et documenté** par MEMORY.md. La règle existe en mémoire morte mais pas en mémoire vive (runtime).

**Impact revenu :** P1. Coût direct opérateur récurrent. Plus subtil : un prospect qui veut "voir Axon en action" via une cmd standard subira l'incident — démo cassée.

---

### MACRO-10 — Pivot architectural récent ; SOLL canonique encore jeune [P2]

**Angle :** G (méta-méthodologie) · F (commercial)

**Preuve (Agent C.4) :**
- `docs/archive/v1.0/` + `docs/archive/v2/` = 2 refontes ARCHITECTURE.md/CONTRACT.md/SPEC complètes.
- `docs/archive/soll-exports/` : ~80 dumps `SOLL_EXPORT_*` sur 36h (2026-03-30 → 2026-04-01).
- `docs/archive/working-notes/2026-04-01-reprise-handoff.md` + `reality-first-stabilization-handoff.md` : 2e reprise en 6 semaines.
- SOLL n'est devenu canonique qu'en avril 2026.

**Pourquoi ça affaiblit le produit :** la cristallisation produit a moins de 60 jours. Les claims architecturaux sont post-hoc. Risque de 3e pivot non-nul.

**Impact revenu :** P2. Limite la story "produit mature". Pas bloquant à court terme mais plafond crédibilité commerciale.

---

## MESO — Faiblesses architecturales et méthodologiques par sous-système

### MESO-A · MCP surface

| ID | Faiblesse | Crit | Preuve | Impact revenu |
|---|---|---|---|---|
| MA-1 | 59 tools, recouvrement `query`/`inspect`/`retrieve_context`/`retrieve_context_layered` sans matrice de décision | P0 | `mcp/catalog.rs:53-876` ; Agent D.1 | LLM-client tâtonne, ×2-3 tokens onboarding |
| MA-2 | 5 chemins d'écriture SOLL non documentés (`soll_manager` / `soll_apply_plan` / `document_intent` / `infer_soll_mutation` / `entrench_nuance`) | P0 | catalog.rs ; Agent D.1 | "documente" routé sur tool différent selon la doc consultée (cf MACRO-2) |
| MA-3 | Tools fantômes `retrieve_context_v2` et `code_search` dans CLAUDE.md, absents du catalog | P0 | CLAUDE.md:47 ; Agent E.5 | Premier appel = 404 sur le différentiateur produit |
| MA-4 | Exemples `tools_help.rs` couvrent 2/60 tools (3,3 %) | P0 | `mcp/tools_help.rs:287-344, 342` | LLM devine arguments → parameter_repair systématique = 1 retry minimum |
| MA-5 | `problem_class` structuré pour 2 cas sur ≥10 nécessaires | P1 | `mcp/dispatch.rs:158-172` ; Agent D.8 | Agent ne peut pas switcher sur erreurs → re-essai aveugle |
| MA-6 | Pas de `pipeline_status` tool exposant `StageSnapshot` (A1..B3 in/out/err/bp/mean_dur) | P1 | `pipeline_v2/metrics.rs` ; Agent E.8 | LLM aveugle au pipeline → 3-5 round-trips diagnostiques |
| MA-7 | `retrieve_context.degraded_reason` enfoui dans `data.*` sans `isError=true` | P1 | `mcp/tools_context.rs:251,517` ; Agent E.7 | LLM produit du code sur index pourri sans signal fort |
| MA-8 | Pas de timestamp `data_freshness` dans réponses `retrieve_context` | P1 | Agent E.7 | LLM ne peut pas raisonner staleness fichier vs index |
| MA-9 | Strings d'erreur `mcp/tools_soll/*` recommandent `cypher SELECT` (tool renommé `sql` slice 6B Phase F) | P1 | `mcp/tools_soll/workflow_project.rs:1139,1143`, `manager.rs:35,97` | LLM suit le hint → tool 404 |
| MA-10 | `tools_context.rs` à 4 293 LoC pour 1 tool + 1 wrapper | P2 | Agent D.9 | Concentration LOC, refactor risqué, bus-factor élevé |

### MESO-B · SOLL integrity

| ID | Faiblesse | Crit | Preuve | Impact revenu |
|---|---|---|---|---|
| MB-1 | REQ-AXO-323 silent UPSERT data-loss `current` | P0 | `psql tags=axon-bug` ; Agent D.5 | Démo enterprise plante ; non-conformité SOC2/ISO |
| MB-2 | REQ-AXO-254 deadlock `soll_apply_plan` + 763 PG errors `current` | P0 | Agent D.5 | Tool d'écriture canonique inutilisable à charge réelle |
| MB-3 | 312/941 nodes (33,2 %) en statut non-canonique (`active`, `accepted`, `completed`, `open`, `archived`, `draft`, `in_progress`) | P1 | constraint `soll_node_status_canonical` `NOT VALID` ; Agent D.5 | `soll_work_plan` retourne du bruit, filtre status='current' insuffisant |
| MB-4 | 272/397 Requirements AXO sans `VERIFIES` (couverture validation 25 %) | P1 | `psql edges WHERE relation_type='VERIFIES'` ; Agent D.7 | Pitch "graphe intent vérifié" tient à un quart |
| MB-5 | 56 orphelins (5,9 %) dont 20 Milestones + 11 REQ + 14 Concept | P1 | `soll_validate` ; Agent D.7 | Traçabilité projet-niveau cassée |
| MB-6 | 89,6 % des nodes sans signature `Originator:` (10,4 % couverture CPT-AXO-019) | P1 | `psql description ILIKE '%Originator:%' OR metadata?'originator'` ; Agent D.6 | Métrique "% intent autonomes" non mesurable = pitch proactivité LLM tombe |
| MB-7 | 15 fixtures-leak permanentes (REQ-AXO-90001..09, 9001, 900, 901, BKS-90001) dont 8 en `current`/`delivered` | P2 | Agent D.5 | Pollution graphe canonique, démo risquée |
| MB-8 | 78 Decisions AXO en `current` pour 2 `delivered` | P2 | Agent D.4 | Decisions jamais explicitement livrées → traçabilité décision→delivery floue |
| MB-9 | 37 Validations en `planned` non délivrées | P2 | Agent D.4 | Gap test-coverage manifeste |
| MB-10 | 22 REQ sans criteria/evidence (`soll_validate`) | P2 | `soll_validate` output | Backbone "intent verifiable" troué |

### MESO-C · Pipeline v2 & Indexation

| ID | Faiblesse | Crit | Preuve | Impact revenu |
|---|---|---|---|---|
| MC-1 | Drops A3→B1 silencieux sur `try_send`, jamais comptés dans `metrics::record_backpressure_block` | P0 | `pipeline_v2/stage_a3.rs:212` ; `metrics.rs:20-21,72` ; `worker_pool.rs:74` | LLM ne distingue pas "B2 lent" de "B2 mort + A3 dépose" |
| MC-2 | `DbWriteTask::ExecuteCypher` zombie : variant toujours dispatché en prod (`main_telemetry.rs:460`) avec branches `panic!` | P1 | `worker.rs:40,699,756` ; Agent E.4 | Crash silencieux write thread → data loss latent |
| MC-3 | `b1_cold_start_poll` filet de sécurité devenu régime permanent | P1 | `stage_b1.rs:60-92` ; `mod.rs:33` | Le pipeline annoncé "streaming" est en réalité "polling toutes les 30s" |
| MC-4 | Pipeline v2 REQ-AXO-289 S6b bloqué WSL2 DXG deadlock | P0 | Agent C.2 ; session-20 handoff | Aucun bench live défendable |
| MC-5 | `embedding_status` n'expose pas drops, GPU util, VRAM, throughput dérivé, last B2/B3 error | P1 | `mcp/tools_system.rs:305` ; Agent E.8 | LLM ne distingue pas "GPU saturé" / "GPU mort" / "indexer down" |
| MC-6 | Mode `brain_only` = posture de référence avec `freshness=stale`, `trust=degraded`, File=0 | P1 | `status` mode verbose (audit live) | Lecture IST non-canonique acceptée comme baseline opérationnelle |
| MC-7 | Mono-vendor NVIDIA (TensorRT + CUDA, pas de ROCm/MPS) | P2 | `embedder/gpu_backend.rs:110-118` | Perte marché Apple Silicon (~30 % dev seats) |
| MC-8 | `unsafe impl Send/Sync for GpuB2Embedder` sans runtime assert `AXON_B2_WORKERS=1` | P2 | `pipeline_v2/embedder_gpu.rs:47-48` | UB si opérateur scale workers |
| MC-9 | Modèle `BAAI/bge-large-en-v1.5` hardcodé, pas de model registry | P3 | `embedding_contract.rs:3` | Migration modèle = chantier multi-semaine |
| MC-10 | Co-existence v2 + legacy `vector_pipeline_3stages.rs` (50 kB) + `vector_worker_loop.rs` (23 kB) sans feature flag | P2 | `src/axon-core/src/embedder/` | Duplication, surface d'attaque bugs |

### MESO-D · Runtime & Release

| ID | Faiblesse | Crit | Preuve | Impact revenu |
|---|---|---|---|---|
| MD-1 | 4-verb vision (DEC-AXO-060) vs 23 sous-commandes `scripts/axon`, 14 flags `start.sh`, 71 env vars `AXON_*` | P1 | `scripts/axon:13-39` ; `start.sh:374-435,30-460` ; Agent B.1 | Marketing contredit par exécutable → trust érodé 2e session |
| MD-2 | Recovery procedure `stop --hard ; start --brain-only` = 2 commandes shell, pas scriptées, pas testées e2e | P1 | `~/.claude/CLAUDE.md:15` ; absence `scripts/axon recover` ; Agent B.3 | Auto-recovery illusoire en cas dégradé |
| MD-3 | Promote `pending.aborted-*` × 3 en 11 jours (~27 % taux abort extrapolé) | P1 | `.axon/live-release/pending.aborted-*.json` ; Agent B.4 | Promote automatique nocturne non-viable |
| MD-4 | `current.json.qualification.evidence: []` malgré promesse SKILL.md:166 | P1 | mesuré 2026-05-14 ; Agent B.4 | Audit-trail manquant pour customer compliance |
| MD-5 | Copie bin/ non-atomique POSIX (`shutil.copy2` sans `rename`) | P2 | `promote_live.sh:305-306` ; Agent B.4 | État partiel possible si crash promote |
| MD-6 | Pas de rollback automatique sur post-check fail dans promote | P2 | `promote_live.sh:336-349` ; Agent B.4 | Humain requis pour rollback explicite |
| MD-7 | 4 canaux diagnostic concurrents (MCP / `axon status` / curl HTTP / heartbeat) sans `axon diagnose` unifié | P2 | Agent B.5 | LLM-client doit choisir lequel selon état mental |
| MD-8 | `start.sh` peut `sudo systemctl start nix-daemon` ou `sudo nix-daemon --daemon &` sans interaction | P0 | `start.sh:572-593` | Sécurité enterprise rompue (sudo silencieux), CI / conteneurs immutables impossibles |
| MD-9 | Contradiction CLAUDE.md (freshness gate = bloquant) vs SKILL.md:36 (freshness "does NOT gate any tool") | P2 | CLAUDE.md axon:11 ↔ SKILL.md:36 ; Agent B.7 | Contrat interne incohérent |
| MD-10 | Pas de politique rotation `.axon/live-release/history/` (71 manifests) | P3 | mesuré 2026-05-14 | Disque pollué long-terme |

### MESO-E · Documentation & onboarding

| ID | Faiblesse | Crit | Preuve | Impact revenu |
|---|---|---|---|---|
| ME-1 | "documente" → 2 tools différents selon le doc consulté (`soll_manager` vs `document_intent`) | P0 | CLAUDE.md:26 ↔ SKILL.md:133 ; Agent A.P0-1 | Différentiateur produit invisible |
| ME-2 | `MEMORY.md:14` ↔ `MEMORY.md:32` se contredisent (working-notes canonical vs audit-only) | P0 | même fichier ; Agent A.P0-2 | LLM-client surcharge contexte sur 23 fichiers |
| ME-3 | Onboarding 10-35K tokens avant 1ère action utile | P0 | Agent A.2 | Sabotage propre-cache-5min, multiplicateur coût client (cf MACRO-3) |
| ME-4 | 6+ skills sub-agent-first installés sans garde "DO NOT use in Axon" | P0 | system-reminder skills ; MEMORY.md:44 ; Agent A.P0-3 | Brûle 100-200K tokens par invocation accidentelle |
| ME-5 | CLAUDE.md axon stale 24h+ vs réalité MIL-AXO-017 (Gate 7 shipped) | P2 | commits `28cf9533`, `a4964284`, `d1e54419` ; Agent C.6 | Anti-signal prospect ; sous-estime maturité produit |
| ME-6 | CPT-AXO-029 référencée `~/projects/axon/CLAUDE.md:11` mais absente de la liste canonique MEMORY.md:35 | P1 | grep ; Agent A.P1-4 | Friction onboarding sur invariant le plus critique (IST trust gate) |
| ME-7 | 180 plans `docs/plans/`, 178 non touchés en 30j ; aucune politique de purge | P2 | `find docs/plans -mtime +30 \| wc -l` ; Agent A.4 | Bruit `Read`/`Grep` exploratoire, dette qui s'accumule |
| ME-8 | 26 fichiers `docs/architecture/`, dernier 2026-04-18 (antécèdent pipeline v2, AGE retirement, DuckDB purge) | P2 | `ls -lt docs/architecture/` ; Agent A.4 | Diagrammes obsolètes ; `visualize-nexus-pull.html` montre encore `AGE vertex` |
| ME-9 | `docs/adr/` vide | P2 | `ls docs/adr/` | Décisions hors SOLL = aucune trace pour LLM externe sans MCP |
| ME-10 | 16 feedback memories chargées "BEFORE acting" = 8 710 tokens, dont 2 dupliqués sur devenv shell | P2 | MEMORY.md:3 ; `feedback_axon_build_in_devenv_shell.md` + `feedback_build_inside_devenv_shell.md` ; Agent A.4 | Doublons = invalidation prompt cache 5-min systématique |
| ME-11 | SKILL.md axon-engineering-protocol : 26 675 chars / ~6 668 tokens, 66 IDs opaques sans annotation inline, cellules 600-730 chars | P1 | mesuré ; Agent A.6 | Charge attentionnelle élevée → erreurs parsing LLM |
| ME-12 | Working-notes : 23 fichiers en 13j, 10 `*handoff*`, 0 pruning visible | P1 | `ls docs/working-notes/` ; Agent A.4, Agent C.3 | "Hand Off step 5" mandate audit, non-respecté |
| ME-13 | Emojis (`🛡️ 🤖 📦 ⚠️`) dans CLAUDE.md projects + axon ; truffés dans working-notes "actionable" | P3 | `~/projects/CLAUDE.md:5,11,17` ; CLAUDE.md axon:10 | Violation CPT-AXO-024 (LLM-only doc methodology) — esthétique humaine |
| ME-14 | DuckDB / AGE_READ encore dans stop-list `~/.claude/CLAUDE.md:5` alors que purged/retired | P2 | `feedback_duckdb_fully_retired.md` ; MIL-AXO-017 ; Agent A.P1-5 | Faux blocking dialogs → "véritable arnaque prompt-cache" cité par opérateur |

---

## MICRO — Faiblesses ponctuelles

### Code / runtime

| ID | Faiblesse | Crit | Preuve |
|---|---|---|---|
| µ-1 | `let _ = b1_inbox_tx.try_send(cid.clone())` — résultat ignoré, pas de télémétrie | P0 | `src/axon-core/src/pipeline_v2/stage_a3.rs:212` |
| µ-2 | `DbWriteTask::ExecuteCypher` dispatché en prod, branches `panic!` | P1 | `src/axon-core/src/main_telemetry.rs:460` ; `src/axon-core/src/worker.rs:699,756` |
| µ-3 | Strings d'erreur SOLL recommandent `cypher SELECT` (tool renommé `sql`) | P1 | `src/axon-core/src/mcp/tools_soll/workflow_project.rs:1139,1143` ; `mcp/tools_soll/manager.rs:35,97` |
| µ-4 | Commentaire `// AGE Cypher (preferred)` post-retirement | P2 | `src/axon-core/src/graph.rs` (2 occurrences) |
| µ-5 | `diagnose_indexing` mentionne `AXON_AGE_READ` (flag mort post 6B Gate 7) | P2 | `src/axon-core/src/mcp/tools_governance.rs:739` ; `tools_risk.rs:176` |
| µ-6 | `tools_help.rs:342` : `_ => json!([])` — 58 tools sur 60 sans exemples | P0 | `src/axon-core/src/mcp/tools_help.rs:287-344, 342` |
| µ-7 | `unsafe impl Send/Sync for GpuB2Embedder` sans assert worker count | P2 | `src/axon-core/src/pipeline_v2/embedder_gpu.rs:47-48` |
| µ-8 | `orchestrator.rs` 472/748 LoC = tests inline (63 %) | P3 | `src/axon-core/src/pipeline_v2/orchestrator.rs` |
| µ-9 | `start.sh:1098-1126` : `readiness_kind` lu avec `2>/dev/null || true` fallback "rising" sans signal LLM | P2 | `scripts/start.sh:1098-1126` |
| µ-10 | `start.sh:556-569` retry post-Nix-fail = `devenv gc` puis abandon hard | P2 | `scripts/start.sh:556-569` |
| µ-11 | `stop.sh:415-417` : si `bin/axonctl` absent → exit 1 (recovery échoue avant de commencer) | P1 | `scripts/stop.sh:415-417` |
| µ-12 | `promote_live.sh:332-335` commentaire avoue `60s → 120s` empirique post-timeout | P2 | `scripts/release/promote_live.sh:332-335` |
| µ-13 | Constraint `soll_node_status_canonical` `NOT VALID` (33 % nodes hors enum) | P1 | `\d soll.Node` ; Agent D.5 |
| µ-14 | `docs/working-notes/2026-05-11-axon-methodology-delivery-spec.md` (26K chars) = spec post-doc pour `axon_apply_methodology_bundle` déjà shipped REQ-AXO-276 | P2 | Agent A.4 |
| µ-15 | REQ-AXO-288 reverté 25 min après merge (commit `0ee86120` revert de `4837bd74`, 2026-05-12) | P2 | git log |
| µ-16 | REQ-AXO-194 Bug 2 chaîne v1→v2→v3→v4 + revert le même jour 2026-05-06 | P2 | git log ; Agent C.2 |
| µ-17 | 80 dumps `SOLL_EXPORT_*` archivés sur 36h (2026-03-30→04-01) | P3 | `docs/archive/soll-exports/` ; Agent C.4 |
| µ-18 | 8 sessions de handoff numérotées non-monotones (session 14, 19, 20, 22, 27) | P2 | `docs/working-notes/session-*` ; Agent C.3 |
| µ-19 | `bin/axon-core` + `bin/axon-brain` + `bin/axon-indexer` = 189 MB pour 3 binaires presque identiques | P3 | `ls -la bin/` ; Agent B.7 |
| µ-20 | `feedback_axon_build_in_devenv_shell.md` (4-may, 2 533 chars) + `feedback_build_inside_devenv_shell.md` (9-may, 2 469 chars) = doublon | P2 | MEMORY.md ; Agent A.4 |

### Contrat textuel / documentation

| ID | Faiblesse | Crit | Preuve |
|---|---|---|---|
| µ-21 | Cell SKILL.md ligne 82 = 730 chars pour `soll_manager` recovery (6 catégories d'erreur en 1 cellule) | P2 | `docs/skills/axon-engineering-protocol/SKILL.md:82` |
| µ-22 | SKILL.md ligne 76 = 366 chars sur 1 cellule (`soll_attach_evidence` 7 sous-champs + 8 formats) | P2 | `docs/skills/axon-engineering-protocol/SKILL.md:76` |
| µ-23 | SKILL.md référence `CPT-PRO-004/005/006/007` (4 IDs jamais annotés) | P2 | `docs/skills/axon-engineering-protocol/SKILL.md:12` |
| µ-24 | "MIL-AXO-017 slice 6B Phase F" dupliqué dans SKILL.md (l.8, l.62) | P3 | `docs/skills/axon-engineering-protocol/SKILL.md:8,62` |
| µ-25 | SKILL.md section "Maintenance" pointe sur GUI-PRO-028 → récursion sans contenu inline | P2 | `docs/skills/axon-engineering-protocol/SKILL.md:228-229` |
| µ-26 | `~/projects/axon/CLAUDE.md:13-18` commande `cargo build --release` sans wrap devenv (viole hard rule MEMORY.md:53) | P1 | `~/projects/axon/CLAUDE.md:13-18` ↔ MEMORY.md:53 |
| µ-27 | `docs/plans/2026-04-22-graph-first-simplification-plan.md` orphelin : graph-first déjà implémenté (CPT-AXO-054) | P3 | Agent A.4 |
| µ-28 | `docs/plans/2026-04-29-tensorrt-ready-vector-pipeline-redesign.md` orphelin : TensorRT shipped | P3 | Agent A.4 |
| µ-29 | `docs/plans/2026-04-09-nco-*.md` × 3 fichiers : branche NCO jamais citée dans MEMORY.md / SKILL.md | P3 | Agent A.4 |
| µ-30 | `visualize-nexus-pull.html` (2026-05-13) diagram Mermaid montre encore `AGE vertex` / `SQL+AGE` | P2 | `docs/architecture/visualize-nexus-pull.html` ; Agent E.4 |

---

## Table synthèse priorisée (revenu-first)

| # | Niveau | Crit | Angle | Faiblesse 1-ligne | Preuve clé | Impact revenu |
|---|---|---|---|---|---|---|
| 1 | MACRO-1 | P0 | E·F·B | `retrieve_context_v2` & `code_search` annoncés dans CLAUDE.md, absents du catalog MCP | `mcp/catalog.rs` / `db/ddl/04_graph_functions.sql:287` | Différentiateur produit invisible. Pitch FTS+vector+graph mort au premier appel |
| 2 | MACRO-2 | P0 | A·C·G | Le produit ne pratique pas sa propre méthodologie (CPT-AXO-019 à 10 %, 4 docs auto-contradictoires) | MEMORY.md:14 ↔ MEMORY.md:32 ; 98/941 originator | Auto-audit prospect = échec immédiat |
| 3 | MACRO-3 | P0 | A·F | Onboarding 10-35K tokens sur produit "réducteur LLM coût" | Agent A.2 | 3 300-11 000 $/mois surcoût client à 100 sessions/j × 10 dev |
| 4 | MACRO-4 | P0 | B·A | 59 tools, recouvrements, exemples 3,3 %, surface volatile (31 fix mcp/30j) | catalog.rs ; tools_help.rs:342 | ×2-3 tokens client onboarding |
| 5 | MACRO-5 | P0 | C·F | REQ-AXO-323 silent data-loss + REQ-AXO-254 deadlock sur mutateurs SOLL canoniques en `current` | tags=axon-bug | Deals enterprise > 50K $ bloqués ; non-conformité audit |
| 6 | MACRO-6 | P0 | D·F | Dépendance Nix/devenv structurelle + `sudo nix-daemon` silencieux | scripts/start.sh:572-593 | TAM enterprise hors-Nix amputé |
| 7 | MACRO-7 | P0 | E·F | Pipeline v2 S6b bloqué WSL2 DXG ; 27 CSV bench non capitalisés | Agent C.2, C.5 | Aucune fiche perf défendable |
| 8 | MC-1 / µ-1 | P0 | E | Drops A3→B1 silencieux (`stage_a3.rs:212` try_send sans télémétrie) | code | LLM ne distingue pas "lent" de "B2 mort" |
| 9 | MA-3 / MA-4 | P0 | B | 96,7 % des tools MCP sans exemples ; tools fantômes dans CLAUDE.md | `tools_help.rs:342` | parameter_repair systématique = retry minimum 1 par tool |
| 10 | MD-8 | P0 | D | `start.sh` exécute `sudo systemctl start nix-daemon` sans confirmation | `start.sh:572-593` | Sécurité enterprise rompue |
| 11 | ME-1 | P0 | A | "documente" mappe sur `soll_manager` (CLAUDE.md) vs `document_intent` (SKILL.md) | CLAUDE.md:26 ↔ SKILL.md:133 | Différentiateur REQ-AXO-141 invisible |
| 12 | ME-2 | P0 | A | MEMORY.md s'auto-contredit (lignes 14 vs 32) | même fichier | LLM-client surcharge 23 fichiers working-notes |
| 13 | ME-4 | P0 | A | 6+ skills sub-agent-first installés sans garde Axon | system-reminder | 100-200K tokens par invocation accidentelle |
| 14 | MACRO-8 | P1 | D·F | 31 CSV résiduels + 10 working-notes untracked + 3 `pending.aborted` en 11j (~27 %) | git status ; .axon/live-release/ | Trust qualité agrégat |
| 15 | MACRO-9 | P1 | A·G | "No sub-agents" non-enforcable, `cargo build` instruit hors devenv (viole hard rule) | CLAUDE.md axon:13 vs MEMORY.md:53 | Coût direct opérateur récurrent |
| 16 | MB-1..MB-6 | P0-P1 | C | Backbone SOLL dénaturé (33 % statuts non-canoniques, 25 % VERIFIES coverage, 10 % originator) | psql | Pitch "intent vérifié" creux |
| 17 | MC-2 | P1 | E | `DbWriteTask::ExecuteCypher` panic en prod | main_telemetry.rs:460 ; worker.rs:699,756 | Data loss latent write thread |
| 18 | MC-5 / MA-6 | P1 | B·E | `embedding_status` aveugle (pas de drops, GPU util, throughput, last err) ; pas de `pipeline_status` | tools_system.rs:305 | LLM 3-5 round-trips diagnostiques |
| 19 | MA-7 / MA-8 | P1 | B·E | `retrieve_context` ne flag pas `isError=true` dégradé ; pas de `data_freshness` | tools_context.rs:251,517 | Code généré sur index pourri = churn client final |
| 20 | MA-9 / µ-3 | P1 | B | Strings recommandent `cypher SELECT` post-rename `sql` | mcp/tools_soll/* | LLM 404 |
| 21 | MD-1 | P1 | D | 23 sous-commandes + 14 flags + 71 env vars vs "4-verb canonical" annoncé | `scripts/axon`, `start.sh` | Marketing/exécutable divergent |
| 22 | MD-2 | P1 | D | Recovery `stop --hard ; start --brain-only` non scriptée, non testée e2e | absence `scripts/axon recover` | Auto-recovery illusoire |
| 23 | MD-3 | P1 | D | Promote `pending.aborted` × 3 / 11j (~27 % extrapolé) | .axon/live-release/ | Promote nocturne non-viable |
| 24 | MD-4 | P1 | D | `current.json.qualification.evidence: []` vs promesse SKILL.md:166 | mesuré | Audit-trail manquant compliance |
| 25 | ME-11 | P1 | A | SKILL.md 6 668 tokens, 66 IDs opaques, cellules 600-730 chars | mesuré | Charge attentionnelle élevée |
| 26 | ME-12 | P1 | A·C | Working-notes 23 fichiers/13j, 10 handoffs, 0 pruning | `ls docs/working-notes/` | Hand Off step 5 non-respecté |
| 27 | MC-3 | P1 | E | `b1_cold_start_poll` 30s filet devenu régime permanent | mod.rs:33 | Pipeline "streaming" devient "polling" |
| 28 | MC-6 | P1 | E | `brain_only` = mode de référence avec freshness=stale/trust=degraded/File=0 | status verbose live | Lecture IST dégradée acceptée comme baseline |
| 29 | µ-26 | P1 | A·D | CLAUDE.md axon:13 propose `cargo build` hors devenv (viole hard rule) | direct | Build cassé sur copy-paste |
| 30 | MACRO-10 | P2 | G·F | 2 pivots architecturaux en 6 semaines ; SOLL canonique <60j | docs/archive/{v1.0,v2,soll-exports} | Story "produit mature" limitée |
| 31 | MD-7 | P2 | D | 4 canaux diagnostic concurrents, pas de `axon diagnose` unifié | Agent B.5 | Tier-1 support consommé |
| 32 | ME-5 | P2 | A | CLAUDE.md axon stale 24h+ vs MIL-AXO-017 réalité | commits 2026-05-13 | Anti-signal prospect |
| 33 | MC-7 / MC-8 | P2 | E | Mono-NVIDIA + `unsafe impl Send/Sync` sans assert workers | gpu_backend.rs ; embedder_gpu.rs:47 | Perte Apple Silicon ~30 % seats |
| 34 | ME-7 / ME-8 / ME-9 | P2 | A | 180 plans/26 archi/0 ADR — doc rot massif | `docs/{plans,architecture,adr}/` | Bruit Read/Grep cumulé |
| 35 | MB-7 | P2 | C | 15 fixtures-leak SOLL en `current`/`delivered` | psql | Pollution graphe |
| 36 | MD-5 / MD-6 | P2 | D | Atomicité bin/ non-POSIX ; pas de rollback auto | promote_live.sh:305,336 | État partiel possible |
| 37 | ME-13 / ME-14 | P3 | A | Emojis dans docs LLM-only ; stop-list DuckDB/AGE_READ obsolète | ~/projects/CLAUDE.md ; ~/.claude/CLAUDE.md:5 | Violation CPT-AXO-024, faux blocking |
| 38 | MD-10 / µ-19 | P3 | D | Pas de rotation history (71 manifests) ; 189 MB pour 3 binaires | bin/ ; .axon/live-release/history | Disque + storage |
| 39 | µ-15..µ-18 | P2 | C·G | Reverts récurrents, sessions handoff non-monotones | git log | Validation par production |
| 40 | MC-9 / MC-10 | P3 | E | Modèle BGE-Large hardcodé ; co-existence v2 + legacy embedder | embedding_contract.rs:3 | Lock-in lent ; surface bugs |

---

## Lecture commerciale agrégée

### Trois trous structurels qui plafonnent le revenu

1. **Le différentiateur n'est pas livré côté contrat LLM.** La fusion hybride FTS+vector+graph (MIL-AXO-017 slice 4) existe en PG, performe (p95 25ms), mais aucun tool MCP ne l'expose. Le pitch commercial repose sur un tool fantôme.

2. **Les mutateurs SOLL canoniques (`soll_manager.create`, `soll_apply_plan`) ont des bugs critiques `current` en production.** Toute démo enterprise à charge réelle plante. Tag `commercial-value` confirmé par les LLM-loggers eux-mêmes en backlog.

3. **L'onboarding LLM coûte plus cher que la valeur livrée dans la première session.** 10-35K tokens avant la première action utile, sur un produit qui se vend comme réducteur de coût LLM. Le cache 5-min est sabordé par la procédure GUI-PRO-028 qui mandate des écritures à chaque fin de session.

### Trois trous méta qui plafonnent la crédibilité

4. **Le produit ne pratique pas sa méthodologie sur son propre repo.** 90 % des nodes SOLL sans signature autonome, 23 working-notes accumulés malgré le mandat de pruning, doc d'onboarding stale 24h+. Un audit LLM-client de cohérence vs claims pénalise la doc.

5. **La surface MCP mute toutes les 48 heures.** 31 fix(mcp) en 30 jours, rename `cypher`→`sql` la veille de l'audit, tools fantômes annoncés. Aucun customer ne peut bâtir sur une surface qui change ce rapidement sans contrat de stabilité.

6. **Dépendance Nix/devenv + `sudo nix-daemon` automatique** exclut tout segment enterprise hors-Nix. Concurrents en image OCI standard ramassent le marché.

### Verdict mercenaire 1-phrase

Le moteur d'Axon est techniquement solide (pipeline v2 zéro panic, RRF SQL bench-validée, 941 nodes SOLL riches, `parameter_repair` état de l'art) ; **le packaging produit, le contrat MCP exposé, et l'auto-application de la méthodologie sont structurellement sous-livrés** au point que la proposition de valeur LLM-coding-exclusive reste indéfendable sur un test live avec un prospect technique.

---

## Annexes — Rapports agents source

| Agent | Angle | Chemin |
|---|---|---|
| A | Docs / SKILL.md / MEMORY / working-notes / charge tokens | `/tmp/audit_agent_A_docs.md` |
| B | Runtime / scripts / release / devenv-Nix / diagnostiqabilité | `/tmp/audit_agent_B_runtime.md` |
| C | Git history / SOLL backlog / probes résiduels / pivots | `/tmp/audit_agent_C_git.md` |
| D | MCP contract surface / SOLL volume & dette / PostgreSQL | `/tmp/audit_agent_D_mcp_soll.md` |
| E | Pipeline v2 / retrieval hybride / observabilité LLM-client | `/tmp/audit_agent_E_pipeline.md` |

**Snapshot pré-audit (état runtime au moment du probe) :** `status.trust_boundary=degraded`, `freshness=stale`, `runtime_mode=brain_only`, `Symbol=101 689`, `File=0` (canonical writer). `soll_validate`: 35 violations. `soll_verify_requirements`: done=242 / partial=80 / missing=15. `anomalies`: 0 cycles / 0 god objects / 8 heuristic intent gaps. Cet état est lui-même un signal — c'est l'état "normal" du repo de référence.
