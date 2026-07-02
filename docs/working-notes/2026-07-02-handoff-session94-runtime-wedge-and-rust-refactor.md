# HANDOFF — Session 94 (2026-07-02) — incident runtime wedge GPU + début refactor bash→Rust

> Handoff produit **sans Axon** (MCP down, runtime mort). Fallback markdown sanctionné (CLAUDE.md : markdown seulement si MCP irrécupérable). À reprendre par le prochain LLM / moi après reset WSL.

## ⛔ BLOQUEUR CRITIQUE — à lire en premier
Le **driver GPU WSL2 est planté**. 5 process en état `D` (uninterruptible, unkillable) — 4× `nvidia-smi` + `axon-indexer` — le plus vieux à 1h28. Ils tiennent des ressources/guards. **Tout `start`/`stop`/`promote` hang dessus** (chaque `nvidia-smi` appelé repart en `D` ; `stop --hard` hang à reaper le `D`). Le runtime live est **irrécupérable par logiciel**.

**SEULE reprise possible** (opérateur, depuis Windows) :
```
wsl --terminate Ubuntu     # chirurgical : ce distro seul (pas --shutdown qui tue tout)
```
Ça clère les `D`/zombies + réveille le GPU. `wsl --shutdown` marche aussi (plus large). L'opérateur a d'abord exclu `--shutdown` ; `--terminate Ubuntu` lui a été proposé comme minimum disruptif.

## Reprise scriptée (à exécuter dès que WSL redémarre)
1. `AXON_EMBEDDING_PROVIDER=cpu AXON_SKIP_GPU_DETECT=1 ./scripts/axon-live start brain` → brain_only CPU (bypass GPU), MCP revient. Vérifier `curl -s :44129/readyz`.
2. `mcp__axon__embed_provider action=set provider=cpu` (query-lane CPU, éviter tout wedge GPU).
3. Finaliser le promote 1288 : `./scripts/axon promote-live --manifest .axon/releases/candidates/0.8.0-v0.8.0-1288-g71e0c28f.json --restart-live --resume` (GPU sain cette fois). Vérifier `promote_status`=clean.
4. **Committer** la brique `guard_liveness` (via `axon_commit_work`, MCP up) — voir §Uncommitted.
5. Écrire les entrées SOLL stagées : voir `docs/working-notes/2026-07-02-STAGED-soll-runtime-refactor-and-incident.md`.

## Git — ce qui est SÛR
- `HEAD = origin/main = 71e0c28f` (poussé). 2 quick-wins livrés+poussés :
  - **REQ-AXO-902160** `3ddfe34d` — `kickoff_bundle.derived_session_pointer` (auto-dérivé git HEAD + REQ + commits ; helper pur `derive_session_pointer`, 2 tests). LIVE-PENDING.
  - **REQ-AXO-902161** `71e0c28f` — `soll_manager(action=append_section)` (update sans renvoi du corps, délègue à update ; contrat dérivé ; test). LIVE-PENDING.
- `bin/` = binaire **1288** (staged, sha256-cohérent) ; manifest `current.json` = **1286** (promote NON finalisé). Le live tournait 1286 avant l'incident.

## Uncommitted sur disque (à traiter)
1. `src/axon-core/src/runtime_writer_guard.rs` — **brique refactor #1 : `guard_liveness` (Rust, flock-truth, zombie-safe)** + 3 tests verts (`guard_liveness_*`). = REQ-AXO-902157 slice. **À committer** dès MCP up. Fonctions : `guard_liveness_ist/soll(db_root) -> GuardLiveness::{Free,HeldByLiveProcess}`. Teste le flock (vérité kernel), PAS le `pid=` métadonnées.
2. `scripts/start.sh` — édit `detect_gpu` (bypass env `AXON_EMBEDDING_PROVIDER=cpu`/`AXON_SKIP_GPU_DETECT=1`). **Symptôme, non testé, viole les règles (auto-audit). À DÉCIDER : formaliser sous REQ-slice S2 AVEC test, ou revert.** Ne PAS committer tel quel.
3. `docs/working-notes/2026-07-02-STAGED-soll-runtime-refactor-and-incident.md` — entrées SOLL à tirer.

## RCA de l'incident (à verser dans REQ-AXO-902157 via append_section — DOGFOOD)
- **Cause #1 (bloque restart)** : `scripts/stop.sh` `pid_exists()` = `[ -e /proc/$pid ]` → VRAI pour un zombie. Superviseur process-compose reapé AVANT ses enfants → zombies orphelins (systemd WSL2 ne reape pas) → `verify_writer_guard_release` voit le guard « tenu » (owner zombie) → refuse le restart. **Le guard Rust (`flock`) est DÉJÀ correct** (libéré à la mort, zombie inclus) ; c'est la re-vérif bash qui est buggée. → fix = `guard_liveness` Rust (fait) + arrêter la devinette bash.
- **Cause #2 (bloque start)** : `scripts/start.sh` `detect_gpu()` utilise encore `nvidia-smi -L` (CLI) — LE straggler non migré vers NVML (reste du codebase = `scripts/lib/gpu_nvml.py`, `observed_gpu.rs`). Driver wedgé → `nvidia-smi` en `D` → start hang à l'infini (`timeout`/SIGKILL sans effet sur un `D`).
- **Cause #3 (cliquet)** : chaque restart laisse un nouveau `D`/zombie tenant un guard → start suivant plus bloqué.

## 🎯 DÉCOUVERTE ARCHITECTURALE MAJEURE (le vrai sujet)
**Déviation confirmée (preuve code)** vs intention (axonctl=superviseur Rust, DEC-AXO-060, PIL-AXO-9005 control-plane déclaratif) :
- `axonctl cmd_start` (bin/axonctl.rs:361) = **`exec bash scripts/axon start`** → zéro orchestration Rust ; toute la logique start vit dans `start.sh`.
- `promote` **n'existe pas** dans axonctl → 1200+ lignes de bash (`promote_live*.sh`).
- `axonctl cmd_stop` (:414) a une vraie FSM Rust (StopReport, REQ-AXO-902111) → migration COMMENCÉE mais inégale.
- Le guard Rust utilise `flock` (correct) ; le bash le re-devine en PIRE (le bug). **Le bash ré-implémente ce que Rust fait déjà mieux.**

**Position opérateur (2026-07-02, à graver)** : le bash ne devrait RIEN faire d'autre qu'appeler le Rust ; la complexité bash n'est pas justifiée, c'est de la dérive. **Cible : bash = shim ~20 lignes (entrer devenv + `exec axonctl`), voire ZÉRO bash via Nix `makeWrapper` (baker LD_LIBRARY_PATH/ORT dans le binaire). axonctl (Rust) possède start/stop/status/qualify/promote en réconciliateur blue-green.**

## Plan de refonte (6 REQ-slices — détail dans le doc STAGED)
Décision de refonte : « Rapatrier l'orchestration start+promote dans axonctl (réconciliateur blue-green) ». Pattern déjà en tree (cmd_stop-FSM + `promote_status`). Slices par ROI :
- S1 [P1] Guards → `pg_advisory_lock` PG (auto-release à la mort) — OU garder flock + `guard_liveness` (fait). Tue la classe zombie-stale.
- S2 [P1] `detect_gpu` → NVML non-bloquant (straggler nvidia-smi). LIER à la REQ NVML ouverte (chercher `soll_query_context "nvml nvidia-smi"`).
- S3 [P1] Bascule blue-green dans axonctl (shadow-port → health-gate liveness runtime_contract COMPLET + qualify → flip atomique → drain ancien). Promote raté = ZÉRO panne.
- S4 [P2] `pid_exists` zombie-aware + wirer le bash verify sur `guard_liveness` Rust.
- S5 [P2] Vérifier runtime_contract complet avant qualify.
- S6 [P2] systemd units remplacent process-compose + PID-files.

## Autres fils de la session (OPV inbox msg [16] — étude de faisabilité 4 limites)
- Umbrella **REQ-AXO-902158** (BELONGS_TO PIL-AXO-002). OPV notifié 2×.
- **REQ-AXO-902159 P0 (runtime/monde-vivant) = REJECTED** (dérive APM, dilue le moat ; verbe `measure` exploré puis écarté). Réouverture seulement sous cadrage intention↔runtime en DEC.
- **902160 (session_pointer) = LIVRÉ**, **902161 (patch/append) = LIVRÉ** (les 2 quick-wins ci-dessus).
- **902162 (index-freshness) P2 = ouvert** (measure-first, mieux mesuré sur live fraîche post-recovery).
- Friction-bug corroboré : `practice_recall` renvoie 0 corps (reproduit) — canal `mcp_feedback`.

## Discipline (l'opérateur a repris la session sur ce point)
GUI-PRO-114 (nouvelle règle dure CLAUDE.md) : Axon MCP PRIORITÉ ABSOLUE ; `query`/`inspect`/`impact` avant grep ; `impact` avant refactor ; JAMAIS `git commit` brut (→ `axon_commit_work`) ; **REQ SOLL AVANT tout travail** ; RCA root-cause pas symptôme (GUI-PRO-106) ; TDD ; ne pas s'acharner en cassant. Auto-audit s94 : mon édit `start.sh` a violé ces règles (symptôme, sans REQ, sans test, thrashing) — à corriger.

## Runtime facts (vérifiables)
Live 44127-44132 / dev 44137-44142 · PG `:44144` up · brain=MCP+SOLL writer · indexer=IST writer. `AXON_EMBEDDING_PROVIDER=tensorrt` pinné dans `.axon/runtime-config.live.env` (écrase le shell après detect_gpu). `psql` absent (utiliser MCP `sql`).
