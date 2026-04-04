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
5. Commit atomique: `axon_soll_commit_revision`.
6. Attacher des preuves: `axon_soll_attach_evidence`.
7. Contrôler la couverture: `axon_soll_verify_requirements`.
8. En cas d’erreur: `axon_soll_rollback_revision`.

## Hiérarchie opérationnelle
- `Vision` -> stratégie.
- `Pillar` -> contraintes stables.
- `Requirement` -> objectifs vérifiables.
- `Decision` -> choix techniques.
- `Concept` -> mécanique d’implémentation.
- `Validation` -> preuve.
- `Milestone` -> jalon.

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
    ]
  }
}
```

## Style attendu
- Factuel, mesurable, sans marketing.
- Chaque requirement doit être testable.
- Chaque decision doit contenir contexte + rationale + impact.
