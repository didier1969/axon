# STAGED SOLL writes — à exécuter dès que MCP revient (fallback markdown, MCP irrécupérable 2026-07-02)

Contexte : promote v0.8.0-1288 échoué (step 5 restart) ; cascade wedge GPU WSL2
(`nvidia-smi` en D-state unkillable) + bug guard/zombie → live DOWN irrécupérable
sans `wsl --shutdown` (exclu par l'opérateur). MCP down → SOLL non-écrivable en direct.

## A. Append RCA à REQ-AXO-902157 (soll_manager action=append_section — DOGFOOD le nouveau tool)
section_title: "RCA complète (incident promote 1288, s94)"
section:
- Cause #1 (bloque le restart) : `pid_exists()` (scripts/stop.sh:177) = `[ -e /proc/$pid ]`
  → VRAI pour un zombie. Superviseur process-compose reapé AVANT ses enfants → zombies
  orphelins (systemd WSL2 ne les reape pas) → guards writer stale (owner zombie) → la
  hard-stop-verification refuse le restart. Fix : `pid_exists` doit exclure l'état `Z`.
- Cause #2 (bloque le start) : `detect_gpu()` (scripts/start.sh) utilise encore
  `nvidia-smi -L` (CLI) — LE straggler non migré vers NVML (le reste du codebase utilise
  `scripts/lib/gpu_nvml.py` / `observed_gpu.rs`). Driver GPU wedgé → `nvidia-smi` part en
  D-state (unkillable, `timeout`/SIGKILL sans effet) → start hang à l'infini.
- Cause #3 (effet cliquet) : chaque restart laisse un nouveau zombie/process-D tenant un
  guard → start suivant plus bloqué. Recovery impossible sans clear GPU (wsl --shutdown).
- Guard-takeover absent : un guard détenu par un owner mort/zombie n'est pas récupéré
  automatiquement (rm manuel × plusieurs cette session).

## B. Décision de refonte (soll_manager create decision, BLOCKED_BY? ou BELONGS_TO PIL-AXO-9005)
title: "Rapatrier l'orchestration start+promote dans axonctl (réconciliateur blue-green) — corriger la déviation control-plane"
body:
- DÉVIATION constatée (preuve code) vs intention (axonctl=superviseur, DEC-AXO-060, PIL-AXO-9005 control-plane déclaratif) :
  - `axonctl cmd_start` (bin/axonctl.rs:361) = `exec bash scripts/axon start` → ZÉRO orchestration Rust ; toute la logique start vit dans start.sh (bash).
  - `promote` N'EXISTE PAS dans axonctl → 1200+ lignes de bash (promote_live*.sh) possèdent toute la bascule/guards/verify/resume.
  - `axonctl cmd_stop` (:414) a une vraie FSM Rust (StopReport, REQ-AXO-902111) → migration COMMENCÉE mais inégale.
- CIBLE (conforme PIL-AXO-9005, pattern déjà en tree via cmd_stop-FSM + promote_status) :
  1. Réconciliateur déclaratif dans axonctl : desired-state = manifest ; converge observed→desired.
  2. Bascule BLUE-GREEN health-gated : nouveau sur ports shadow → gate (readyz + liveness runtime_contract COMPLET + qualify) → flip atomique → drain+stop ancien. Promote raté = ZÉRO panne.
  3. Liveness par la VÉRITÉ (répond sur socket / tient un lease PG), jamais `/proc/pid`.
  4. Guards = `pg_advisory_lock` PostgreSQL (auto-release à la mort de la connexion) au lieu de lock-fichiers.
  5. Sonde GPU = NVML non-bloquante en arrière-plan, JAMAIS une porte de start.
  6. Supervision = systemd units (il EST pid 1, il reape) au lieu de process-compose + PID-files.

## C. REQ-slices (children REFINES de la Décision B), priorité par ROI stabilité
- S1 [P1] Guards → `pg_advisory_lock` (remplace lock-fichiers) — tue la classe zombie-stale-lock. Petit, énorme.
- S2 [P1] `detect_gpu` → NVML non-bloquant (straggler nvidia-smi) — root-cause du wedge ; LIER à la REQ NVML ouverte existante (à retrouver via soll_query_context "nvml nvidia-smi").
- S3 [P1] Bascule blue-green dans axonctl (shadow-port → health-gate → flip atomique → drain).
- S4 [P2] `pid_exists` zombie-aware + guard-takeover owner-mort (REQ-AXO-902157).
- S5 [P2] Vérifier runtime_contract COMPLET avant qualify (REQ-AXO-902157).
- S6 [P2] systemd units remplacent process-compose + PID-files.

## D. Interim non committé à statuer
- `scripts/start.sh` `detect_gpu` : bypass env `AXON_EMBEDDING_PROVIDER=cpu` / `AXON_SKIP_GPU_DETECT=1`
  (édit non committé, non testé, symptôme). Décision : soit formaliser sous S2 AVEC test, soit revert.
  Ne PAS committer tel quel (viole GUI-PRO-001/106/114 — cf. auto-audit s94).

## Git safe : HEAD=71e0c28f poussé (2 quick-wins 902160/902161 committés+poussés). bin/=1288 staged, manifest current=1286.
