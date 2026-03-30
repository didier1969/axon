---
title: Commercial Stabilization Roadmap
date: 2026-03-30
status: active
branch: feat/axon-stabilization-continuation
---

# Objective

Turn Axon first into a tool that Didier can use confidently on real projects under real conditions, then into a commercially viable product by prioritizing stability, trustworthiness, and operational clarity before aggressive optimization.

# Phase 0: Validation En Conditions Reelles

Goal: make Axon genuinely usable by Didier in day-to-day development under real conditions before any commercialization effort.

## D0-1. Real Project Usability

Objective:

- use Axon on real projects in normal development conditions
- validate that startup, indexing, querying, and iteration are practical
- identify blockers to repeated daily use

Acceptance criteria:

- Axon is used repeatedly on at least one real project
- the main workflows are usable without workaround-heavy operation
- the highest-friction blockers are identified and prioritized

## D0-2. Daily Trust Threshold

Objective:

- ensure Axon is trustworthy enough to inform development decisions
- make failures visible rather than silent
- protect conceptual state during daily use

Acceptance criteria:

- no silent corruption of `SOLL`
- no critical workflow depends on guesswork about system state
- failure modes are understandable and recoverable

## D0-3. Developer Ergonomics

Objective:

- reduce friction in install, startup, validation, and routine usage
- make Axon feel operational rather than experimental

Acceptance criteria:

- canonical startup path documented and usable
- validation path is short and repeatable
- diagnostics are understandable during normal use

## Exit Criterion For Phase 0

Didier voluntarily uses Axon on real development work because it is helpful, stable enough, and more valuable than bypassing it.

# Phase 1: Foundation

Goal: make Axon reliable after the first successful real-conditions validation threshold and before selling it.

## P0-1. IST/SOLL Integrity

Objective:

- define and enforce invariants between `IST` and `SOLL`
- guarantee that `IST` is reconstructible
- guarantee that `SOLL` is never destroyed by ingestion, migration, or agent action
- preserve traceability between requirement, decision, code, and validation

Acceptance criteria:

- invariants documented
- non-regression tests covering `IST` / `SOLL` isolation
- no destructive path from ingestion to `SOLL`
- recovery procedure documented for `SOLL`

## P0-2. SOLL Export / Restore

Objective:

- define an official export format for `SOLL`
- define a restoration procedure
- validate that timestamped exports can reconstruct a usable conceptual state
- formalize whether exports are versioned artifacts or external archives

Acceptance criteria:

- export format documented
- restore path implemented or documented with verification
- at least one restore validation executed successfully
- retention policy decided

## P0-3. Rust / Elixir / MCP Contracts

Objective:

- inventory all internal messages and events
- normalize payloads, required fields, and errors
- version internal protocol expectations
- remove or isolate stale historical paths

Acceptance criteria:

- protocol surface documented
- payload contracts clarified
- integration tests cover main event flows
- stale paths identified and either documented or isolated

## P0-4. Crash Recovery

Objective:

- guarantee safe behavior across crash during scan
- guarantee safe behavior across crash during parse
- guarantee safe behavior across crash during write
- restart without silent duplication or data loss

Acceptance criteria:

- recovery scenarios documented
- replay / retry semantics validated
- no silent corruption on restart
- failure modes visible through logs or status

## P0-5. MCP Audit Truthfulness

Objective:

- identify tools that are still heuristic
- distinguish certified, inferred, and estimated outputs
- remove misleading authority from outputs
- add tests proving output consistency

Acceptance criteria:

- classification of tool output truth level documented
- misleading wording removed
- consistency tests added for key audit tools
- UI or documentation reflects limits honestly

# Phase 2: Trust

Goal: make Axon diagnosable and defensible for serious use.

## P1-1. Operational Observability

Objective:

- correlate traces by file, batch, and request
- expose ingestion, latency, error, and saturation metrics
- produce support-grade logs
- give operators a clear incident view

Acceptance criteria:

- trace propagation points documented
- key metrics visible
- logs structured enough for debugging
- dashboard exposes operational state credibly

## P1-2. Environment Reproducibility

Objective:

- maintain one canonical development and runtime path
- validate shells and dependencies
- simplify start and validation scripts
- minimize ambiguity around optional integrations

Acceptance criteria:

- one official path documented
- validation script remains green in canonical shell
- scripts reflect current architecture
- obsolete coupling is either removed or explicitly optional

## P1-3. Dashboard Stabilization

Objective:

- make dashboard states explicit and reliable
- ensure user actions are safe
- remove misleading displays
- separate live state, persisted state, and conceptual state clearly

Acceptance criteria:

- dashboard tests green
- state labels aligned with backend truth
- no critical interaction depends on stale assumptions
- operator messages are understandable

## P1-4. Security Baseline

Objective:

- define MCP perimeter
- handle secrets safely
- restrict risky workspace actions
- audit sensitive operations

Acceptance criteria:

- basic security assumptions documented
- secrets handling reviewed
- risky paths identified
- minimum audit trail available

# Phase 3: Productization

Goal: make Axon installable, operable, and marketable.

## P2-1. Packaging

Objective:

- define installation path
- support local and team modes
- clarify versioning
- document compatibility and limits

Acceptance criteria:

- install path documented
- supported modes listed
- compatibility expectations written
- upgrade path identified

## P2-2. Useful Performance

Objective:

- benchmark small, medium, and large repositories
- define CPU and RAM budgets
- measure key MCP latencies
- define degradation strategy under load

Acceptance criteria:

- benchmark scenarios defined
- budgets documented
- key latency metrics captured
- degradation behavior known

## P2-3. Product Positioning

Objective:

- define what Axon guarantees
- define what Axon does not yet guarantee
- convert vision into an operational promise

Acceptance criteria:

- guarantee surface documented
- non-goals documented
- commercial positioning grounded in current reality

# Recommended Execution Order

1. `D0-1` real project usability
2. `D0-2` daily trust threshold
3. `D0-3` developer ergonomics
4. `P0-1` IST/SOLL integrity
5. `P0-2` SOLL export / restore
6. `P0-3` contracts between Rust, Elixir, and MCP
7. `P0-4` crash recovery
8. `P0-5` MCP audit truthfulness
9. `P1-1` observability
10. `P1-2` reproducibility
11. `P1-3` dashboard stabilization
12. `P1-4` security baseline
13. `P2-*` productization work

# Working Method

This roadmap should be executed with:

- `reality-first-stabilization` for environment-first and truth-first recovery work
- `axon-digital-thread` for conceptual governance and protection of `SOLL`

Additional specialist skills may be used when needed:

- `mission-critical-architect`
- `system-observability-tracer`
- `hardware-aware-scaling`
- `devenv-nix-best-practices`

# Rule

No optimization-first work should be accepted if it weakens stability, trust, or traceability.

Real-conditions validation value comes before commercialization value.
