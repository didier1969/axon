---
name: curate-soll
description: Recursively curate the current project's SOLL graph until fixed-point — close finished events, mark supersessions, re-level misplaced nodes, compress density. Autonomous, no operator interaction. Trigger via `/curate-soll`, or as the SOLL-cleanup step of the project's handoff procedure.
---

# curate-soll

Autonomous SOLL curator. Operator excluded for the run. Project auto-resolved via Axon MCP.

## Algorithm

```
loop:
  pass_T:  # temporal — reflect recent reality
    close finished work     → terminal status (per DEC-PRO-100 vocabulary)
    emit SUPERSEDES          replaced intent → replacement
    emit VERIFIES            delivered work → evidence
  pass_H:  # hierarchical — re-level
    elevate undersold       (REQ→CPT/PIL, DEC→PIL, …)
    demote overclaimed      (CPT→REQ, PIL→CPT, DEC→REQ, …)
  pass_D:  # density + future utility — GUI-PRO-100
    compress descriptions    > 2K chars → preserve 100% intent
    convert prose refs       "REQ-XXX-N" → native edges
    strip inline dates       → metadata or Revision
    lifecycle compress       post-delivery nodes → thin pointer
    skill size (DEC-AXO-094) docs/skills/*/SKILL.md > 5500 chars → flag
    skill cell width         any line > 200 chars in docs/skills/*/SKILL.md → flag
    skill ID annotation      (CPT|DEC|GUI|REQ|MIL|PIL)-[A-Z]{3}-[0-9]+ without `(label)` → flag
    skill immutability       SKILL.md mutated outside tool/surface change → flag (GUI-AXO-NEW)
  if last two passes produced zero mutation: stop   # fixed-point
```

## Mutation discipline

- Every mutation carries its rationale in the SOLL revision.
- Never raw SQL on intent tables. Mutate only via the canonical SOLL mutation surface — see `axon-engineering-protocol` for which MCP tool, which edge type, which status vocabulary.
- Insufficient confidence → deposit a tombstone (below). Do not mutate.

## Tombstone (graph-native, no string-match)

A `Requirement` with `status=current`, linked to the affected node by edge:

```
relation_type: TARGETS
metadata:      { "kind": "deferral" }
```

`description` = 3 sections: case observed · options considered · reason for deferral.

Discoverable by future runs:

```sql
SELECT n.* FROM soll.node n
JOIN soll.edge e ON n.id = e.source_id
WHERE e.relation_type = 'TARGETS'
  AND e.metadata->>'kind' = 'deferral'
```

Tombstones are first-class SOLL nodes; future runs curate them.

## Exit contract

The next LLM opening this project finds SOLL requiring no caveat. Sole success criterion.
