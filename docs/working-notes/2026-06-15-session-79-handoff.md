# Session 79 — handoff (2026-06-15)

Audit-only narrative. Canonical actionable state = SOLL `CPT-AXO-052`. This file = prose context only.

## Arc de session
Démarrée sur un triage « demandes clients d'abord, bugs prioritaires ». Découverte structurante : **les demandes clients arrivent par 2 canaux** (SOLL re-homing depuis les tenants + logs friction/telemetry) — formalisé `CPT-AXO-90048`. Un rapport client **NEX** (Elixir + NIF Rust) puis un consumer **HYC** ont déposé des REQ directement en SOLL et re-vérifié les correctifs (5/6 prouvés).

## Livré + promu (live v0.8.0-1047-g39a9ecc7)
- **Bug cross-LLM #1** : `soll_manager` advertisé avec un schéma conditionnel `allOf/if/then` (le SEUL des 72 outils) → droppé au binding par les clients MCP → « capability absent » → grep. Aplati (REQ-901991). Vérifié curl : `tools/list` = NONE conditional.
- **entrench Writer Error** (901995/988) : `expand_named_params` échappait les apostrophes seules ; JSON metadata embarque backslashes/backticks → Writer Error. Fix de classe = dollar-quoting (`pg_str_literal`) sur TOUT `execute_param`. Vérifié live.
- CALLS Elixir case/with/pipe/args (901969) · NIF cross-language loader is_nif-scoped (901986) · embed-pressure GPU découplé (901987) · proof_gap remediation (901989) · control-plane guard cwd (901968) · project_status timings (901982) · gitignore purge (901950).
- 6 bugs DX clients (901994/996/997/998/999/993-R1).

## Thèse produit (réponse à « comment forcer les LLM sur Axon »)
On ne force pas par le prompt — le LLM dérive vers grep sous pression de latence. Deux leviers : (A) **contrainte environnementale** = PreToolUse hook `scripts/hooks/axon-mcp-first-guard.py` (bloque grep/rg/find code → redirige MCP ; fail-open si Axon down) ; (B) **supprimer les causes de défection** (latence < 500ms, jamais mentir, binding fiable, sortie > grep). Formalisé `CPT-PRO-100` + `GUI-PRO-112` (méthodologie livrée aux tenants).

## Décision test-infra
`DEC-AXO-901634` : abandon de la couche scoping PG-éphémère (choix X opérateur). Le clone `create_test_db` (REQ-901877) RESTE (load-bearing pipeline tests) ; la couche scoping (scoped_test_project_code 59 sites + helpers + wipe) est redondante sous le clone → retrait = `REQ-AXO-902001` (spec prête, non rushé avant handoff). 91560/718/720 rejetés.

## Reste (cf CPT-AXO-052 pour les 3 next actions)
902001 (cleanup X) · 901976 (relevance gate — implémentation MESURÉE, l'auteur prévient regression) · 901934 + Wave 5.

## Leçons méthodo
- Piloter `soll_manager` en curl POST :44129/mcp quand le registre client est périmé (901994).
- `tail -f|grep` Monitor race la création du log → s'appuyer sur la notification de complétion (mémoire feedback_monitor_tail_race).
