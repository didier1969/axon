---
phase: 03-workflow-integration
plan: 01
subsystem: cli
tags: [shell-hook, direnv, bash, zsh, pid-file, auto-start]

requires:
  - phase: 02-large-project-performance
    provides: stable watcher process to auto-start

provides:
  - axon shell-hook command (bash/zsh eval-safe output)
  - axon init command (instructions + direnv .envrc creation)

affects: [03-02-ci-integration, 03-04-developer-documentation]

tech-stack:
  added: []
  patterns:
    - PID-file process deduplication (.axon/watch.pid)
    - eval-safe stdout (print() not console.print()) for shell integration code
    - sentinel-guarded .envrc block for idempotent file writes

key-files:
  created: []
  modified:
    - src/axon/cli/main.py
    - tests/cli/test_main.py

key-decisions:
  - "Use print() not console.print() for shell-hook — stdout must be eval-safe (no Rich markup)"
  - "Sentinel '# >>> axon auto-start <<<' guards .envrc block against duplication"
  - "PID-file (.axon/watch.pid) prevents duplicate watcher processes on re-entry"

patterns-established:
  - "Shell integration via eval: eval \"$(axon shell-hook)\" — standard pattern for CLI shell hooks"
  - "Idempotent file block writes: check sentinel before append"

duration: ~30min
started: 2026-02-26T00:00:00Z
completed: 2026-02-26T00:00:00Z
---

# Phase 3 Plan 01: Shell Integration Summary

**`axon shell-hook` and `axon init` deliver zero-friction watcher auto-start via bash/zsh hooks and direnv integration.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~30 min |
| Tasks | 2 completed |
| Files modified | 2 |
| Tests added | 16 |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: shell-hook bash output | Pass | `_axon_chpwd`, `PROMPT_COMMAND`, `axon watch`, `watch.pid` all present |
| AC-2: shell-hook zsh output | Pass | `_axon_chpwd`, `add-zsh-hook`, `axon watch`, `watch.pid` all present |
| AC-3: shell-hook default (no --shell) | Pass | bash-compatible output, exit 0 |
| AC-4: init creates .envrc snippet | Pass | Creates `.envrc` with PID-file guard and axon watch |
| AC-5: init --direnv appends to existing .envrc | Pass | Original content preserved, block appended |
| AC-6: init without --direnv prints instructions | Pass | Shows both shell-hook and direnv approaches |

## Accomplishments

- `axon shell-hook [--shell bash|zsh]` — prints eval-safe shell function; bash uses `PROMPT_COMMAND`, zsh uses `add-zsh-hook chpwd`
- `axon init [--direnv]` — without flag: prints instructions; with flag: creates/appends idempotent block to `.envrc` using sentinel `# >>> axon auto-start <<<`
- 16 new tests across `TestShellHook` (11) and `TestInit` (5) — all passing
- 668/668 tests passing (652 prior + 16 new), no regressions

## Task Commits

| Task | Commit | Type | Description |
|------|--------|------|-------------|
| Task 1: shell-hook + init commands | (pending) | feat | Add axon shell-hook and init CLI commands |
| Task 2: TestShellHook + TestInit | (pending) | test | Add 16 tests for shell-hook and init commands |

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `src/axon/cli/main.py` | Modified | Added `shell_hook()` command (~50 lines) and `init()` command (~55 lines) |
| `tests/cli/test_main.py` | Modified | Added `TestShellHook` (11 tests) and `TestInit` (5 tests) |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| `print()` not `console.print()` for shell-hook | stdout must be eval-safe; Rich markup would break eval | Shell integration code is clean plain text |
| Sentinel `# >>> axon auto-start <<<` | Prevents duplication on repeated `axon init --direnv` calls | Idempotent by design |
| `kill -0` for PID-file liveness check | Standard POSIX approach; no extra dependencies | Same pattern usable in direnv block and shell function |

## Deviations from Plan

None — plan executed exactly as specified.

## Issues Encountered

| Issue | Resolution |
|-------|-----------|
| Pre-existing ruff lint errors in test file (F821 `pytest` undefined name, E501 long lines) | Pre-existing codebase issue; new tests follow same pattern as existing tests. No new error categories introduced. |

## Next Phase Readiness

**Ready:**
- `axon shell-hook` output is eval-safe and ready for documentation in 03-04
- `.envrc` integration pattern documented for CI guide (03-02)
- PID-file dedup pattern established — consistent across shell and direnv paths

**Concerns:**
- Pre-existing ruff lint errors in `tests/cli/test_main.py` (F821, F841, E501) — not blocking but worth a cleanup pass

**Blockers:**
- None

---
*Phase: 03-workflow-integration, Plan: 01*
*Completed: 2026-02-26*
