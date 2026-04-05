---
name: axon-digital-thread
description: Gouvernance SOLL/IST opérationnelle via MCP Axon. Utiliser pour structurer Vision/Pillar/Requirement/Decision/Concept, appliquer des plans SOLL v2 (dry-run -> commit), attacher des preuves et vérifier la couverture.
---

# Axon Digital Thread (SOLL v2 via MCP)

## Quand utiliser ce skill
- Quand il faut gouverner le fil numérique SOLL/IST d’un projet Axon.
- Quand il faut appliquer un plan SOLL via MCP avec audit trail.
- Quand il faut relier exigences, décisions, preuves, et état cockpit.

## Flux obligatoire (LLM-safe)
1. Découvrir les outils MCP actifs: `tools/list`.
2. Charger le contexte projet: `axon_soll_query_context`.
3. Préparer le plan: `axon_soll_apply_plan_v2` avec `dry_run=true`.
4. Vérifier le diff, puis valider.
5. Commit atomique via MCP: `axon_commit_work` (Vérifie le code contre les Guidelines SOLL, exporte le Markdown et exécute Git).
6. Attacher des preuves: `axon_soll_attach_evidence`.
7. Contrôler la couverture: `axon_soll_verify_requirements`.
8. En cas d’erreur: `axon_soll_rollback_revision`.

## Hiérarchie opérationnelle
- `Vision` -> stratégie.
- `Pillar` -> contraintes stables.
- `Requirement` -> objectifs vérifiables.
- `Decision` -> choix techniques.
- `Concept` -> mécanique d'implémentation.
- `Validation` -> preuve.
- `Milestone` -> jalon.
- `Guideline` -> lois d'ingénierie et règles procédurales perpétuelles (ex: TDD).

## Règles strictes
- Ne jamais inventer d’ID: utiliser les IDs gérés par `soll.Registry`.
- Ne jamais muter SOLL avec SQL brut depuis le skill: passer par les tools MCP SOLL.
- Toujours garder `logical_key` stable pour l’idempotence.
- Toute modification importante doit produire une `Revision`.

## Tools à utiliser
- `axon_soll_apply_plan_v2`
- `axon_soll_commit_revision`
- `axon_soll_query_context`
- `axon_soll_attach_evidence`
- `axon_soll_verify_requirements`
- `axon_soll_rollback_revision`
- `axon_init_project` -> initialiser un projet et lire les Guidelines Globales.
- `axon_apply_guidelines` -> hériter des Guidelines Globales pour le projet local.
- `axon_commit_work` -> **Obligatoire** pour valider et commiter le code (vérifie SOLL, génère l'export Markdown, et lance Git Commit). Ne jamais utiliser `git commit` bash.
- Compat legacy: `axon_soll_apply_plan`, `axon_soll_manager`, `axon_validate_soll`, `axon_export_soll`, `axon_restore_soll`

## Payload minimum recommandé (`apply_plan_v2`)
```json
{
  "project_slug": "AXO",
  "author": "codex",
  "dry_run": true,
  "plan": {
    "requirements": [
      {
        "logical_key": "latency-mcp-dashboard-sql",
        "title": "Latences MCP Dashboard SQL",
        "description": "Mesurer p50/p95/p99 et staleness",
        "status": "current",
        "priority": "P1",
        "owner": "platform",
        "acceptance_criteria": ["p95 mcp < 150ms", "staleness < 3s"]
      }
    ],
    "guidelines": [
      {
        "logical_key": "gui-fast-response",
        "title": "Performances Absolues",
        "description": "Toute API doit répondre sous 50ms"
      }
    ]
  },
  "relations": [
    {
      "source_id": "req-latency-mcp-dashboard-sql",
      "target_id": "gui-fast-response",
      "relation_type": "COMPLIES_WITH"
    }
  ]
}
```

*Note SOTA:* Vous pouvez utiliser vos `logical_key` directement dans le tableau `relations`. Le serveur MCP résoudra atomiquement ces clés en IDs canoniques (Zero-Shot Payload) et renverra un dictionnaire `identity_mapping` dans la réponse.

## Style attendu
- Factuel, mesurable, sans marketing.
- Chaque requirement doit être testable.
- Chaque decision doit contenir contexte + rationale + impact.
