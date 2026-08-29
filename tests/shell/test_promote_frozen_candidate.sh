#!/usr/bin/env bash
# REQ-AXO-902529 — every release gate certifies one frozen candidate bundle.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
PROMOTE="$ROOT_DIR/scripts/release/promote_live_safe.sh"
START="$ROOT_DIR/scripts/start.sh"

PASS=0
FAIL=0
pass() { printf '  PASS  %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL  %s\n' "$1" >&2; FAIL=$((FAIL + 1)); }

if grep -q -- '--candidate-bin-dir' "$START" && \
   grep -q 'CANDIDATE_BIN_DIR/axon-brain' "$START" && \
   grep -q 'CANDIDATE_BIN_DIR/axon-indexer' "$START"; then
  pass "dev start accepts and resolves an explicit immutable candidate bundle"
else
  fail "dev start cannot select an explicit candidate bundle"
fi

if grep -q 'AXON_INSTANCE_KIND.*-z "$CANDIDATE_BIN_DIR"' "$START"; then
  pass "dev auto-rebuild is structurally disabled for an explicit candidate bundle"
else
  fail "candidate start can still enter the mutable source auto-rebuild branch"
fi

if grep -q -- '--candidate-bin-dir "$PROMOTE_FROZEN_WORKTREE/bin"' "$PROMOTE"; then
  pass "promote starts DEV from the exact frozen worktree bundle"
else
  fail "promote does not pass the frozen candidate bundle to DEV"
fi

if grep -q 'PROMOTE_FROZEN_WORKTREE=' "$PROMOTE" && \
   grep -q '_cleanup_frozen_worktree' "$PROMOTE" && \
   grep -q 'source_root="$PROMOTE_FROZEN_WORKTREE"' "$PROMOTE" && \
   grep -q 'run_targeted_cargo_build.sh' "$PROMOTE" && \
   grep -q 'test-lib "$source_root/src/axon-core"' "$PROMOTE"; then
  pass "frozen worktree survives through test-target compilation and has EXIT cleanup"
else
  fail "test targets are not compiled from the retained frozen worktree"
fi

if grep -q 'PROMOTE_MANIFEST_BG' "$PROMOTE"; then
  fail "unsafe manifest/dev concurrency toggle still exists"
else
  pass "unsafe PROMOTE_MANIFEST_BG path is absent"
fi

missing_line="$(grep -n 'could not extract .*runtime_version.build_id' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
if [[ -n "$missing_line" ]] && \
   sed -n "${missing_line},$((missing_line + 3))p" "$PROMOTE" | grep -q 'return 1'; then
  pass "missing DEV build identity fails closed"
else
  fail "missing DEV build identity still passes as a warning"
fi

recheck_line="$(grep -n 'run_step 3 candidate_recheck' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
manifest_line="$(grep -n '^# --- Step 4' "$PROMOTE" | tail -1 | cut -d: -f1 || true)"
if [[ -n "$recheck_line" && -n "$manifest_line" && "$recheck_line" -lt "$manifest_line" ]]; then
  pass "candidate digest/build identity is rechecked immediately before manifest creation"
else
  fail "no post-DEV candidate recheck precedes manifest creation"
fi

parent_path_line="$(grep -n 'PROMOTE_FROZEN_WORKTREE=.*promote-worktrees/stable' "$PROMOTE" | tail -1 | cut -d: -f1 || true)"
build_step_line="$(grep -n 'run_step 1 build build_from_frozen_worktree' "$PROMOTE" | head -1 | cut -d: -f1 || true)"
if [[ -n "$parent_path_line" && -n "$build_step_line" && "$parent_path_line" -lt "$build_step_line" ]]; then
  pass "candidate path is published in the parent before run_step's pipeline subshell"
else
  fail "candidate path is only assigned inside the streamed build subshell"
fi

if grep -q 'install -m 755.*ROOT_DIR/bin' "$PROMOTE"; then
  fail "build still replaces LKG bin/* before activation"
else
  pass "candidate build cannot overwrite LKG bin/* before activation"
fi

if grep -q 'reuse_checkpoint=1' "$PROMOTE" && \
   grep -q 'advance_checkpoint=1' "$PROMOTE" && \
   grep -q 'switch --detach "$sha"' "$PROMOTE" && \
   grep -q 'rev-parse HEAD' "$PROMOTE" && \
   grep -q 'diff --quiet HEAD' "$PROMOTE" && \
   grep -q 'status --porcelain --untracked-files=normal' "$PROMOTE"; then
  pass "a clean stable worktree is reused or advanced without rewriting unchanged sources"
else
  fail "frozen candidate checkpoints cannot preserve Cargo source identity across SHAs"
fi

if grep -q 'test-lib "$source_root/src/axon-core"' "$PROMOTE" && \
   grep -q 'cargo test --lib --no-run -j 1' "$ROOT_DIR/scripts/release/run_targeted_cargo_build.sh"; then
  pass "test-target compilation has an explicit one-job memory backpressure guard"
else
  fail "test-target compilation can still fan out under memory pressure"
fi

if grep -q '_cleanup_frozen_worktree "$rc"' "$PROMOTE" && \
   grep -q 'checkpoint figé conservé pour retry' "$PROMOTE"; then
  pass "a controlled pre-cutover failure preserves the immutable checkpoint for retry"
else
  fail "controlled gate failures still destroy the expensive immutable checkpoint"
fi

if grep -q 'ROOT_DIR/.axon/promote-worktrees' "$PROMOTE" && \
   ! grep -q 'TMPDIR:-/tmp.*axon-promote' "$PROMOTE"; then
  pass "candidate targets are disk-backed and cannot exhaust the /tmp tmpfs"
else
  fail "promotion still stores its multi-gigabyte Cargo target in tmpfs"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
