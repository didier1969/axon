# Live Release Operator Notes

## Canonical Commands

Use:

```bash
./scripts/axon release-preflight
./scripts/axon create-release-manifest --state qualified
./scripts/axon promote-live --manifest <manifest.json>
./scripts/axon rollback-live --manifest <manifest.json>
```

These commands define the operator-facing release cycle for the live instance.

`release-preflight` is now the mandatory first gate before manifest creation or live promotion.

It must prove both:

- the release metadata is coherent
- the workspace artifact body is the exact canonical build output

## Release States

- `pushed`
  Code exists on Git, but it is not qualified for production.
- `qualified`
  A manifest exists for an immutable artifact and the required gates passed.
- `staged`
  A pending manifest exists for a selected release, but it is not yet `promoted`.
- `promoted`
  The live instance restarted successfully and MCP `status` matched the manifest runtime identity.

Do not treat `git push` as a production release.

## Manifest Contract

Each manifest binds:

- Git source identity
- `release_version`
- `package_version`
- `build_id`
- immutable artifact path
- artifact checksum
- qualification evidence

Promotion and rollback operate on the archived artifact from the manifest, not on the mutable worktree binary.

For workspace qualification, `bin/axon-core` must itself be a deterministic copy of the canonical workspace release target:

- `.axon/cargo-target/release/axon-core`

`build-info` is no longer sufficient by itself. The recorded artifact checksum and the workspace target checksum must agree too.

For the `live` instance, ordinary `./scripts/axon --instance live start ...` now rehydrates `bin/axon-core` from `.axon/live-release/current.json` when a promoted manifest exists. A normal live restart must therefore preserve the promoted artifact lineage instead of silently replacing it with the workspace build.

## Promotion

Topological checklist:

1. `./scripts/axon release-preflight`
2. `./scripts/axon create-release-manifest --state qualified`
3. `./scripts/axon promote-live --manifest <manifest>.json --restart-live`
4. confirm MCP `status` matches the manifest
5. only then treat the release as `promoted`

Additional invariant before step 2:

- `bin/axon-core` must match `.axon/cargo-target/release/axon-core`
- `bin/axon-core.build-info` must match the real checksum of `bin/axon-core`

Stage only:

```bash
./scripts/axon promote-live --manifest .axon/releases/candidates/<manifest>.json
```

Without `--restart-live`, promotion is now bookkeeping only:

- `pending.json` is written
- `bin/axon-core` is not replaced yet
- live keeps serving the current promoted version

Promote with restart and strict post-check:

```bash
./scripts/axon promote-live --manifest .axon/releases/candidates/<manifest>.json --restart-live
```

With restart, the script now verifies live via MCP `status` and compares:

- `instance_identity.instance_kind == live`
- `runtime_version.release_version`
- `runtime_version.package_version`
- `runtime_version.build_id`
- `runtime_version.install_generation`

Only then does the manifest become `promoted`.

The restart now boots from the exact staged manifest under verification, not from a different active manifest.

## Rollback

Rollback uses the same artifact-driven and MCP-verified flow:

```bash
./scripts/axon rollback-live --manifest .axon/live-release/history/<generation>.json --restart-live
```

`rollback-live` accepts only manifests that were already `promoted` on live. A candidate or merely qualified manifest is not a rollback target.

Rollback is not complete until the same strict MCP post-check passes.

## When `--skip-postcheck` Is Acceptable

Use `--skip-postcheck` only when:

- live restart is being handled manually in a tightly controlled window, or
- the operator is intentionally leaving the release in a non-promoted bookkeeping state before rerunning promotion with an explicit restart window.

In that case the release remains `staged`, not `promoted`.
`staged` must not be treated as a durable promoted live cutover state by itself. If `--restart-live --skip-postcheck` was used, live may already be serving the staged bits, but that state is unverified and the bookkeeping remains non-promoted until a later successful MCP post-check.

## Failed Post-Check

If a restart happened and the MCP runtime-version post-check fails:

- live may already be running the staged artifact
- `pending.json` remains in place
- `current.json` remains unchanged
- the script exits non-zero with an explicit recovery message

If restart itself fails, the scripts now emit the same kind of explicit recovery message and leave:

- `pending.json` in place
- `current.json` unchanged
- recovery to the operator through a rerun or explicit rollback/promotion

Recovery is then operator-driven:

- inspect live `status`
- either rerun the same promote/rollback with restart and post-check
- or perform an explicit rollback/promotion to a known-good manifest

## When `--dry-run` Is Acceptable

Use `--dry-run` to validate:

- manifest readability
- artifact presence
- checksum consistency
- operator intent

`--dry-run` must remain side-effect free.

## Safety Rules

- Do not skip `release-preflight`.
- Do not promote from an unqualified manifest.
- Do not bypass the archived artifact with a worktree binary.
- Do not call a version `live` unless MCP `status` proves it.
- Do not assume rollback is safe unless schema/data compatibility was part of qualification.
