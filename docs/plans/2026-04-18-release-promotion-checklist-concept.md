# Axon Release Promotion Protocol: Topological Checklist Hardening

## Context

The live promotion completed today exposed a protocol gap, not a product gap.

The promoted code was correct, but the release workflow allowed an inconsistent intermediate state:

- a manifest could be created from a stale binary
- runtime-untracked local state could pollute release truth
- promotion staged `pending.json`
- startup only consumed `current.json`
- shell probes could look healthy while MCP `status` still served the old build

The missing piece is a strict, topological operator checklist with hard gates.

The release flow must behave like an aviation checklist:

- ordered
- blocking
- explicit
- no skipped commit points

## Product Decision

### 1. Release truth is a chain, not a collection of files

A live promotion is valid only if all of these agree:

- Git commit
- build id / git describe
- artifact checksum
- manifest runtime version
- live MCP `status`

If any one diverges, the promotion is not complete.

### 2. MCP `status` is the final runtime authority

For production verification:

- MCP `status` is authoritative
- local shell probes are secondary operator probes only

No release becomes `promoted` until MCP `status` confirms:

- `instance_kind=live`
- expected `build_id`
- expected `install_generation`
- expected public surface / policy

### 3. Promotion must have one explicit commit point

There must be exactly one point where staged release state becomes promoted release state.

Before that point:

- the release is only `staged`
- rollback remains trivial

After that point:

- `current.json` becomes `promoted`
- history is written
- stale pending state is removed

### 4. Staging and activation must use the same source of truth

If startup consumes `current.json`, then restartable staged promotion must update `current.json` before restart or use a startup mode that explicitly consumes `pending.json`.

The system must never stage one manifest and start from another.

### 5. Build and manifest must be coupled

Manifest creation is only valid for the exact artifact built from the candidate commit.

Therefore:

- build preflight is mandatory
- stale binary reuse must be blocked
- runtime-untracked local state must not mark a release dirty
- tracked code dirtiness must still block release qualification

## Required Checklist Layers

### Layer A: Release Preflight

Block unless all pass:

- tracked Git worktree clean
- build artifact exists
- build info exists
- build id matches candidate commit
- artifact checksum calculable
- qualification evidence targets the same build
- qualification evidence is fresh for the candidate promotion wave
- no stale `pending.json` or failed staging state remains unless the operator explicitly clears it

### Layer B: Staging

Allowed only after preflight passes:

- write a single staged manifest
- record intended install generation
- ensure startup will read that exact staged release

### Layer C: Hard Stop

Before restart:

- no live `axon-core` process
- no live `tmux` session
- no MCP listener on live port

This is a real gate, not best-effort.

### Layer D: Restart and Runtime Proof

After restart:

- MCP endpoint reachable
- the checked endpoint is the exact `live` target instance being promoted
- `status` returns expected `build_id`
- `status` returns expected `install_generation`
- `status` returns expected `release_version`
- expected public surface and policy are visible

Proof timing rule:

- runtime proof has a bounded timeout
- timeout means promotion has failed to verify and must not commit

### Layer E: Promotion Commit Point

Only after runtime proof succeeds:

- mark active manifest `promoted`
- write history entry
- remove stale pending state

If runtime proof fails:

- do not mutate promoted truth
- keep the release explicitly non-promoted
- require operator recovery or rollback before another promotion attempt

## Constraints

- Minimal change first; do not redesign release management from scratch.
- Preserve current live/dev model.
- Preserve the existing release manifest format unless a new field is necessary for correctness.
- Prefer server/runtime truth over shell inference.

## Non-goals

- no package registry redesign
- no CI/CD platform redesign
- no full deployment orchestration rewrite

## Expected Outcome

After this hardening:

- a stale binary cannot be accidentally promoted under a newer commit
- staging and startup cannot drift apart
- shell-local health cannot override MCP runtime truth
- promotion finishes only when the live runtime proves the expected build is actually serving
