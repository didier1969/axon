# Release Artifact Integrity Implementation Plan

1. Make workspace release artifact selection deterministic.
   - Update `scripts/setup.sh` to copy only `.axon/cargo-target/release/axon-core`
   - Fail fast if that artifact is missing after build

2. Record artifact identity next to build identity.
   - Update `bin/axon-core.build-info` generation to include:
     - `AXON_ARTIFACT_SHA256`
     - `AXON_ARTIFACT_SOURCE`

3. Harden `release-preflight`.
   - Verify recorded sha matches the real artifact sha
   - When preflighting `bin/axon-core` in normal build-match mode, verify it matches the canonical workspace release target

4. Update operator doctrine.
   - Add the invariant to release operator notes
   - Add the invariant to the Axon skill release checklist

5. Verify.
   - `bash -n` on touched shell scripts
   - run `scripts/release/preflight.sh` in normal state
   - create a synthetic mismatch between `bin/axon-core` and the canonical target and verify preflight fails
   - restore clean state and verify preflight passes

6. Final CDD review.
   - runtime/release reviewer
   - operator/protocol reviewer
