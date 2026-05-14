# SOLL Derived Docs Auto-Sync Implementation Plan

Date: 2026-04-19
Status: draft for CDD review

## Objective

Turn derived SOLL docs into an automatically maintained, incrementally updated human-readable documentation surface with:

- a global root for all projects
- one isolated site per project

## Phase 1: Root Model

1. Add canonical root output:
- `docs/derived/soll/index.html`

2. Build project summaries from canonical identity + SOLL counts:
- project code
- project name
- node counts
- link target

3. Keep it explicitly marked as derived and non-canonical.
4. Add visible freshness metadata:
- last generated timestamp
- generation mode `full|incremental`
- explicit derived/non-canonical badge
5. Use a stable root ordering:
- project code first
- then project name

## Phase 2: Generator API

1. Extend generator entrypoint to support:
- single project generation
- root-only generation
- all-project generation

2. Add mode selection:
- `full`
- `incremental`
- `auto`

Default:

- `auto`

## Phase 3: Incremental Contract

1. Extend manifest with:
- generator version
- project summary hash
- page dependency list
- last full generation timestamp

2. Add invalidation helpers:
- changed node ids
- neighbor ids
- affected subtree roots
- root index impact

3. Add manifest-driven cleanup:
- remove pages present in the previous manifest but no longer emitted
- do not leave stale node or subtree pages behind

4. If manifest is missing or incompatible:
- fall back to full build

## Phase 4: Auto-Sync Hooks

1. Trigger after successful sync mutations:
- `soll_manager`
- `soll_attach_evidence`
- `soll_commit_revision`
- `soll_rollback_revision`
- `axon_init_project`

2. Trigger after successful async mutation completion:
- `soll_apply_plan`
- any future heavy SOLL mutation jobs

3. Pass the affected `project_code` and changed ids into the generator.
4. Implement one shared refresh boundary per execution class:
- one sync post-mutation hook
- one async completion hook
- no ad hoc per-tool divergence

## Phase 5: Operational Guardrails

1. Best-effort only for first wave:
- mutation success remains primary
- doc refresh failure is visible but does not corrupt SOLL mutation result

2. Emit explicit refresh metadata:
- refreshed root yes/no
- refreshed project yes/no
- full vs incremental
- changed pages count
- stale docs yes/no
- refresh error text when refresh fails

3. Keep commit hygiene:
- no broad auto-stage of generated trees

## Phase 6: Validation

1. Unit tests
- full build when no site exists
- incremental build when manifest exists
- only changed files are rewritten
- root page includes all projects
- per-project site excludes foreign project nodes

2. Integration tests
- mutate one project, confirm only its site and root update
- init a new project, confirm root gains the new project and project site is created
- async mutation completion refreshes docs
- delete or restructure a subtree, confirm obsolete pages are removed
- successful mutation + failed refresh surfaces machine-readable stale status

3. Runtime validation
- run on `AXO`
- run on at least one second project such as `NTO`

## Risks To Watch

1. Hook placement drift across mutation paths
2. stale manifest assumptions
3. excessive root regeneration
4. user confusion between derived site and canonical export

## Rollout

1. ship behind the existing public tool surface
2. validate on dev
3. promote
4. only then consider making the derived site the default human-facing reading path
