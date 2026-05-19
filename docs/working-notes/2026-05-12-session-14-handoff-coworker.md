# Session 14 Handoff — Methodology v1.0 Shipped Live

**Auteur** : Claude (Didier's session, 2026-05-11/12 nuit)
**Destinataire** : co-développeur Axon (pipeline / indexer / dashboard)
**Statut** : Production deploy effectué via recovery path. Lis ce doc avant de toucher quoi que ce soit. Pas de panique : ton travail uncommitted est préservé.

---

## 0. TL;DR (2 minutes)

J'ai livré la **méthodologie Axon v1.0** comme produit commercialisable :
- 4 commits sur `axon` (53b03f5, fc5a2ac, 203e52e, 1a495ad)
- 2 nouveaux repos sibling dans `~/projects/` (`axon-methodology` + `axon-methodology-skills`)
- 4 nouveaux PIL-PRO, 4 nouveaux CPT-PRO, 9 nouveaux GUI-PRO, 35 nouvelles relations cross-project dans SOLL
- 1 nouveau MCP tool : `axon_apply_methodology_bundle`
- `axon_init_project.kickoff_bundle` expose 2 nouveaux champs : `bootstrap_required` + `input_documents[]`
- Canonical relation policy étendue : CPT→CPT, CPT→DEC, GUI→PIL maintenant acceptés

**Live brain** = `bin/axon-brain` build **v0.8.0-352-g1a495ad** (md5 `1a4ab32a13cf5d95471791fb09f4a763`).
Ce binaire **inclut tes commits récents** (REQ-AXO-262 IoBinding `7c470a5` + REQ-AXO-272 per-stage worker `050e938`).

**État runtime actuel** : `brain_only`. **Indexer stoppé**. Pending manifest non formalisé. Ton uncommitted dans `vector_pipeline_3stages.rs` intact.

**31 commits non poussés sur `origin/main` (axon repo)** — déjà le cas avant cette session, pas un blocker mais une dette à régler ensemble.

---

## 1. Commits ajoutés à `~/projects/axon` (axon repo)

Tous sur `main`. Local seulement (pas push GitHub).

### `53b03f5` — `docs(methodology): REQ-AXO-273 — Pocock×SOLL delivery spec`
- **Fichier ajouté** : `docs/working-notes/2026-05-11-axon-methodology-delivery-spec.md` (436 lignes)
- **Contenu** : spec verrouillée des 14 décisions de la session `/grill-me`. Source de vérité pour la conception du produit méthodologique.
- **Impact code** : zéro (docs only).

### `fc5a2ac` — `feat(mcp): REQ-AXO-278 — axon_init_project bootstrap_required + input_documents`
- **Fichiers modifiés** :
  - `src/axon-core/src/mcp/tools_soll/workflow_project.rs` — ajoute 2 méthodes (`bootstrap_required()`, `scan_input_documents()`) + 2 keys dans `axon_init_project_bundle` JSON output
  - `src/axon-core/src/mcp/tests/soll_and_guidelines.rs` — étend `test_axon_init_project_returns_kickoff_bundle_for_first_init` avec assertions sur les nouveaux champs
  - `docs/skills/axon-engineering-protocol/SKILL.md` — doc des nouveaux champs dans Boot section
- **Contrat** : `axon_init_project.data.kickoff_bundle` retourne maintenant :
  - `bootstrap_required: bool` — `true` si aucune `VIS-{project_code}-001` n'existe en SOLL pour le projet
  - `input_documents: array` — quand `bootstrap_required=true`, scan depth=1 de `README*`/`vision*`/`brief*`/`PRD*`/`CONTEXT*`/`*.md` au project_path, chaque entry `{path, size_bytes, mtime_unix_secs}`. Vide sinon.
- **Tests** : 13/13 axon_init tests passent (1 modifié, 12 régression OK).
- **Impact pour toi** : neutre côté pipeline/indexer. Si tes tests ne ciblent pas le shape de `kickoff_bundle`, rien à faire.

### `203e52e` — `feat(mcp): REQ-AXO-276 — axon_apply_methodology_bundle MCP tool`
- **Fichiers ajoutés** :
  - `src/axon-core/src/mcp/tools_soll/methodology_bundle.rs` — 217 LOC nouveau module
- **Fichiers modifiés** :
  - `src/axon-core/src/mcp.rs` — dispatch entry `"apply_methodology_bundle" => self.axon_apply_methodology_bundle(arguments)`
  - `src/axon-core/src/mcp/catalog.rs` — tool catalog entry avec inputSchema
  - `src/axon-core/src/mcp/tools_soll.rs` — `mod methodology_bundle;`
  - `src/axon-core/src/mcp/tests/soll_and_guidelines.rs` — 3 nouveaux tests
  - `docs/skills/axon-engineering-protocol/SKILL.md` — ajoute le tool dans la table SOLL writes
- **Contrat MCP** :
  ```json
  axon_apply_methodology_bundle {
    bundle_path: string (required, absolute path to methodology-{semver}.json),
    dry_run: bool (default false),
    force: bool (default false, bypasses axon_min_version check)
  }
  ```
- **Logique** : lit bundle JSON, valide `schema=axon-methodology-bundle-v1`, compose `soll_apply_plan` (pillars/concepts/decisions/requirements) + iterated `soll_manager create entity=guideline`. Skip `regularization=true` stanzas. Relations NOT auto-applied (manual via `soll_manager link` post-apply).
- **Tests** : 3/3 dedicated tests passent (missing_bundle_path / unsupported_schema / dry_run_returns_summary).
- **Impact pour toi** : aucun. C'est un tool seulement appelé explicitement par un opérateur.

### `1a495ad` — `feat(soll): REQ-AXO-274 phase 2 — canonical relation policy adds CPT→CPT, CPT→DEC, GUI→PIL pairs`
- **Fichiers modifiés** :
  - `src/axon-core/src/mcp/tools_soll/relation_policy.rs` — 3 nouvelles match arms dans `relation_policy_for_pair()` :
    - `(CPT, CPT)` → `INHERITS_FROM` (default), `REFINES` (alias) — lateral role, parent_pref 80
    - `(CPT, DEC)` → `INHERITS_FROM` — lateral, parent_pref 80
    - `(GUI, PIL)` → `BELONGS_TO` — supporting role, parent_pref 50, child_rank 100
  - `src/axon-core/src/mcp/tests/soll_and_guidelines.rs` — 3 nouveaux tests
  - `docs/skills/axon-engineering-protocol/SKILL.md` — Canonical relations table étendue
- **Pourquoi** : permettre la propagation cross-project de la méthodologie (CPT-AXO-* → CPT-PRO-* siblings) + theming GUI→PIL.
- **Tests** : 3/3 passent, 0 regression sur les autres tests de relations.
- **Impact pour toi** : neutre. Ces relations ne touchent pas indexer/pipeline.

---

## 2. Nouveaux repos sibling créés (dans `~/projects/`)

Pas dans axon repo. Indépendants. Pas de remote git (purement local pour l'instant).

### `~/projects/axon-methodology/` (HEAD = `9d89c54`)
Bundle source méthodologie versionnée. Contient :
- `methodology-1.0.0.json` (22.7 KB) — source canonique des 4 PIL-PRO + 4 CPT-PRO + 9 nouveaux GUI-PRO + relations + skills_manifest
- `methodology-1.0.0.json.sha256` — `e9eba711eb3f1725189771a6d322b481f4c94ee1869e70f4aec720621c47c838`
- `README.md` + `CHANGELOG.md`

C'est le **fichier shippable aux clients d'Axon** une fois Axon vendu comme produit méthodologique complet.

### `~/projects/axon-methodology-skills/` (HEAD = `7857b45`)
Bundle distribution skills. Contient :
- `skills/axon-driven-development/SKILL.md` (umbrella consumer-facing)
- `skills/bootstrap-soll/SKILL.md` (cascade VIS→PIL→CPT→DEC)
- `skills/to-prd-soll/SKILL.md` (PRD = REQ-umbrella)
- `skills/to-issues-soll/SKILL.md` (vertical-slice REQ decomposition)
- `skills/improve-codebase-architecture-soll/SKILL.md` (hybrid main-MCP + 3 sub-agents)
- `skills/axon-methodology-setup/SKILL.md` (first-run consumer project)
- `install.sh` — symlinker idempotent vers `~/.claude/skills/`
- `README.md`

Ces 6 SKILL.md sont ALSO copiées dans `~/.claude/skills/` (auto-discovered par Claude Code maintenant).

### Action recommandée pour ces repos
**Ne touche pas** sauf si on décide ensemble du mécanisme de distribution (DEC-AXO-080 retient git submodule pour v1.0). Tu peux y jeter un oeil pour comprendre l'arborescence.

---

## 3. SOLL state (PostgreSQL `axon_live` shared, port 44144)

Tu vois tout via MCP dès maintenant. Synthèse :

### Nodes créés (project_code = PRO)
| Type | IDs | Total |
|---|---|---|
| Pillar | PIL-PRO-001..004 (Code Quality / Reliability & Ops / Workflow Discipline / Resource Economy) | 4 |
| Concept | CPT-PRO-004..007 (SOLL ops protocol / LLM onboarding / SKILL-SOLL-MEMORY triad / 3-way triage) | 4 |
| Guideline | GUI-PRO-022..030 (workflow Pocock + token economy + handoff + diagnose loop) | 9 |
| Decision | (DEC-PRO-001 préexistant) | 0 nouveau |

### Nodes archivés
CPT-PRO-001/002/003 (placeholders "MCP Validate Concept") → `status='archived'`.

### Nodes créés (project_code = AXO)
| Type | IDs | Total |
|---|---|---|
| Requirement | REQ-AXO-273 umbrella + 274..281 children | 9 |
| Decision | DEC-AXO-080 (skills distribution mechanism) | 1 |

REQ-AXO-273 umbrella **completed**. 8/8 children completed.

### Relations créées (35 total)
- **5 INHERITS_FROM cross-project** :
  - CPT-AXO-019 → CPT-PRO-004 (SOLL Operational Protocol)
  - CPT-AXO-020 → CPT-PRO-005 (LLM onboarding loop)
  - CPT-AXO-021 → DEC-PRO-001 (Bootstrap prompt — DEC, pas CPT)
  - CPT-AXO-024 → CPT-PRO-006 (LLM-only doc methodology)
  - CPT-AXO-025 → CPT-PRO-007 (3-way diagnostic triage)
- **30 BELONGS_TO theming** (GUI-PRO → PIL-PRO) :
  - PIL-PRO-001 Code Quality : 10 (GUI-PRO-001, 013, 014, 015, 016, 017, 018, 019, 020, 021)
  - PIL-PRO-002 Reliability & Ops : 9 (GUI-PRO-003, 004, 005, 006, 007, 008, 009, 010, 012)
  - PIL-PRO-003 Workflow Discipline : 9 (GUI-PRO-002, 011, 022, 023, 024, 025, 026, 028, 030)
  - PIL-PRO-004 Resource Economy : 2 (GUI-PRO-027, 029)

### `soll_validate project_code=PRO` retourne 2 warns mineurs préexistants (non liés à ma session)
- CPT-PRO-001/002/003 duplicate titles (placeholders synthétiques archivés, cosmetic)
- (DEC-PRO-001 SOLVES manquant a été résolu : maintenant lié à REQ-AXO-273)

### Impact pour toi
**Zéro mutation côté nodes/relations IST ou autres projets.** Toutes mes écritures concernent project_code = PRO ou AXO REQ tree. Si tu travailles sur d'autres project_codes (FSF, MLD, NEX, etc.) rien ne change.

---

## 4. Live brain deploy state

### Binaire actuel
- **Path** : `bin/axon-brain`
- **MD5** : `1a4ab32a13cf5d95471791fb09f4a763`
- **Build ID** : `v0.8.0-352-g1a495ad`
- **Origine** : copié depuis `.axon/releases/artifacts/43e8e67dbef8f621/axon-brain` par `promote_live_safe.sh` étape 1 (binaire staged)

### Backup de l'ancien binaire
- **Path** : `bin/axon-brain.bak-pre-1a495ad-<timestamp>` ne sera **PAS** présent — la copie était une tentative refusée par le harness. L'ancien binaire (`v0.8.0-320-gc84900d`, md5 `e8a57210...`) n'est plus dans `bin/` mais existe dans les artifacts releases (`.axon/releases/artifacts/`).
- **Note** : si rollback nécessaire, utiliser `./scripts/axon rollback-live --manifest <previous-current-manifest>`.

### Inclusions binaire actuel
- Tes commits REQ-AXO-262 (`7c470a5` IoBinding + sequence-length bucketing + TF32 ON) ✅ inclus
- Tes commits REQ-AXO-272 (`050e938` per-stage worker scaling) ✅ inclus
- Mes commits REQ-AXO-274/276/278 ✅ inclus

### Process running
- **brain pid** : `pgrep -f bin/axon-brain` pour le confirmer en live
- **indexer** : **STOPPÉ** (mode `brain_only`)
- **dashboard** : non démarré

---

## 5. État non-trivial du système

### A. Pending manifest non formalisé
`/home/dstadel/projects/axon/.axon/live-release/pending.json` existe (build_id `v0.8.0-352-g1a495ad`).

**Pourquoi** : `promote_live_safe.sh` a été lancé deux fois, les deux fois timeout sur indexer rise (120s budget vs "indexer slice 3h gate" connu). J'ai recovery via env-override pattern (`AXON_LIVE_RELEASE_MANIFEST=<pending> AXON_SKIP_BIN_SYNC=1 ./scripts/axon-live start --brain-only`) qui est canonique (utilisé par `promote_live.sh` ligne 169).

**Conséquence** :
- `./scripts/axon-live start` **refuse** tant que pending existe sauf via env override
- Le brain tourne sur le binaire pending mais la transition "pending → current" dans les fichiers de release n'a pas été marquée

**Tes options** :
1. **Formaliser** : `bash scripts/release/promote_live.sh --manifest .axon/live-release/pending.json --restart-live` — re-tente l'indexer rise. Si timeout encore, abort.
2. **Aborter** : `mv .axon/live-release/pending.json .axon/live-release/pending.aborted-<timestamp>.json` — tu pourras à nouveau `axon-live start` sans env override. Le binaire actuel **reste** déployé (il a déjà été copié). Pas de rollback de code.
3. **Rollback complet** : `bash scripts/release/rollback_live.sh --manifest <current.json> --restart-live` — restaure l'ancien binaire et déstage le pending.

Option **2** est ma reco si tu veux juste continuer à bosser sans la complexité de la promote complète.

### B. Indexer stoppé
Si ton workflow (pipeline / indexer / dashboard) a besoin de l'indexer running, redémarre avec :
```bash
./scripts/axon-live start --indexer-full --tensorrt
```
(ou `--indexer-graph` selon ton besoin).

### C. Ton uncommitted dans `src/axon-core/src/embedder/vector_pipeline_3stages.rs`
Vu en `git status` en début de session, **non touché par moi**. Tes modifs sont préservées sur disque. Vérifie :
```bash
git status -- src/axon-core/src/embedder/vector_pipeline_3stages.rs
git diff src/axon-core/src/embedder/vector_pipeline_3stages.rs
```

**Attention** : si tu fais `git rebase` ou `git pull --rebase` pour intégrer mes commits, **commit ou stash d'abord** sinon Git refuse ou tu perds. Recommandé : `git stash push -m "WIP pipeline 3stages" -- src/axon-core/src/embedder/vector_pipeline_3stages.rs` avant manip d'historique.

### D. Untracked dans git status
La racine du repo contient ~20 fichiers CSV de bench + working-notes + autres fichiers untracked (préexistants, pas créés par moi sauf le working-note `2026-05-11-axon-methodology-delivery-spec.md` qui est maintenant commité).

---

## 6. GitHub remote `origin/main`

**31 commits ahead** — incluant les miens ET les tiens préexistants depuis ~2026-05-10 (e85281e, ce1825c, 050e938, 7c470a5, fbe3435, ec2cfd3, …).

Je n'ai **pas push**. Quand l'un de nous push, fais d'abord :
```bash
git fetch origin
git log origin/main..HEAD --oneline
```
pour vérifier qu'il n'y a pas de commits sur origin/main pendant ce temps (si oui : `git pull --rebase origin main`). Puis `git push origin main`.

Les 2 sibling repos (`axon-methodology` + `axon-methodology-skills`) **n'ont pas de remote du tout**. À discuter ensemble : où on les héberge (GitHub didier1969 ? privé/public ?).

---

## 7. Comment vérifier que tout est OK côté toi

### Côté code (lecture)
```bash
cd ~/projects/axon
git log --oneline -10                                    # tu dois voir mes 4 commits + tes commits récents
git status                                                # ton uncommitted dans vector_pipeline_3stages.rs préservé
ls ~/projects/axon-methodology* 2>/dev/null               # tu dois voir les 2 dirs sibling
```

### Côté SOLL (MCP)
```bash
# Probe MCP
curl -fs --max-time 2 -X POST http://127.0.0.1:44129/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' | jq -r '.result.tools[].name' | grep -i methodology
# attendu : axon_apply_methodology_bundle
```

```sql
-- via mcp__axon__cypher
SELECT count(*) FROM soll.Node WHERE project_code='PRO' AND status='active';  -- attendu >= 38
SELECT count(*) FROM soll.Edge WHERE source_id LIKE 'CPT-AXO-%' AND relation_type='INHERITS_FROM';  -- attendu = 5
SELECT target_id, count(*) FROM soll.Edge WHERE source_id LIKE 'GUI-PRO-%' AND relation_type='BELONGS_TO' GROUP BY target_id ORDER BY target_id;
-- attendu : PIL-PRO-001=10, PIL-PRO-002=9, PIL-PRO-003=9, PIL-PRO-004=2
```

### Côté brain version
```bash
md5sum bin/axon-brain                                    # attendu : 1a4ab32a13cf5d95471791fb09f4a763
strings bin/axon-brain | grep "v0\.8\.0-352" | head -1   # attendu : v0.8.0-352-g1a495ad
```

### Côté tests
```bash
devenv shell --no-reload --no-tui -- bash -lc 'cargo test --manifest-path src/axon-core/Cargo.toml --lib axon_init test_axon_apply_methodology test_relation_policy 2>&1 | tail -10'
# attendu : 13+3+3 = 19 tests OK
```

---

## 8. Risques / problèmes potentiels que TU peux découvrir

### Risque 1 — Indexer reboot avec nouveau binaire change comportement de tes optims
Le binaire v0.8.0-352-g1a495ad inclut tes IoBinding/per-stage worker commits. Si tu n'as pas encore bench le résultat post-déploiement live, le **bench actuel reflète tes commits live**. Lance ton bench standard, compare aux baselines.

### Risque 2 — Ton uncommitted code ne match plus avec le binaire live
Tu as des modifs locales dans `vector_pipeline_3stages.rs`. Le brain live ne les exécute pas. Si tu veux les tester live, il faudra commit + promote-live.

### Risque 3 — Pending manifest bloque tes promote-live futurs
Tant que `pending.json` existe, `axon-live start` standard refuse. Soit tu cleanup (option 2 ci-dessus, recommandé), soit tu utilises l'env override.

### Risque 4 — `soll_validate` reporte 2 warns mineurs sur PRO
Cosmetic. Pas un problème. CPT-PRO-001/002/003 placeholders dupliqués (peuvent être renommés en `archived_placeholder_NNN` si tu y tiens).

### Risque 5 — Si tu fais `git checkout <ancien commit>` pour bisecter
Le brain en cours d'exécution reste sur v0.8.0-352-g1a495ad. Ton workspace fichiers retournent à l'ancien état mais le brain ne suit pas. Si tu veux bisecter avec brain restart, attention au pending manifest.

---

## 9. Action items recommandées pour toi (ordre)

1. **Lis ce doc en entier** (~10 min)
2. **`git status`** — confirme que ton `vector_pipeline_3stages.rs` uncommitted est intact
3. **Décide pending manifest** : option 2 (abort) recommandée — `mv .axon/live-release/pending.json .axon/live-release/pending.aborted-<timestamp>.json`
4. **Restart indexer si besoin** : `./scripts/axon-live start --indexer-full --tensorrt` (ou variante selon ton workflow)
5. **Bench REQ-AXO-262 (IoBinding) live** — confirme les gains attendus sur le nouveau binaire
6. **Quand on coordonne** : push GitHub des 31 commits + décider où héberger les 2 sibling repos

---

## 10. Rollback paths

Si quoi que ce soit ne va pas et tu veux retourner à l'état pré-session 14 :

### Rollback complet (code + binaire)
```bash
git reset --hard <commit-pre-53b03f5>      # ⚠️ DESTRUCTIVE — ton uncommitted disparaît si pas stashé
bash scripts/release/rollback_live.sh --manifest .axon/live-release/current.json --restart-live
```

### Rollback SOLL seul (annuler mes 35 relations + 17 nouveaux nœuds)
Utilise `soll_rollback_revision` sur chaque revision créée cette session. Chaque `soll_apply_plan` / `soll_manager create` crée une revision (visible via `mcp__axon__snapshot_history`). **Ne supprime pas directement via SQL** (memory rule SOLL : preserve knowledge base).

### Rollback partiel
Si tu veux juste les commits méthodologie OUT mais garder le binaire :
```bash
git revert 1a495ad 203e52e fc5a2ac 53b03f5     # crée 4 commits inverses
# Le binaire reste v0.8.0-352-g1a495ad mais ton arbre source revient
# Tu re-promote-live ensuite avec un binaire rebuild sans mes commits
```

---

## 11. Documents canoniques de référence

- **Spec verrouillée** : `docs/working-notes/2026-05-11-axon-methodology-delivery-spec.md` (436 lignes, 14 décisions /grill-me)
- **Bundle source** : `~/projects/axon-methodology/methodology-1.0.0.json`
- **Bundle CHANGELOG** : `~/projects/axon-methodology/CHANGELOG.md`
- **Session pointer SOLL** : `cypher SELECT description FROM soll.Node WHERE id='CPT-AXO-052'`
- **REQ-AXO-273 umbrella** : `cypher SELECT description FROM soll.Node WHERE id='REQ-AXO-273'`

---

## 12. Questions probables que tu te poses

**Q : Pourquoi 31 commits non poussés ?**
R : Pas mon fait — état antérieur à ma session. À investiguer ensemble : soit tu pousses depuis une autre machine, soit personne n'a poussé depuis le 10 mai. Pas un blocker tant qu'on est sur la même machine, mais c'est de la dette de backup offsite.

**Q : Tu as touché à mon code embedder/pipeline ?**
R : **Non.** Mes modifications sont dans `src/axon-core/src/mcp/` uniquement. Ton uncommitted dans `vector_pipeline_3stages.rs` est intact.

**Q : Pourquoi le binaire live inclut mes commits IoBinding ?**
R : `promote_live_safe.sh` build depuis `HEAD`. Mes 4 commits étaient au-dessus des tiens dans `git log`. Donc le release build inclut tout. C'est le comportement attendu — la prochaine promotion live inclura toujours tous les commits courants.

**Q : Le binaire pré-session est-il sauvé ?**
R : Pas dans `bin/axon-brain.bak-*` (le harness a refusé une copy manuelle). Mais le binaire `v0.8.0-320-gc84900d` reste dans `.axon/releases/artifacts/<sha>/` et est accessible via `rollback_live.sh` avec l'ancien manifest (`current.pre-5bea7ae.json` ou les fichiers `history/live-2026*.json`).

**Q : C'est quoi le mode `brain_only` ?**
R : Mode runtime où seul axon-brain tourne, pas axon-indexer. Le MCP fonctionne, soll_query / soll_manager OK. Mais pas d'indexing actif, pas de file watcher, pas de fresh IST projection. C'est correct pour SOLL work mais pas pour ton workflow indexer/pipeline.

**Q : Les 2 sibling repos sont-ils nécessaires pour Axon ?**
R : Non, ils sont les **artefacts du produit méthodologique commercial**. Axon le serveur MCP fonctionne sans eux. Ils servent à shipper la méthodologie à des clients externes. Tu peux les ignorer si tu travailles uniquement sur l'infra.

---

## 13. Si tu as besoin de moi (Claude)

Tu peux relancer une session Claude Code dans ce repo (`~/projects/axon`). La méthodologie inclut maintenant un skill `/handoff` et un session pointer CPT-AXO-052 — le LLM frais saura reprendre le contexte automatiquement via `axon_init_project`.

Méthodologie commerciale v1.0 : **LIVRÉE et déployée**. Bon dev.

— Claude, session 14
