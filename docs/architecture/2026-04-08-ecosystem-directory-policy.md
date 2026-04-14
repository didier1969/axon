# Ecosystem Directory Policy

## Purpose

Axon no longer decides noisy-directory exclusion from a flat list of segment names spread across the scanner. The runtime now uses a centralized policy that derives exclusion rules from the parser ecosystems Axon actually supports.

The target is operational, not cosmetic:
- stop watcher and scanner churn on dependency stores, build trees, caches, and framework outputs
- keep one shared semantic decision for scan descent and `subtree_hint` admission
- preserve operator control for ambiguous directories without regressing to hardcoded special cases

## Source Of Truth

The policy now lives in [indexing_policy.rs](/home/dstadel/projects/axon/src/axon-core/src/indexing_policy.rs).

Its ecosystem inventory is exposed by [parser/mod.rs](/home/dstadel/projects/axon/src/axon-core/src/parser/mod.rs) through `supported_parser_ecosystems()`.

This means Axon derives active directory rules from supported parser families such as:
- JavaScript / TypeScript
- Python
- Elixir / Erlang
- Rust
- Go
- JVM
- C / C++
- C#
- Ruby
- PHP
- Web assets
- Data logic

## Classification Model

Each path is classified as one of:
- `Allow`
- `HardExcluded`
- `SoftExcluded`

Each exclusion carries:
- an `ecosystem`
- an `artifact class`
- a stable `rule_id`

The current artifact classes are:
- `DependencyStore`
- `BuildOutput`
- `Cache`
- `ToolingState`
- `GeneratedFrameworkOutput`
- `RuntimeArtifact`
- `RepositoryMetadata`

## Policy Semantics

### Hard Excludes

These are directories that should not become indexable by accident:
- repository metadata: `.git`, `.svn`, `.hg`
- generic tooling state: `.direnv`, `.devenv`, `.cache`
- JavaScript / TypeScript: `node_modules`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`, `.parcel-cache`
- Python: `__pycache__`, `.venv`, `venv`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.tox`, `.nox`, `.eggs`, `site-packages`, `dist-packages`
- Elixir / Erlang: `_build*`, `deps`, `.elixir_ls`, `.mix`, `ebin`, `.rebar3`
- Rust: `target`
- JVM: `.gradle`

### Soft Excludes

These are excluded by default, but may be intentionally reopened:
- `vendor`
- `dist`
- `build`
- `out`

This is the key doctrinal change. These segments are no longer turned into hard excludes by config defaults.

## Runtime Integration

The policy is applied in [scanner.rs](/home/dstadel/projects/axon/src/axon-core/src/scanner.rs) in two places:
- `classify_path()` for normal file and directory eligibility
- `classify_subtree_hint_path()` for directory-event admission into `subtree_hints`

Because [fs_watcher.rs](/home/dstadel/projects/axon/src/axon-core/src/fs_watcher.rs) already routes decisions through `Scanner`, the watcher now inherits the same policy without a second copy of the rules.

The effective decision order is:
1. workspace hard rule for top-level `.worktrees`
2. centralized ecosystem policy
3. hierarchical `.axonignore`
4. `.axoninclude`
5. git ignore / exclude
6. extension support checks

So the new policy filters structural noise first, while local ignore/include files still refine project-specific behavior afterward.

## Configuration Surface

The operator-facing config remains in `.axon/capabilities.toml`, loaded by [config.rs](/home/dstadel/projects/axon/src/axon-core/src/config.rs).

The new override is:

```toml
[indexing]
soft_excluded_directory_segments_allowlist = ["vendor", "dist"]
```

Meaning:
- a `SoftExcluded` segment listed here becomes `Allow`
- a `HardExcluded` rule remains excluded

The old config arrays still exist, but their role is now additive:
- `ignored_directory_segments`
- `blocked_subtree_hint_segments`

They are reserved for explicit operator hard-deny additions such as environment-specific runtime directories. They are no longer the primary carrier for ecosystem semantics.

## Why This Matters For OOM Control

The original failure mode came from watcher and subtree-hint churn over generated or non-source trees. A flat denylist cannot reliably converge because variants like `_build_truth_*` or ecosystem-specific generated folders keep escaping nominal matching.

The centralized policy improves that by:
- classifying `_build*` by prefix, not by one exact name
- blocking framework outputs and caches before they become hot subtree scans
- ensuring scanner and watcher use the same decision
- preserving a narrow override path for genuinely ambiguous directories

This does not replace memory governance or subtree-hint budgeting. It reduces one of the dominant structural causes of useless work entering the pipeline.

## Residual Limits

Current limits of this policy:
- it is directory-segment based, not repository-manifest aware
- `SoftExcluded` reopening is segment-based, not path-scoped
- the full crate-wide test suite has not yet been used as the release gate for this change

Reasonable follow-ups:
- add path-scoped soft-allow overrides
- emit policy `rule_id` in watcher telemetry for rejected directory events
- add operator docs with ecosystem examples per language family
