# Release Artifact Integrity Concept

## Problem

The release protocol correctly enforced:

- tracked-clean Git state
- `build_id == git describe`
- manifest checksum integrity

But it did not enforce that the mutable workspace artifact `bin/axon-core` was the
same binary that had just been built from the current `HEAD`.

This allowed a false identity state:

- new `build-info`
- old binary

The immediate trigger was non-deterministic release binary selection in `scripts/setup.sh`
via `find ... | head -n 1`.

## Required Invariants

1. The canonical workspace release artifact is `$(CARGO_TARGET_DIR or .axon/cargo-target)/release/axon-core`.
2. `scripts/setup.sh` must copy only that artifact into `bin/axon-core`.
3. `bin/axon-core.build-info` must record:
   - `AXON_BUILD_ID`
   - `AXON_PACKAGE_VERSION`
   - `AXON_RELEASE_VERSION`
   - `AXON_INSTALL_GENERATION`
   - `AXON_ARTIFACT_SHA256`
   - `AXON_ARTIFACT_SOURCE`
4. `release-preflight` must fail if:
   - tracked Git state is dirty
   - `build_id != git describe`
   - recorded artifact sha does not match the real artifact
   - workspace `bin/axon-core` drifts from the canonical workspace release target

## Minimal Change Strategy

- do not redesign manifests
- do not redesign promotion flow
- fix deterministic artifact selection at source
- harden preflight so metadata/body divergence becomes impossible to miss

## Non-Goals

- no new package manager
- no release image pipeline redesign
- no change to live manifest semantics
