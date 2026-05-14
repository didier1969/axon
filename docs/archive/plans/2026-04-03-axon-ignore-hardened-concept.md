# Axon Ignore Hardened Concept

Date: 2026-04-03
Status: proposed canonical concept
Scope: sovereign filtering policy for watcher, scanner, ingress, and project workspace coverage

## Goal

Freeze a single, explicit filtering concept for Axon so future changes do not drift between:

- the useful workspace universe
- technical noise exclusion
- project-local exceptions
- watcher-time vs scanner-time behavior

This document is normative for the filtering model.

## What Was Re-Verified

Current files found:

- engine-level file exists: `/home/dstadel/projects/axon/.axonignore`
- template exists: `/home/dstadel/projects/axon/templates/.axonignore`
- global workspace file exists: `/home/dstadel/projects/.axonignore`

Current engine-level rules in `/home/dstadel/projects/axon/.axonignore`:

- `.git`
- `.axon`
- `_build`
- `deps`
- `.elixir_ls`
- `**/*.log`
- `target/`
- `src/dashboard/priv/static/`
- `node_modules/`
- `*.db`
- `*.db-wal`
- `*.db-shm`

Current template rules in `templates/.axonignore` are broader and already include categories such as:

- VCS noise
- env noise
- build/dependency trees
- caches/logs
- Axon internals
- DB artifacts
- binaries/media

## Canonical Universe Rule

Axon useful scope is:

- all readable immediate project roots under `/home/dstadel/projects`
- with `axon` prioritized, but not exclusive

This means:

- we do not shrink the useful universe to `axon` only
- we do not exclude worktrees or projects by guesswork alone
- filtering must remove noise inside that universe, not silently redefine the universe

## Canonical Axon Ignore Hierarchy

The hardened hierarchy is:

1. runtime overrides
2. `.axonignore.local`
3. nearest `.axonignore`
4. ancestor `.axonignore`
5. hard technical Axon protections

Applied to this workspace, the intended levels are:

1. `/home/dstadel/projects/.axonignore`
   - agency/workspace scope
   - can exclude whole project roots or shared technical trees

2. `/home/dstadel/projects/axon/.axonignore`
   - engine-level technical defaults
   - applies to the whole workspace when the scan root is `/home/dstadel/projects`

3. `/home/dstadel/projects/<PROJECT>/.axonignore`
   - project-specific filtering
   - may re-include with `!pattern`

4. `/home/dstadel/projects/<PROJECT>/.axonignore.local`
   - machine-local or operator-local exceptions
   - must not be treated as canonical team policy

## Golden Rule

Markdown remains strategically important.

The filtering concept must preserve the possibility that project understanding files stay indexable, especially:

- `README.md`
- planning files
- progress notes
- architecture notes
- operational docs

This does not mean "all markdown always wins over everything".

It means:

- markdown must not be accidentally lost because of broad technical ignore rules
- if broad markdown exclusion is ever introduced, explicit allow-rules must remain possible and visible

## Hard Technical Protections

Some exclusions are unconditional technical protections and should not depend on project taste.

The protected classes are:

- VCS internals
  - `.git/`
- Axon internal storage and runtime artifacts
  - `.axon/`
  - `*.db`
  - `*.db-wal`
  - `*.db-shm`
- build and dependency trees
  - `node_modules/`
  - `target/`
  - `_build/`
  - `deps/`
- volatile IDE and cache trees
  - `.elixir_ls/`
  - `.devenv/`
  - `.venv/`
  - `__pycache__/`
  - `.pytest_cache/`
  - `.mypy_cache/`
  - `.fastembed_cache/`

These protections must apply as early as possible:

- watcher directory event filter
- scanner path filter
- ingress reduction

They must not wait until late canonical promotion to DuckDB.

## Watcher vs Scanner Contract

The filter must behave consistently across both paths.

### Watcher

Watcher must:

- reject noisy directory events before creating a `subtree_hint`
- reject ignored files before enqueueing ingress
- honor `.axonignore` hierarchy for both files and directories

Watcher must not:

- recursively rescan a technical subtree just because a directory event happened

### Scanner

Scanner must:

- honor the same `.axonignore` hierarchy
- honor the same hard technical protections
- remain the canonical cold discovery path

Scanner may still walk a large workspace, but only after early path filtering.

## Worktrees

Worktrees are not globally forbidden by concept.

Rule:

- a worktree can be useful because it may contain the latest active branch code needed by an LLM
- therefore `.worktrees/` is not auto-banned as a category in the concept
- but worktree noise inside a useful project still remains subject to hard technical protections and local `.axonignore`

In other words:

- useful universe is defined by readable immediate roots under `/home/dstadel/projects`
- noise suppression happens inside that universe
- no silent architectural drift from "multi-project useful scope" to "axon only"

## Current Gaps Against This Concept

1. `/home/dstadel/projects/.axonignore` exists but was too broad in some places:
   - it banned all `.worktrees`
   - it excluded `claude-context-local`
   - it excluded `assets/` and `priv/static/` too aggressively
2. Engine-level `.axonignore` was narrower than the runtime needs.
3. Watcher-side directory filtering was historically too late.
4. The filtering concept was documented in fragments, not frozen in one canonical document.

## Immediate Consequences

The next configuration and code changes should follow this order:

1. tighten `/home/dstadel/projects/.axonignore`
2. tighten `/home/dstadel/projects/axon/.axonignore`
3. keep project-local `.axonignore` for true local needs
4. keep `.axonignore.local` for operator-only exceptions
5. enforce the same policy early in watcher and scanner

## Acceptance Criteria

The concept is correctly implemented when all of the following are true:

- Axon still sees the full declared useful universe under `/home/dstadel/projects`
- technical noise no longer floods watcher ingress
- `.axonignore` hierarchy behaves predictably across ancestor levels
- project-local exceptions can re-include intentionally
- the same file is either accepted or rejected consistently on both cold scan and hot delta paths
- future contributors can recover the policy from this document alone
