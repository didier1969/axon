---
name: idea-to-delivery
description: Use when a non-trivial idea, feature, migration, refactor, or architecture change is still partially specified and needs a rigorous path from concept to plan to implementation, with independent expert challenge and autonomous execution.
---

# Idea To Delivery

## Overview

Use this skill as a lightweight **meta-orchestrator** for complex work that is not ready for direct implementation.

It does not replace other workflow skills.
It selects them, orders them, and enforces phase gates, independent critique, and evidence-based closure.

## When To Use

Use this skill when all of the following are materially true:

- the request is non-trivial, risky, or cross-cutting
- the idea is still partially formed, ambiguous, or under-specified
- the work likely needs:
  - concept shaping
  - then planning
  - then implementation
- independent expert challenge would reduce risk

Typical triggers:

- architecture changes
- environment/runtime redesign
- migrations
- major features
- operator workflow redesign
- refactors with runtime or data consequences

## When Not To Use

Do not use this skill when:

- the task is simple and fully specified
- a valid implementation plan already exists
- the task is already inside an active execution workflow using `subagent-driven-development` or `executing-plans`
- the task is a direct bugfix better handled by `systematic-debugging`

## Routing Rules

This skill must route to existing skills instead of reimplementing them.

- Before `writing-plans` for any `standard` or `full` task:
  - ensure branch/worktree isolation is already in place when the work is not meant to happen directly in the current main workspace
  - if isolation is needed, use `using-git-worktrees` first
- If the request still needs design exploration and user approval:
  - use `brainstorming`
- Once the concept is approved or sufficiently bounded:
  - use `writing-plans` for `standard` and `full`
- For execution:
  - use `subagent-driven-development` when staying in the current session with task-level checkpoints
  - use `executing-plans` when execution should happen in a separate session or larger batches
  - in `executing-plans` mode, the orchestrator must re-enter at each batch boundary to run the required independent reviews before the next batch proceeds
- Before claiming success:
  - use `verification-before-completion`
- When branch/worktree isolation matters:
  - use `using-git-worktrees`
- When execution is complete and integration choice matters:
  - use `finishing-a-development-branch`

## Triage

Classify first:

- `light`
  - small but ambiguous
  - concept convergence only
- `standard`
  - concept + plan + execution
- `full`
  - architecture, migration, live data, operator doctrine, or large blast radius

Triage exit rules:

- `light`
  - stop after concept convergence and explicit summary to the user
  - do not auto-continue into planning or implementation
- `standard`
  - run concept -> plan -> execution
- `full`
  - run concept -> plan -> execution with the strictest review discipline

See [triage-and-gates.md](references/triage-and-gates.md).

## Three Mandatory Phases

### Phase 1: Concept

1. Inspect the real system.
2. If design is still exploratory, route into `brainstorming` and stop until design approval exists.
3. Write the concept or evaluation+concept doc.
4. Get two independent expert reviews.
5. Iterate until convergence.

### Phase 2: Plan

1. Route into `writing-plans`.
2. Before doing so, ensure the proper branch/worktree isolation already exists when the task should not proceed directly in the current workspace.
3. Produce the ordered plan.
4. Make validation, risk, and rollout explicit.
5. Get two independent expert reviews.
6. Iterate until convergence.

### Phase 3: Execute

1. Create or confirm branch/worktree isolation when appropriate.
2. Choose exactly one execution mode:
   - `subagent-driven-development`
   - `executing-plans`
3. Execute through that routed skill.
4. Review defined checkpoints with independent subagents.
5. Correct issues iteratively.
6. Update code, docs, skills, and operational notes as needed.
7. Run validations.
8. Use `verification-before-completion`.

Checkpoint policy:

- with `subagent-driven-development`
  - checkpoint at each plan task
- with `executing-plans`
  - checkpoint at each batch or phase boundary defined by the plan
  - after each batch boundary, the orchestrator must collect the independent reviews before authorizing the next batch

## Review Rules

When asking subagents to review:

- pass only the minimum necessary context
- do not preload the expected answer
- use different review angles when possible
- require explicit verdicts

### Verdict Vocabulary

Use only:

- `approved`
- `approved_with_reservations`
- `needs_reframe`
- `blocked`

If reviewers disagree materially:

1. summarize the disagreement explicitly
2. compare against the evidence hierarchy
3. do one of:
   - revise and re-review
   - escalate to the user if the disagreement is architectural and underdetermined
   - choose the better-supported position and explain why

Do not silently average incompatible reviews.

Default max review loops per phase: `2`

Beyond that:

- reframe
- escalate
- or continue with explicitly downgraded confidence

See [review-protocol.md](references/review-protocol.md).

## Evidence Hierarchy

When reviews and reality conflict, rank evidence like this:

1. real runtime / real data observation
2. automated tests / validation runs
3. code and docs inspection
4. expert opinion

Consensus is not enough if runtime evidence contradicts it.

## Fallback Without Subagents

- `light`
  - may proceed without subagents
- `standard`
  - may proceed in fallback mode if subagents are unavailable
- `full`
  - should not claim full consensus quality without subagents
  - may continue only if the user accepts reduced assurance, or if the task is reframed down from `full`

## Safety and Documentation

Always:

- detect whether live data or destructive operations are involved
- prefer read-only diagnosis first
- prefer additive and reversible changes when possible
- keep docs in sync when behavior, architecture, or operator doctrine changes

## Completion Gate

Do not claim completion until all relevant items are true:

- functionality implemented
- validations executed
- docs updated
- review findings resolved or explicitly accepted
- residual risks stated
- branch or worktree ready for integration decision

See:

- [triage-and-gates.md](references/triage-and-gates.md)
- [review-protocol.md](references/review-protocol.md)
- [review-prompts.md](references/review-prompts.md)
