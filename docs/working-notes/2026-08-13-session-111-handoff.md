# Session 111 handoff (2026-08-13) — livré + incident promote → ✅ RÉSOLU le 2026-08-14

> Canonique = SOLL. Ce fichier = audit. Probe `CPT-AXO-052` + `git log` d'abord.

## ✅ CLÔTURE (2026-08-14) — incident résolu, cause traitée à la source, ne rien reprendre ici
- **VM rebootée** → zombie disparu, canal `dxgvmb` libre.
- **agent-deck a RELU sa config sans-gpu** (démarré 06:34:52, après le boot 06:33:14). Vérifié empiriquement : **0 `nvidia-smi` sur 25 s** (5 fenêtres de poll), descendants = clients tmux seuls.
- **2 promotes réussis** : live = **v0.8.0-1454-ge3aabb67**, 4 rôles Ready. `d_state=0`, **teardown indexeur 65-66 ms** (contre blocage infini la veille), step 2d PASS en 19-22 s (contre 206 s d'échec) → la RCA de collision est confirmée par la mesure.
- **Frictions 2 / 125 / 144 FERMÉES** sous REQ-902289, chacune avec preuve E2E live (entity inféré / enum status publié / help suggère).
- **Fix durable livré `e3aabb67`** (REQ-902271 levier 3) : l'indexeur sort en `libc::_exit(0)` — aucun deinit GPU sur le chemin d'arrêt. Protège les arrêts FUTURS (le promote qui l'a déployé arrêtait encore l'ancien binaire).
- **Levier 2 ABANDONNÉ** après vérification (REQ-902293, rétrogradé P3) : le « promote brain-only » n'existe pas — les 3 binaires partagent `axon-core` (empreinte `axon-indexer` CHANGÉE alors que le diff était 100 % `mcp/*`). Le redémarrage était légitime.

> Ce qui suit est le compte rendu de l'incident, conservé pour la RCA. **La séquence de reprise §3 est CADUQUE** (exécutée avec succès).

## 1. Livré et POUSSÉ (origin/main jusqu'à `2080854c`)
5 REQ clôturés `delivered`, 3 commits :
- `18aba16d` test(mcp) — preuve E2E rejouable `scripts/test_mcp_cwd_resolution.py` (REQ-902286/902287). Tunnel lancé depuis cwd=AgriOptim → **AGO** ; cwd non-enregistré → refus explicite. 902288 déjà résolu (friction #68).
- `07dc153f` fix(mailbox) — **REQ-902278** : `reject_body_less_send` refuse un envoi sans corps (direct + fan-out), `body_dense` required au schéma. RCA corrigée : pas de « générateur d'alerte » fautif, c'est le CONTRAT qui acceptait le dead-end (5 corps vides/6356).
- `2080854c` fix(mcp) — **REQ-902289** : `entity` inféré du préfixe d'id (RCA : champ OMIS, pas valeur invalide) ; `data.status` publie son enum dérivé de `CANONICAL_NODE_STATUSES` ; `help unknown_tool` suggère les noms proches (containment-first + Levenshtein).

**Fermeture des frictions 2/125/144 EN ATTENTE de promote + preuve E2E live** (le brain sert encore `be1771bc` ; ne pas `mark_resolved` avant — motif « 8/9 régressées faute de preuve »).

## 2. Trouvé et tracé (non corrigé)
- **REQ-902291** (P1) — `soll_query_context` rend le SOLL d'AXO à un client AgriOptim : ~10 handlers repliant sur le littéral `"AXO"` hors allow-list `PROJECT_AUTORESOLVE_TOOLS`. Défaut de correction → défaut d'isolation dès qu'Axon sert >1 principal. Déclaré known-gap dans le script E2E.
- **REQ-902292** — `mcp_friction_report` annonce N signatures et n'en imprime aucune (data l'a, `content[0].text` l'omet — classe REQ-902279).
- **REQ-902293** (P1) — le **step 2d du promote INDUIT la panne** : il restart-vérifie l'indexeur LIVE servant → laisse le live pire qu'à t0. REFINES 902256.

## 3. ⛔ INCIDENT EN COURS — live dégradé, VM À REBOOTER
**Promote `20260813T200615Z` échoué au step 2d** (AVANT cutover → brain jamais swappé, commits PAS live). L'indexeur live (pid 495336) est **zombie-Terminating** : thread tokio 497682 en **D ininterruptible sur `dxgvmb_send_sync_msg`** (canal GPU vmbus WSL2). Inkillable (SIGKILL sans effet sur D), process-compose ne le reap jamais.

**Débordement sur le brain** : mes requêtes `semantic` ont déclenché un embed GPU → le brain a maintenant un thread en D sur `dxgglobal_acquire_process_adapter_lock` → `status`/`retrieve_context` via `/mcp` **timeout** (`/readyz` répond encore). NE PAS relancer de requêtes sémantiques (ça ajoute des D-threads).

### Root cause de la RÉCURRENCE (4 wedges/191 promotes — corps complet dans REQ-902271)
Collision sur l'**unique canal GPU série de WSL2** (`dxgvmb`) entre 2 consommateurs : (1) l'indexeur qui fait des appels CUDA/TensorRT **synchrones in-process** à chaque start/stop (teardown = `GpuB2Embedder::drop_session` `embedder_gpu.rs:109`, appelé via le graceful shutdown `runtime_boot.rs:1189` REQ-902233) ; (2) agent-deck qui poll `nvidia-smi` **toutes les 5 s**. Collision dans la fenêtre de teardown, sur le driver NVIDIA chroniquement instable de ce host → wedge permanent.
**Enableur de récurrence** : le fix session-109 (`~/.agent-deck/config.toml` sans `"gpu"`) est sur le disque mais **agent-deck tourne depuis 10 j et n'a jamais relu sa config** → il poll toujours le GPU. Le fix n'a jamais été mis en service.

### Séquence de reprise (OPÉRATEUR — ordre STRICT)
1. **`wsl --shutdown`** depuis Windows (PowerShell/cmd). SEUL remède au `dxgvmb` jammé. ⚠️ tue TOUTE la VM : autres sessions Claude sous tmux (server 1784540), postgres de tous les projets, watchman.
2. Au retour, **restart agent-deck** pour qu'il relise la config sans-gpu (ou vérifier : aucun `nvidia-smi` parenté par agent-deck). SINON on remet la pièce dans la machine.
3. `./scripts/axon-live start --indexer-full` (standing = indexer_full GPU, NE PAS basculer CPU-embed — directive permanente).
4. Vérifier : `curl :8080/processes` (4 rôles Running), code-intel via `status`, `pgrep -c rustc`=0 + D-state GPU=0 avant tout promote.
5. **Re-promote** : `bash scripts/release/promote_live_safe.sh --project AXO` (build `2080854c` déjà poussé ; `pending.json` absent, rien à rejouer). Avec le poll agent-deck coupé → pas de collision.

### Fix durable À CODER (tue la classe, pas encore fait)
- **Levier 3 (le plus robuste)** : le shutdown indexeur ne doit JAMAIS faire de deinit GPU synchrone. Cible = le drop du `GpuB2Embedder` sur le chemin SIGTERM (`runtime_boot.rs:1189` → destructeurs → `embedder_gpu.rs:109`). Option : `_exit()` dur après flush non-GPU (l'indexeur est un writer dérivé idempotent, rien à perdre ; le contexte GPU est repris par l'OS à l'exit). Testable en unité (handler) mais l'E2E GPU exige le GPU sain.
- **Levier 2 (REQ-902293)** : le promote ne doit pas restart l'indexeur live pour un commit brain-only (mes 3 commits = 100% `mcp/*`). Le step 2d + cutover l'ont recyclé pour rien.
- **Levier 1 (prévention, opérateur)** : mettre en service le fix agent-deck (restart pour reload no-gpu).

## 4. Runtime au moment du handoff
- LIVE = `be1771bc` (v0.8.0-1449), HEAD = origin/main = `2080854c` (3 commits ahead, pas promus).
- Brain : `/readyz` ok mais `/mcp` dégradé (thread D sur lock GPU). Indexeur : zombie-Terminating. `pending.json` absent, `current.json`=be1771bc.
- `soll_validate` = 0 violation (avant le wedge). Suite : gate filtrée verte sur la surface changée ; flake préexistant `embedder::semantic_policy` (REQ-902274) rouge en run complet parallèle, sans rapport.
