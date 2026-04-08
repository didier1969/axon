# Ecosystem Directory Policy Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce a centralized directory exclusion policy derived from Axon's supported parser ecosystems and apply it consistently to scanner and subtree-hint admission.

**Architecture:** Add a dedicated `indexing_policy` module that classifies paths as allowed, hard-excluded, or soft-excluded using ecosystem-aware declarative rules. Keep `config.rs` as the operator override layer, have `parser/mod.rs` expose the supported ecosystem set, and route scanner decisions through the new policy so watcher/scanner behavior converges on the same semantics.

**Tech Stack:** Rust, Axon core scanner/config/parser modules, `cargo test`

**Status:** Complete le `2026-04-08`

**Resultat implemente:**
- `indexing_policy.rs` centralise maintenant la classification `Allow | HardExcluded | SoftExcluded`
- `parser/mod.rs` expose l’inventaire des ecosystems supportes utilise par la politique
- `config.rs` porte les overrides operateur pour les segments `SoftExcluded`
- `scanner.rs` route les decisions de descente et de `subtree_hint` vers la politique centralisee
- le worktree dispose aussi d’une boucle courte Rust versionnee via `.cargo/config.toml`, `scripts/cargo-env.sh` et `scripts/dev-fast.sh`

**Validation executee:**
- `cargo test indexing_policy -- --nocapture`
- `cargo test scanner::tests -- --nocapture`
- `bash -n scripts/cargo-env.sh scripts/dev-fast.sh scripts/setup.sh scripts/start.sh`
- `bash scripts/dev-fast.sh help`
- `cargo test --manifest-path src/axon-core/Cargo.toml -- --nocapture`

**Verdict:**
- la politique d’exclusion ecosysteme est couverte par des tests unitaires et d’integration scanner
- la boucle rapide shell est syntaxiquement valide et executable
- la suite complete `axon-core` passe avec cette tranche incluse

---

### Task 1: Add failing tests for centralized path classification

**Files:**
- Create: `src/axon-core/src/indexing_policy.rs`
- Modify: `src/axon-core/src/lib.rs`

**Step 1: Write the failing test**

Add unit tests that require:
- `_build_truth_dashboard_ui` to be `HardExcluded`
- `node_modules/react/index.js` to be `HardExcluded`
- `vendor/acme/lib.rb` to be `SoftExcluded`
- `src/vendor_adapter.rs` to remain `Allow`
- `supported_parser_ecosystems()` to include at least `JavaScript`, `TypeScript`, `Python`, `Elixir`, `Rust`, `Go`, `Jvm`

**Step 2: Run test to verify it fails**

Run: `cargo test indexing_policy -- --nocapture`
Expected: FAIL because the module and API do not exist yet.

**Step 3: Write minimal implementation**

Create the new policy module with:
- ecosystem enum
- artifact class enum
- exclusion policy enum
- path disposition enum
- parser ecosystem inventory helper
- path classifier with declarative directory rules

**Step 4: Run test to verify it passes**

Run: `cargo test indexing_policy -- --nocapture`
Expected: PASS

### Task 2: Add failing tests for config-derived soft-exclude overrides

**Files:**
- Modify: `src/axon-core/src/config.rs`
- Modify: `src/axon-core/src/indexing_policy.rs`

**Step 1: Write the failing test**

Add tests that require:
- default policy to soft-exclude `vendor`, `dist`, `build`, `out`
- config override helper to re-allow selected soft-excluded segments
- exact hard exclusions to remain non-overridable by soft-allow rules

**Step 2: Run test to verify it fails**

Run: `cargo test indexing_policy_override -- --nocapture`
Expected: FAIL because config does not expose override lists yet.

**Step 3: Write minimal implementation**

Extend `IndexingConfig` with additive allowlists for soft-excluded directories and wire the policy classifier to honor them.

**Step 4: Run test to verify it passes**

Run: `cargo test indexing_policy_override -- --nocapture`
Expected: PASS

### Task 3: Add failing scanner integration tests

**Files:**
- Modify: `src/axon-core/src/scanner.rs`
- Test: `src/axon-core/src/scanner.rs`

**Step 1: Write the failing test**

Add scanner tests that require:
- `node_modules`, `_build_truth_*`, `.next`, `.gradle`, `__pycache__` to be pruned by directory descent
- `vendor` to be blocked by default for subtree hints
- a configured soft-allow segment to remain scannable

**Step 2: Run test to verify it fails**

Run: `cargo test scanner::tests -- --nocapture`
Expected: FAIL because scanner still uses flat lists and ad-hoc generated-artifact matching.

**Step 3: Write minimal implementation**

Route scanner path decisions through the centralized policy and keep existing ignore controls (`.axonignore`, `.axoninclude`, git rules) intact.

**Step 4: Run test to verify it passes**

Run: `cargo test scanner::tests -- --nocapture`
Expected: PASS

### Task 4: Clean integration points and verify targeted behavior

**Files:**
- Modify: `src/axon-core/src/parser/mod.rs`
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/config.rs`

**Step 1: Refactor**

Remove duplicated directory classification logic from scanner and expose parser ecosystem inventory from one obvious place.

**Step 2: Run focused verification**

Run:
- `cargo test indexing_policy -- --nocapture`
- `cargo test indexing_policy_override -- --nocapture`
- `cargo test generated_artifact_prefixes_are_treated_as_build_noise -- --nocapture`
- `cargo test blocked_subtree_hint_segments_reject_build_like_directory_events -- --nocapture`

Expected: PASS

### Task 5: Final hygiene

**Files:**
- Modify: `docs/plans/2026-04-08-ecosystem-directory-policy-plan.md`

**Step 1: Run diff hygiene**

Run: `git diff --check -- src/axon-core/src/indexing_policy.rs src/axon-core/src/config.rs src/axon-core/src/parser/mod.rs src/axon-core/src/scanner.rs docs/plans/2026-04-08-ecosystem-directory-policy-plan.md`
Expected: no whitespace or conflict issues

**Step 2: Document residual risks**

Note any ecosystems not yet covered by exact rules and any follow-up integration still needed on watcher telemetry or operator docs.
