# Handoff — 2026-05-01 (Claude Opus 4.7, contract-polish wave **promoted live**)

> **Lis cette section en premier** même si tu crois savoir ce qui se passe. La méthodologie ci-dessous est le cœur de la coopération avec Didier dans ce dépôt — sauter directement à la liste des tâches sans absorber la méthode, c'est garantir la dérive. Les LLM précédents s'y sont fait reprendre dans les premières dizaines de tours. Ne le sois pas.

---

## Part 1 — Cold-start (mandatoire)

### 1.1 Ordre de lecture

Skipping est la première cause de dérive.

1. `~/.claude/CLAUDE.md` — règles inter-projets (Axon MCP universel, contrat "documente", runner.sh, bootstrap CPT-AXO-021, opérationnel CPT-AXO-020).
2. `~/projects/axon/CLAUDE.md` — discipline projet (build/test, sub-agent policy, data policy, deployment pipeline).
3. `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md` — mémoire persistante. Lis chaque feedback memory pointée. Les cinq les plus utiles aujourd'hui :
   - `feedback_no_premature_stop_with_open_unblockers.md` — pas de closing summary / `/schedule` / "tu veux continuer ?" tant qu'un wave-1 unblocker pré-autorisé a des descendants ouverts.
   - `feedback_axon_commit_work_only_stages_deletions.md` — `git rm` est auto-stagé, Edit/Write ne l'est pas. `git add` les modifs avant commit, `git status --short` après.
   - `feedback_documente_means_soll.md` — doc → SOLL via `soll_manager`, pas Markdown.
   - `feedback_axon_mcp_universal_protocol.md` — observe → log SOLL → link → re-plan → execute relentlessly.
   - `feedback_always_use_promotion_pipeline.md` — jamais `cargo build --release` + copie manuelle ; toujours `promote_live_safe.sh`.
4. **`mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`** — depuis 2026-05-01 (REQ-AXO-119), cet appel renvoie un `data.kickoff_bundle` complet : `kickoff_prompt` (DEC-PRO-001 verbatim), `methodology_summary` (CPT-AXO-019 verbatim), `entry_points` (10 étapes machine-actionable), `active_handoff` (chemin vers le handoff actif). C'est l'entry point unique. Si tu n'as que MCP, cet appel suffit à t'orienter.
5. `mcp__axon__help` puis `mcp__axon__status mode=brief` — confirme MCP joignable et capture l'instance / profil / freshness / vector backlog.
6. **Vision projet** :
   ```
   mcp__axon__cypher SELECT id, title, description FROM soll.main.Node WHERE project_code='AXO' AND type='Vision'
   ```
7. **Pillars projet** (lis chaque description en entier) :
   ```
   mcp__axon__cypher SELECT id, title, description FROM soll.main.Node WHERE project_code='AXO' AND type='Pillar' ORDER BY id
   ```
8. **Travail récent** :
   ```
   git log --oneline | head -20
   mcp__axon__cypher SELECT id, title, status FROM soll.main.Node WHERE project_code='AXO' AND type IN ('Decision','Milestone') AND status IN ('accepted','delivered','completed') ORDER BY id DESC LIMIT 30
   ```
9. **`mcp__axon__soll_validate project_code=AXO`** — cible 0 violations.
10. **`mcp__axon__soll_work_plan project_code=AXO format=brief top=5 limit=15`** — score wave-1 (210 vs 120 vs 80) **autoritaire** sur ton jugement.

### 1.2 Discipline opérationnelle

- **OBSERVE** activement (friction, bugs, simplifications, obsolète, violations contrat LLM). N'attends pas que Didier le dise.
- **LOG SOLL** via `soll_manager(action=create)` — `requirement` pour les actionables, `decision` pour les choix, `concept` pour les modèles partagés. Linke immédiatement à un Pillar via `soll_manager(action=link, type=BELONGS_TO)` ou `soll_validate` flagge l'orphan.
- **EXÉCUTE le top du work plan**. Si tu pivotes, dis-le explicitement avant.
- **UN FIX = UN COMMIT** (~30-150 LOC + son test + son SKILL.md update si `tools_*.rs` change). Bisect-friendly, review-friendly.
- **PRÉ-FLIGHT, PUIS COMMIT**. `axon_pre_flight_check` AVANT `axon_commit_work`. Si pre-flight bloque sur GUI-PRO-002, édite `docs/skills/axon-engineering-protocol/SKILL.md` AVANT de retenter. Si bloqué sur GUI-PRO-001, ajoute un test (et si c'est un binaire, mets-le dans un sibling `_tests.rs` via `#[path]` — voir REQ-AXO-121).
- **PRÉ-STAGE LES MODIFS**. `git add <files>` avant `axon_commit_work`. `git status --short` après.

### 1.3 Quand interrompre Didier (rare)

Uniquement pour :
- Action destructive irréversible (force-push, drop table, rm -rf hors scope).
- Décision architecturale qui nécessite l'autorité humaine.
- Hard blocker qui réclame une info non-dérivable.
- Milestone réel **avec impact externe** (deploy, release, fix qui débloque un humain). PAS la vélocité interne ("j'ai fait N commits").

Sinon : **execute relentlessly**. Petits status pings entre phases, pas de question "tu veux que je continue ?".

### 1.4 Patterns qui ont marché en 2026-05-01

- **Probe → confirm → fix → test → commit → SOLL update** : chaque fix de cette journée a suivi cette boucle.
- **Quand 4+ sites dupliquent, extrais un helper** (cf. `wrong_project_scope_response`, `normalized_soll_writer_error`, `auto_resolve_project_code_str`, `axon_init_project_bundle`, `prune_old_soll_exports`).
- **Tests Rust de binaires** : Rust `#[cfg(test)] mod tests {}` inline n'est PAS reconnu par GUI-PRO-001 (REQ-AXO-121). Mets le mod dans un fichier sibling `<binname>_tests.rs` référencé via `#[cfg(test)] #[path = "..."] mod xxx;`. Vu pour `axonctl_tests.rs` et `graph_query_tests.rs`.
- **Tests qui touchent env vars** : `let _guard = env_lock();` au début, `unsafe { std::env::set_var(...) }`, `unsafe { std::env::remove_var(...) }` à la fin. **Attention** : `env_lock` ne sérialise QUE les tests qui le prennent. Ne pose PAS d'env var qu'un test parallèle pourrait lire (REQ-AXO-126's failed disabled-branch test l'a illustré).
- **Liens SOLL après création.** Toute nouvelle requirement orpheline → warn de `soll_validate`. Linke à `PIL-AXO-XXX` immédiatement.
- **Si tu vois `Insert error: INSERT INTO ... ?`**, c'est REQ-AXO-091 qui mordait avant aujourd'hui. **Désormais corrigé en live**. Si ça remord, c'est une régression — re-ouvrir REQ-AXO-091 et re-tester.

### 1.5 Failure modes à NE PAS répéter

- **Stop-bias** : produire un closing summary "milestone atteint" alors que la wave-1 a encore des descendants ouverts. Didier l'a explicitement reproché ("Qui t'a demandé de t'arrêter ?"). Le déclencheur "milestone reached" du protocole = impact externe (deploy, release), PAS travail interne en cours.
- **Sub-agents pour exploration code** : NE JAMAIS spawn de sub-agent pour query/inspect/lookup symboles. Les sub-agents n'ont pas accès MCP, retombent en raw file reads, brûlent 100-200K tokens. Toujours utiliser Axon MCP (`query`, `inspect`, `retrieve_context`, `impact`, `anomalies`) directement.
- **`mcp__axon__query` tombe en timeout sous brain_only ?** Erreur fréquente début 2026-05-01 : interpréter `Advanced indexed surfaces visible: no` comme exclusion design. Didier a corrigé : brain_only doit donner accès complet à SOLL **et** IST ; seule l'indexation est exclue. Le timeout vient du chemin embedding-fallback sous charge (REQ-AXO-124). Les requêtes single-token sur structural marchent. Multi-token sémantique peut timeout — re-essaye en single-token ou fallback `cypher`.
- **Cypher INSERT/UPDATE pour bypass un missing API** : interdit. Cypher est read-only par spec. Ouvre la requirement pour ajouter l'API au lieu de tricher.
- **Markdown fallback alors que MCP est joignable** : restart MCP au lieu de fallback. `bash /home/dstadel/projects/axon/scripts/lib/start-brain.sh AXON_INSTANCE_KIND=live`.
- **Re-fermer une observation re-vérifiée comme "ma mauvaise interprétation"** : la mauvaise interprétation EST le bug. Le contrat a échoué. Re-frame en LLM-contract violation, garde l'entry ouverte.
- **`git status` après commit** : commit b009c49 du 2026-05-01 pré-session ne contenait QUE les `git rm`, pas les Edit. Toujours vérifier.

### 1.6 Aides pré-autorisées

- `/home/dstadel/.claude/runner.sh` — exécutable pré-autorisé. Édite son corps puis `bash /home/dstadel/.claude/runner.sh`. PAS pour bypass action destructive.
- Bootstrap canonique CPT-AXO-021 dans SOLL : `mcp__axon__cypher SELECT description FROM soll.main.Node WHERE id='CPT-AXO-021'`.

---

## Part 2 — État courant (snapshot 2026-05-01 22:00 UTC)

### 2.1 Live runtime

- **Live brain promu** : `v0.8.0-89-g6ff7e39` / generation `live-20260501T200437Z`.
- **Profil** : `brain_only` (pas d'indexer live, dashboard alive).
- **Health** : `HEALTHY` (qualify-mcp verdict ok pour quality + latency).
- **`Advanced indexed surfaces visible: yes`** — IST reads OK depuis le brain.
- Dashboard : http://172.31.148.130:44127/cockpit
- MCP : http://172.31.148.130:44129/mcp

### 2.2 Dev runtime

- Dev indexer pid 27243 toujours alive depuis avant la session, état partiel (MCP non bound). Pas dérangé par la promotion. Si tu veux faire du dev, `bash scripts/axon-dev stop` puis restart proprement.

### 2.3 Git

- Branche `main` à `6ff7e39` (15 commits cette session, dont les 14 du contract polish + smoke test REQ-AXO-127).
- **`origin/main` à jour** : `git push origin main` du 2026-05-01 a livré `992f7a2..6ff7e39` (42 commits, dont 27 anciens unpushed + 15 de cette session).
- Working tree clean (seuls les working notes / queries lab / scripts de bench restent untracked, gitignore-friendly).

### 2.4 Commits livrés cette session (du plus récent au plus ancien)

| SHA | REQ | Type | Note |
|---|---|---|---|
| 6ff7e39 | REQ-AXO-125 | mcp | Normalisation des erreurs writer (no raw SQL leak) |
| 742e6f7 | REQ-AXO-126 | mcp | soll_export disabled by default + 562 fichiers purgés |
| 0f889c4 | REQ-AXO-104 | mcp | Status brief omits public_tools list |
| cdca947 | REQ-AXO-100 | bash | --full alias for --indexer-full |
| f812b4a | REQ-AXO-089 ext | mcp | query+inspect cwd auto-resolve |
| 2b5c8b1 | REQ-AXO-119 | mcp | axon_init_project kickoff bundle |
| 78066b0 | REQ-AXO-095 | bash | MCP verification fall-through to HTTP |
| af1e4ab | REQ-AXO-089 | mcp | retrieve_context cwd auto-resolve |
| 8a71234 | REQ-AXO-091 | rust | **Placeholder bug** (`?` no longer eaten) |
| f88f1fb | REQ-AXO-043 | mcp | conception_view + change_safety wrong_project_scope |
| 380be19 | REQ-AXO-043 | mcp | axon_apply_guidelines empty/unknown contract |
| c6a1383 | REQ-AXO-116 | rust | axonctl Rust-side socket cleanup tests |
| eea76c6 | REQ-AXO-109 | bash | Cross-instance env contamination guard |
| 5459821 | REQ-AXO-117 | bash | Socket lifecycle test + extracted helpers |

### 2.5 SOLL (project AXO)

- `soll_validate` : 0 violation.
- Nouvelles observations loggées cette session : REQ-AXO-120 (gitignore unanchored), REQ-AXO-121 (TDD checker friction), REQ-AXO-124 (brain_only label), REQ-AXO-125 (writer error leak — fixed), REQ-AXO-126 (soll_export retention design — pending), REQ-AXO-127 (smoke test — completed).
- REQ closes : 089, 091, 095, 100, 104, 109, 116, 117, 119, 125, 126, 043 (5 sub-fixes).

### 2.6 Tests

- Lib tests : 47 runtime_surface, 5 graph_query, 13 axonctl, 20 soll_manager, 4 axon_init_project — tous verts cette session sur les chemins touchés.
- REQ-AXO-099 (24+ failures en suite complète) reste open. PAS prioritaire.

---

## Part 3 — Travail en attente

### 3.1 Décisions design pending (Didier)

Aucune urgence ; toutes sont des items high-priority qui demandent une orientation avant d'engager du code.

- **REQ-AXO-094** (boot output hygiene) — UX, ~150 LOC. Acceptance criterion explicite : couches "status d'abord puis warnings", CI zero compile warnings, BEAM alarms escalent en degraded-readiness. Spans bash + Rust + Elixir → trop pour un commit, à splitter.
- **REQ-AXO-097** (status returns HEALTHY when role processes dead) — robustness, ~200-400 LOC. Touche `axonctl status` (bon déjà), `axon_status_status_impl` (à durcir), et un watchdog. PIL-AXO-001 enjeu.
- **REQ-AXO-098** (degraded-readiness contract) — overlap avec 097 ; tristate ready/degraded(reason)/failed(reason).
- **REQ-AXO-099** (test suite global state) — multi-session. 24+ failures en suite complète mais individuels OK. Nécessite `serial_test` ou refactor lourd.
- **REQ-AXO-124** (brain_only label clearer + skip embedding fallback fast) — design + Rust. Le label est passé "no" → "yes" post-promotion (incidemment fixé) mais la séparation IST-availability vs embedding-availability reste à formaliser.
- **REQ-AXO-126** (soll_export retention final policy) — pending. Quatre options : on-demand only, auto-rotation N, snapshot par release, suppression complète au profit de soll_query_context. Didier décide.

### 3.2 Petits items tractables (pour démarrer la prochaine session sans design)

- **REQ-AXO-115** (low) — SOLL relation schema PIL-CPT / CPT-PIL forbid. Soit étendre le canonical relation schema, soit documenter le workaround dans `soll_relation_schema` help. Effort petit.
- **REQ-AXO-103** (low) — SOLL_EXPORT retention — mostly couvert par REQ-AXO-126's disable, mais le concept de rotation reste si on ré-active.
- **REQ-AXO-094 sub-batch** — start.sh seul (sans BEAM ni Rust) : normaliser les `⚠️` warnings via une fonction `axon_log_warn` qui ajoute un prefix machine-readable et émet via stderr. Petit, scope contenu.
- **REQ-AXO-120** (gitignore unanchored) — fix : `bin/` → `/bin/` à la racine. Ouvre src/axon-core/src/bin/ aux nouveaux fichiers sans `-f`.
- **REQ-AXO-121** (TDD checker doesn't recognize inline Rust binary tests) — soit étendre `path_satisfies_required_path` pour reconnaître `#[cfg(test)] mod` inline dans `src/*/bin/*.rs`, soit documenter explicitement la convention sibling `_tests.rs`.

### 3.3 Surface REQ-AXO-043 — encore à auditer

J'ai fait 14 tools + 2 helpers partagés (`wrong_project_scope_response`, `normalized_soll_writer_error`). Restent :
- `axon_apply_guidelines` ✅ done (commit 380be19)
- `change_safety` ✅ done (commit f88f1fb)
- `conception_view` ✅ done (commit f88f1fb)
- `debug` — déjà bon, skip
- `diagnose_indexing` — déjà bon (rapport "scope_mismatch_or_wrong_project_code" est actionable), skip
- `audit` — handoff précédent disait "déjà bien structuré, peut-être skip"
- `soll_attach_evidence` — déjà adopté précédemment ; vérifier que c'est aligné avec le helper `normalized_soll_writer_error`
- `soll_manager link/attach` paths — encore raw error strings (lignes ~485 manager.rs). Les passer au helper `normalized_soll_writer_error("link", e)` / `("attach", e)`. Petit commit suite à REQ-AXO-125.

### 3.4 Hygiène bonus

- Warning Rust `static GUARD_CONSECUTIVE_RECYCLES is never used` dans `embedder.rs:1055`. Soit `#[allow(dead_code)]` (matching les constantes voisines), soit suppression complète si la feature consecutive-recycles n'a pas d'avenir. Petit commit.

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot pour Didier

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-01-handoff-contract-polish-promoted.md`. Applique la Part 1 en entier avant toute action. Puis appelle `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon` pour récupérer le kickoff bundle. Puis demande-moi quoi attaquer en priorité.

### 4.2 Le plus court chemin si tu n'as que MCP

Depuis 2026-05-01, REQ-AXO-119 a fait de `axon_init_project` l'entry point unique. Si la nouvelle session n'a accès qu'à MCP (pas de shell, pas de file reads), un seul appel suffit pour récupérer :
- `data.kickoff_bundle.kickoff_prompt` — DEC-PRO-001 verbatim, le bootstrap protocol au complet
- `data.kickoff_bundle.methodology_summary` — CPT-AXO-019 verbatim, l'observe-log-link-replan-execute loop
- `data.kickoff_bundle.entry_points` — 10 étapes machine-actionable (file/mcp/cypher), incluant le pointer vers ce handoff via `active_handoff`
- `data.kickoff_bundle.active_handoff` — chemin vers le handoff actif (ce fichier après le commit du pointeur dans MEMORY.md)

L'appel à `axon_init_project` est donc le **deuxième** appel à faire après `help()` / `status()`, avant tout autre travail.

### 4.3 Smoke test rapide post-promotion (optionnel)

Si tu doutes que la promotion soit effective, vérifie en une commande :

```
mcp__axon__status mode=brief
```

Tu dois voir :
- `Runtime identity: axon-live-axon-brain`
- `Advanced indexed surfaces visible: yes`
- `Public tools count: 54 (full list available via status mode=verbose...)` — preuve que REQ-AXO-104 est actif
- Pas de "Public tools: help, refine_lattice, fs_read, ..." inline — le contract polish est en place.

Si tu vois `Public tools: help, refine_lattice, fs_read,` (liste inline), le live n'a pas la promo et il faut investiguer (regarder `runtime_version` dans status verbose).

### 4.4 Note importante sur `?` dans les payloads SOLL

REQ-AXO-091 est corrigé en live (commit 8a71234, promu dans `v0.8.0-89-g6ff7e39`). Tu peux désormais inclure des `?` littéraux dans les titres et descriptions SOLL. Si tu vois jamais un `Writer Error: INSERT INTO ... ?, ?, ?` malformé revenir, c'est une régression — re-ouvre REQ-AXO-091 et re-teste.

---

C'est tout. Le bundle de Part 1 et REQ-AXO-119 font le reste. Bonne session.
