# SOLL MCP Operating Procedure

Date: 2026-04-03
Status: current

## Default Procedure

For `SOLL` maintenance, the default safe procedure is:

1. `axon_validate_soll`
2. targeted query on `SOLL`
3. targeted MCP mutation
4. `axon_validate_soll` again
5. optional `axon_export_soll`

This should be the default recommendation for LLM operators. Mass restore should not be the first reflex.

## Recommended Tool Choice

### Use `axon_validate_soll`

Use it first and last.

Purpose:
- surface minimal coherence violations
- verify that a targeted change really closed the issue

### Use `axon_soll_manager`

Use it for:
- targeted `link`
- targeted `update`
- small additive edits when IDs are safe and understood

Best current use:
- repairing missing links
- updating isolated entities

### Use `axon_restore_soll`

Use it only for:
- coherent batch updates
- controlled replay from a reviewed Markdown export
- restoration after conceptual drift or loss

It is appropriate when the change spans many entities and relations and must remain Git-reviewable as one canonical document.

## What Worked Well In Practice

During the 2026-04-03 ingestion/SOLL update:

- `axon_restore_soll` worked well for replaying the additive snapshot
- `axon_validate_soll` correctly reported residual violations
- `axon_soll_manager link` worked well for the final targeted repairs
- after the 2026-04-03 hardening pass, `axon_soll_manager` also supports:
  - `vision`
  - dedicated pillar ID allocation
  - broader targeted updates with metadata preservation

This confirms the following operational pattern:

- bulk coherent update: `restore`
- final surgical repair: `soll_manager`
- proof: `validate`

## Current MCP Limitations

The current `axon_soll_manager` is useful but not yet sufficient as the only editing interface.

Observed limitations:

- `update` is now broader, but the interface is still entity-specific rather than schema-driven
- `concept` editing still relies on the canonical `CPT-...: Name` naming convention
- `soll.Registry` can still drift conceptually if modified outside the supported MCP flows

Consequence:

- `axon_soll_manager` is now suitable as the default tool for targeted maintenance
- `axon_restore_soll` still remains the better tool for reviewed batch updates across many entities and relations

## Guidance For Other LLMs

Do this by default:

1. `axon_validate_soll`
2. inspect exact entities and links
3. prefer targeted MCP edits
4. validate again

Do **not** default to full restore unless one of these is true:

- you are intentionally applying a reviewed canonical snapshot
- you must update many entities and relations at once
- the local CRUD surface is too limited for the required change

## Suggested Next Improvements

To keep improving targeted MCP maintenance:

- add explicit `get` / `list` read actions to `axon_soll_manager`
- add dry-run / preview mode for bulk link creation
- make concept lookup explicit by canonical concept ID instead of name-prefix inference
- add registry introspection and repair tooling for exceptional cases
