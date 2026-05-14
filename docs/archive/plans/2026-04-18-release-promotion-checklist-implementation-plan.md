# Implementation Plan: Release Promotion Checklist Hardening

## Scope

Harden the Axon production promotion workflow so that build, artifact, manifest, startup, and live runtime proof are topologically consistent.

## Dependency Order

1. Add build/release preflight checks
2. Couple manifest creation to exact build identity
3. Fix staging vs startup source-of-truth mismatch
4. Make restart enforce a hard stop before startup
5. Make promotion completion depend on MCP runtime proof
6. Update operator docs
7. Re-run dry-run and live-safe validations

## Tasks

### Task 1: Build Preflight Gate

Add a release preflight script or equivalent checks that verify:

- tracked Git state is clean
- artifact exists
- build-info exists
- build-info build id matches candidate Git describe / commit
- artifact checksum is computable
- stale pending/live staging state is absent or explicitly cleared
- qualification evidence belongs to the candidate build and current promotion wave

The manifest step must fail fast if preflight fails.

### Task 2: Manifest Exactness Gate

Strengthen `create_manifest.py` so that:

- it can reject stale build-info / artifact mismatches
- it records enough source truth to compare artifact and Git identity
- runtime-untracked local state does not mark the candidate dirty

### Task 3: Unify Staging and Activation

Fix the current mismatch where:

- promotion stages `pending.json`
- startup consumes `current.json`

Choose one authoritative flow:

- either startup can explicitly start from `pending.json`
- or promotion writes the staged manifest to the active startup source before restart

Rule:

- the restarted live process must always boot from the exact manifest being validated

### Task 4: Hard Stop Gate

Before live restart, require explicit confirmation that:

- no live core process remains
- no live tmux session remains
- no live MCP port listener remains

If not true, abort before start.

### Task 5: Runtime Proof Gate

Use MCP `status` as the post-start truth source and verify:

- `instance_kind=live`
- the checked endpoint is the exact live target URL
- expected `build_id`
- expected `install_generation`
- expected `release_version`
- expected public surface / policy where relevant

This proof step must happen before writing `promoted`.
It must also have a bounded timeout and fail closed.

### Task 6: Promotion Commit Point

On successful runtime proof:

- write active manifest as `promoted`
- write history entry
- remove pending/stale staging state

If runtime proof fails:

- leave the release staged or failed
- do not mutate final promoted truth
- emit a clear operator recovery message

### Task 7: Operator Notes

Update release notes/operator docs to state:

- preflight is mandatory
- MCP `status` is final truth
- shell probes are advisory
- staged vs promoted semantics

## Validation Matrix

### Code-level

- shell syntax checks for release/start scripts
- Python compile checks for release helpers

### Flow-level

- manifest dry-run with preflight
- promotion dry-run
- controlled live restart using staged/current path
- MCP status proof on the expected build

### Evidence-level

Verify after implementation:

- no stale artifact can produce a valid qualified manifest
- promotion cannot restart from a different manifest than the one being validated
- final promoted state matches the live runtime truth

## Risks

### Risk 1: Over-tightening local operator flow

Mitigation:

- keep the checklist strict only on release-critical points
- preserve expert/manual recovery paths

### Risk 2: Breaking current restart automation

Mitigation:

- change the minimum number of moving parts
- prefer explicit active-manifest semantics over broader runtime redesign

### Risk 3: History/state drift

Mitigation:

- make one final commit point responsible for:
  - current manifest
  - history
  - pending cleanup

## Rollback

If the protocol hardening causes regressions:

- revert the release-script wave as one unit
- preserve the existing manifest format and live runtime data
