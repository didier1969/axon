# SOLL Canonical IDs and Project-Scoped Audit

## What changed

- SOLL entity IDs are now server-owned and canonicalized as `TYPE-CODE-NNN`.
- `project_slug` is resolved by Axon through `soll.ProjectCodeRegistry`.
- `BookingSystem` now maps to `BKS`, so server-generated IDs use forms like `DEC-BKS-001`.
- `validate_soll` and `export_soll` now accept optional `project_slug` and apply real backend filtering.

## Operational contract

- `create`: send `project_slug` plus business fields; the server returns the canonical ID.
- `update`: canonical `id` is mandatory.
- `link`: canonical `source_id` and `target_id` are mandatory.
- `validate_soll(project_slug=...)`: project-only invariants.
- `export_soll(project_slug=...)`: project-only snapshot.

## Schema notes

- `soll.ProjectCodeRegistry` is the source of truth for `project_slug -> project_code`.
- `Vision`, `Concept`, and `Stakeholder` now carry server identity and project metadata.
- Startup migration rewrites legacy long-slug IDs and propagates replacements through SOLL relation tables and traceability references.

## Current mappings

- `AXO -> AXO`
- `BookingSystem -> BKS`
