# SOLL Create+Attach Concept

Date: 2026-04-19
Branch: `feat/live-dev-dual-instance`
Status: CDD Phase 1

## Problem

Axon SOLL is now mostly correct and discoverable, but authoring still has too much interaction friction.

Common current loop:

1. create node
2. inspect relation policy
3. create link

This is canonically correct, but too expensive for normal graph authoring.

## Goal

Reduce SOLL authoring friction by allowing one-step canonical create+attach for the obvious graph-building cases, while keeping the server authoritative over relation legality.

## Reality Check

We already have the ingredients:

- `soll_manager create`
- `soll_manager link`
- `select_relation_type_for_link(...)`
- `insert_validated_relation(...)`
- `soll_relation_schema`

So the next step should reuse the existing truth path, not introduce a second orchestration layer.

## Design Direction

Extend `soll_manager` `action=create` with optional canonical attach arguments:

- `attach_to`
  - canonical existing target id
- `relation_hint`
  - optional explicit relation when the pair has no default or is multi-valued

This is about canonical SOLL graph attachment only.
It does not overlap with `soll_attach_evidence`, which remains the dedicated traceability/evidence surface.

Example:

- create `requirement`
- `attach_to = PIL-NTO-001`

Server behavior:

1. create the node normally
2. attempt canonical relation resolution between the new node and `attach_to`
3. if the relation is unambiguous, insert the edge in the same operation
4. if ambiguous or invalid:
   - keep the create path truthful
   - do not invent a link
   - return explicit structured attach guidance

## Canonical Rule

The server remains authoritative.

- no client-defined arbitrary edge patterns
- no heuristic relation invention outside server policy
- relation choice must come from the same policy used by `soll_manager link`

## Minimal API Change

No new top-level tool in the first wave.

Use:

- `soll_manager`
  - `action=create`
  - optional `data.attach_to`
  - optional `data.relation_hint`

This keeps the public surface stable.

## Success Shape

Successful create+attach response should include:

- `created_id`
- `entity_type`
- `project_code`
- `attached`
- `attached_to`
- `applied_relation`
- `canonical_next_links`

Attach failure after successful create should include:

- `created_id`
- `attach_attempted`
- `attach_status = needs_relation_hint | invalid_target_kind | invalid_target_id`
- structured relation guidance derived from canonical policy

## Non-Goals

- no new high-level batch helper
- no automatic graph completion
- no ontology rewrite
- no hidden client heuristics

## Reuse vs Change

Reuse:

- `select_relation_type_for_link(...)`
- `insert_validated_relation(...)`
- current `soll_manager create`
- current relation guidance payloads

Change:

- extend `soll_manager create` request shape
- extend `soll_manager create` response shape
- add tests for create+attach happy and unhappy paths
- update skill guidance

## Risks

Main risk:

- partial success confusion

So the contract must clearly distinguish:

- node created
- node created and attached
- node created but attach refused

That distinction must be explicit in `data`, not only prose.
