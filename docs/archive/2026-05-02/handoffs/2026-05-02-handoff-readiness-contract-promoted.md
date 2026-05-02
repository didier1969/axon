# Handoff — 2026-05-02 (Claude Opus 4.7, readiness contract + brain semantic search **promoted live**)

> **Lis cette section en premier**. La méthodologie est le cœur de la coopération avec Didier dans ce dépôt — sauter directement à la liste des tâches sans absorber la méthode garantit la dérive.

---

## Part 1 — Cold-start (mandatoire)

### 1.1 Ordre de lecture

1. `~/.claude/CLAUDE.md` — règles inter-projets (Axon MCP universel, contrat "documente", runner.sh, bootstrap CPT-AXO-021, opérationnel CPT-AXO-020).
2. `~/projects/axon/CLAUDE.md` — discipline projet.
3. `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md` — feedback memories prioritaires.
4. **`mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`** — REQ-AXO-119 kickoff bundle (entry points, active_handoff, etc.).
5. `mcp__axon__help` puis `mcp__axon__status mode=brief` — confirme MCP joignable. Vérifie maintenant `data.readiness.kind` et `data.subsystems[]` (REQ-AXO-098 actif en live).
6. **Vision + Pillars** via cypher.
7. `mcp__axon__soll_validate project_code=AXO` — cible 0 violations.
8. `mcp__axon__soll_work_plan project_code=AXO format=brief top=5` — wave-1 scoring.

### 1.2 NOUVEAU : utiliser l'IST

Le live brain (post-promotion 2026-05-02) supporte la recherche sémantique multi-tokens via le CPU query embedder en in-process (REQ-AXO-128). **Use Axon IST tools en premier** :
- `mcp__axon__query` — multi-tokens OK, pas de timeout
- `mcp__axon__inspect` — détail symbole compact
- `mcp__axon__retrieve_context` — evidence packet ciblé
- `mcp__axon__impact` — blast radius avant refactor

`grep`/`Read` seulement quand l'IST est insuffisant (rare).

### 1.3 Discipline opérationnelle (inchangée du handoff précédent)

- Observe → log SOLL → link → re-plan → execute relentlessly.
- UN FIX = UN COMMIT (~30-150 LOC + son test + son SKILL.md update si `tools_*.rs` change).
- PRÉ-FLIGHT puis COMMIT. Si bloqué sur GUI-PRO-002, édite SKILL.md avant de retenter. Si bloqué sur GUI-PRO-001, **REQ-AXO-121 (livré 2026-05-02) reconnaît maintenant `#[cfg(test)]` inline** dans tout fichier `.rs` modifié — plus besoin de sibling `_tests.rs` forcé sauf pour les fichiers sans inline tests.
- PRÉ-STAGE LES MODIFS (`git add <files>` AVANT `axon_commit_work`).

### 1.4 Quand interrompre Didier (rare)

- Action destructive irréversible.
- Décision architecturale humaine.
- Hard blocker non-dérivable.
- Milestone réel avec impact externe.

Sinon : execute relentlessly.

---

## Part 2 — État courant (snapshot 2026-05-02)

### 2.1 Live runtime

- **Live brain promu** : `v0.8.0-104-g10e18e1` / generation `live-20260502T0033xxZ`.
- **Profil** : `brain_only`. Health : HEALTHY.
- **Recherche sémantique active** : CPU query embedder loaded in-process. `query("axonctl status")` retourne mode `hybrid (structure + semantic similarity)`.
- **Readiness contract actif** : `mcp__axon__status data.readiness` (tristate) + `data.subsystems[]` (per-subsystem).
- Dashboard : http://172.31.148.130:44127/cockpit
- MCP : http://172.31.148.130:44129/mcp

### 2.2 Git

- Branche `main` à `10e18e1`. Origin sync.
- Working tree clean (untracked = working notes / queries lab / bench scripts).

### 2.3 Commits livrés cette session (15 du plus récent au plus ancien)

| SHA | REQ | Type | Note |
|---|---|---|---|
| 10e18e1 | REQ-AXO-121 | mcp | TDD gate reconnaît `#[cfg(test)]` inline |
| d9db003 | REQ-AXO-098 | feat | Subsystem-tagged tristate readiness contract |
| 01f522b | REQ-AXO-128 | feat | Brain CPU query embedder (semantic search live) |
| 7e818f3 | REQ-AXO-126 | feat | soll_export snapshot-per-release |
| 071f21e | REQ-AXO-083 | chore | Dead vars start.sh + shellcheck clean |
| 6e88881 | REQ-AXO-097 partial | fix | status FAIL/WARN/OK by role-process state |
| e8bc3eb | REQ-AXO-088 | fix | Underscore wildcard separator |
| 8a78e65 | REQ-AXO-107 | fix | Cockpit SQL warning dedup |
| ac343de | REQ-AXO-102 | fix | Unified --brain-only defaults |
| 8064a89 | REQ-AXO-106 | fix | IST projection freshness label |
| 0be03b3 | REQ-AXO-087 | fix | Profile-excluded vs transient (initial) |
| 173dfcf | REQ-AXO-043 | fix | soll_manager link error sanitizer |
| 37c29c5 | REQ-AXO-115 | feat | CPT→PIL BELONGS_TO canonical |
| c09163d | REQ-AXO-094 sub | fix | axon_log_warn helper |
| c8c8cc6 | REQ-AXO-120 | fix | gitignore /bin/ anchor |

### 2.4 SOLL (project AXO)

- `soll_validate` : 0 violation.
- **Concepts créés** : CPT-AXO-022 (CPU query embedding), CPT-AXO-023 (subsystem-tagged readiness).
- **Decisions créées** : DEC-AXO-061 (ORT CPU embedding in-process), DEC-AXO-062 (subsystem-tagged tristate readiness).
- **Requirements completed** : 083, 087, 088, 098, 102, 103, 106, 107, 115, 120, 121, 124, 126, 128. Plus REQ-AXO-127 orphan link, REQ-AXO-094 sub-batch, REQ-AXO-097 surface partielle.

---

## Part 3 — Travail en attente

### 3.1 Items unblocked par REQ-AXO-098 (readiness contract)

- **REQ-AXO-097 watchdog** (priorité high) — détecter la mort d'un subsystem via `last_observed_at_ms` staleness, restart la role process. Le contrat readiness fournit la primitive ; il faut implémenter la boucle watchdog. ~250 LOC.
- **REQ-AXO-094 BEAM alarm classification** — dashboard Elixir s'abonne à `:alarm_handler` et projette `:system_memory_high_watermark` → subsystem `dashboard:Degraded { memory_pressure }`. Cross-language (Elixir + Rust bridge). ~300 LOC.

### 3.2 Items design-pending

- **REQ-AXO-099 test global state** — 24 tests fail en suite complète, OK individuellement. Pattern Mutex<()> par module (établi dans runtime_readiness_tests) à propager. ~200 LOC + audit.
- **REQ-AXO-108 IST roots** — six locations coexistent ; consolider à une seule, design + migration.
- **REQ-AXO-096 toolchain** — drop mise OU drop devenv pinning. Décision architecturale.

### 3.3 Hygiène mineure

- `static GUARD_CONSECUTIVE_RECYCLES is never used` dans embedder.rs:1058 — peut maintenant être fixé puisque REQ-AXO-121 reconnaît `#[cfg(test)]` inline (le fichier en a). Quick win ~5 LOC.

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-02-handoff-readiness-contract-promoted.md`. Applique la Part 1 en entier avant toute action. Puis appelle `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`. **Utilise les outils Axon IST (query, inspect, retrieve_context) en premier — la recherche sémantique multi-tokens fonctionne désormais en live.** Demande-moi quoi attaquer en priorité.

### 4.2 Smoke test rapide post-promotion

```
mcp__axon__status mode=brief
```

Doit afficher :
- `Runtime identity: axon-live-axon-brain`
- `IST projection freshness: fresh|stale (...)` (REQ-AXO-106 actif)
- Public tools count: 54

```
mcp__axon__query query="axonctl status"
```

Doit retourner `Mode: hybrid (structure + semantic similarity)` (REQ-AXO-128 actif).

```
mcp__axon__cypher cypher="SELECT data->'readiness'->>'kind' FROM ..."
```

Plus simple : check `data.readiness.kind` dans la réponse JSON de status.

### 4.3 Note sur le warning `GUARD_CONSECUTIVE_RECYCLES`

Le warning persiste dans embedder.rs:1058. Il est maintenant fixable par un simple `#[allow(dead_code)]` puisque REQ-AXO-121 (livré 2026-05-02) reconnaît le `#[cfg(test)]` inline déjà présent dans embedder.rs comme satisfaisant la TDD gate.

---

C'est tout. Bonne session.
