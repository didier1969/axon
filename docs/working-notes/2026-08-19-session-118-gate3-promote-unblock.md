# Session 118 — Gate 3 handoff-check + déblocage promote (2026-08-19)

Audit-only, append-only. Canonique = SOLL (CPT-AXO-052, REQ-AXO-902358/902359).

## Déclencheur
`axon init` → signal inbox VPC (msg 10522, triage CPT-AXO-025 br.2, `methodology-failure-cause`) :
`axon_handoff_check` rend PASS sur un handoff incomplet (couvre 2 des 3 gates SOLL du
Step 3 de GUI-PRO-028) + rien n'impose de relire l'inbox à la clôture. Vérifié exact sur AXO.

## Livré
- **REQ-AXO-902358** (MIL-054, delivered) — commit `fe1eefc2` :
  - Gate 3 `requirement_without_milestone` dans `axon_handoff_check` (tools_framework.rs), SQL
    canonique verbatim de GUI-PRO-028, posture DIVERGENCE DÉLIBÉRÉE préservée. Test épinglant
    étendu 2 directions. `build --tests` vert.
  - GUI-PRO-028 « MAJ 118 » : rectifie la phrase trompeuse de la MAJ 104 (« les deux blocs sql »
    → il y en a TROIS), état HONNÊTE par binaire servi (le corps SOLL est lu live mais le binaire
    retarde → ne pas réintroduire le défaut inversé dans le temps), + Step 6 relève l'inbox avant
    clôture (symétrique step 3c init).
  - Rappel inbox aussi dans `manual_reminders`.
- **REQ-AXO-902359** (MIL-055, delivered) — commits `b7254a8f` + `644b752c` — débloque le promote :
  - Symptôme : `promote_live_safe.sh` échoue DÉTERMINISTE au step 4 (manifest bg SIGTERM 143),
    3 tentatives, avant cutover (live jamais impacté).
  - RCA honnête : 1re piste (exe-filter `/proc/pid/exe` dans le reap `axon_repo_runtime_child_pids`)
    = durcissement latent RÉEL et unit-testé, mais N'A PAS débloqué (aucune ligne « Reaping » dans
    les logs → le reap était innocent). Vrai fix : le manifest bg (REQ-902188) meurt en concurrence
    avec le dev-restart du step 2 ; standalone il passe en 2s → **manifest SYNCHRONE au step 4**
    (`PROMOTE_MANIFEST_BG=1` ré-active l'overlap). Émetteur exact du SIGTERM non isolé (suivi ouvert).
- 10 REQ orphelins rattachés à MIL-054/055/056 → Gate 3 = 0 sur AXO.

## Promote
5 tentatives. 1-3 : échec step 4 (SIGTERM). 4 : tuée au step 2 sous pression host (swap 100% via
session Fiscaly concurrente ; PAS un OOM — 20 Go dispo). 5 : **clean** → live `v0.8.0-1513-g644b752c`
(cutover phase=clean, coupure MCP ~11s, pas de rollback). A aussi activé 902354/902355 (pré-session).

## Correctif de diagnostic (opérateur)
« 8go? bug? » a corrigé une double erreur de ma part : j'avais accusé le typedb OPV (172 Mo, idle)
d'un faux 13,5 Go — le vrai gros process était le typedb Fiscaly (actif, légitime). Leçon → practices
#1129 (attacher RSS↔PID, /proc/status). Rien à tuer : host légitimement chargé.

## Restant
71 REQ `delivered` sans preuve (warn handoff-check, dette historique, pas de mass-fix). 902340 P0
= arbitrage opérateur. Isoler l'émetteur du SIGTERM manifest-bg.
