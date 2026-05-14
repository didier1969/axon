# Consensus-Driven Delivery Skill Proposal

## Goal

Define a reusable, project-agnostic skill that turns a partially formed idea into:

1. a finalized concept,
2. a validated execution plan,
3. an implemented and independently reviewed result,

through structured iteration with external expert subagents.

The skill should operationalize the methodology that just worked well for the Axon live/dev dual-instance effort.

## Why This Skill Should Exist

Current skills already cover important slices:

- `brainstorming`
- `writing-plans`
- `subagent-driven-development`
- `verification-before-completion`
- `requesting-code-review`
- `axon-engineering-protocol` for Axon-specific work

But what is missing is a **meta-method** for this specific pattern:

- start from a vague or semi-formed idea;
- confront it with the real existing system;
- force independent expert challenge before locking concept and plan;
- execute with subagent review loops at major checkpoints;
- keep going autonomously until the work is fully closed, unless missing rights block execution.

This new skill should not replace the existing skills.
It should orchestrate them.

## Proposed Skill Purpose

Working name:

- `idea-to-delivery`

Alternate acceptable names:

- `iterative-consensus-delivery`
- `expert-consensus-execution`
- `three-phase-consensus-delivery`

Recommended trigger:

- use when a user has a new idea, feature, refactor, architecture direction, or cross-cutting change that is only partially specified and wants a high-rigor path from concept to implementation with expert challenge and autonomous execution.

## Trigger / Non-Trigger Rules

### Use this skill when

All of the following are materially true:

- the request is non-trivial or cross-cutting;
- the idea is still partially formed, ambiguous, or risky;
- the work likely requires concept shaping, then planning, then implementation or execution;
- independent contradiction before commitment is valuable;
- the task is large enough that structured phase gates reduce risk.

Typical triggers:

- architecture changes
- runtime or environment redesign
- migrations
- major features
- operator workflow changes
- refactors with data or runtime consequences

### Do not use this skill when

Any of the following is true:

- the task is simple and already fully specified;
- only direct implementation is needed and a plan already exists;
- the task is a pure bugfix best handled by `systematic-debugging`;
- the task is already inside a running execution workflow driven by:
  - `subagent-driven-development`
  - `executing-plans`
- the user wants only brainstorming/design and has not yet approved a design.

## Triage Levels

Before applying the full method, the skill should classify the request:

- `light`
  - small but ambiguous idea
  - concept convergence only
- `standard`
  - concept + plan + execution
  - limited review loops
- `full`
  - architecture, migration, live data, operator doctrine, or large blast radius
  - full three-phase consensus process

### Triage Exit Rules

- `light`
  - stop after concept convergence and explicit summary to the user
  - do not auto-continue into planning or implementation
- `standard`
  - run concept -> plan -> execution
- `full`
  - run concept -> plan -> execution with the strictest review discipline

## Core Philosophy

This skill would encode four hard principles:

1. **Reality first**
   - start from the real codebase, scripts, docs, runtime, and constraints
   - not from an abstract idea alone

2. **Independent contradiction before convergence**
   - concept and plan are not accepted until at least two expert subagents have challenged them
   - agreement is earned, not assumed

3. **Phased consensus**
   - concept
   - plan
   - implementation
   - each phase must be stabilized before the next

4. **Autonomous completion with controlled review**
   - the system continues by itself unless:
     - a permission/right is missing
     - a destructive action needs approval
     - a major architectural fork truly cannot be resolved from context

## It Must Be a Router, Not a Competing Workflow

This is a hard requirement.

The skill must not reinvent detailed mechanics that already exist elsewhere.
It must route explicitly to the appropriate existing skills:

- concept shaping:
  - `brainstorming`
- plan construction:
  - `writing-plans`
- implementation in the same session:
  - `subagent-driven-development`
- implementation in a separate execution session:
  - `executing-plans`
- final evidence gate:
  - `verification-before-completion`
- optional final independent review:
  - `requesting-code-review`

So this skill is:

- a phase orchestrator
- a convergence policy
- a review discipline

not a fourth competing delivery workflow.

## Three Mandatory Phases

### Phase 1: Concept Finalization

Objective:

- transform a partial idea into a robust concept aligned with the existing system

Required actions:

- inspect existing code, docs, scripts, architecture, runtime assumptions
- identify prior attempts and existing traces
- if the request still requires design exploration and explicit user approval:
  - route into `brainstorming`
  - stop this skill at concept convergence until approval exists
- once design approval exists or the concept is already sufficiently bounded:
  - write a concept or evaluation+concept document
  - submit it to two independent expert subagents
  - iterate until convergence

Required outputs:

- concept document
- explicit list of constraints
- explicit non-goals
- decision on what is reused vs changed

### Phase 2: Execution Planning

Objective:

- produce a rigorous implementation plan with dependency order and validation path

Required actions:

- route explicitly into `writing-plans`
- derive tasks from the stabilized concept
- ensure topological ordering of dependencies
- identify risky migrations, external dependencies, data safety concerns, rollout order
- specify verification and rollback where relevant
- submit the plan to two expert subagents again
- iterate until convergence

Required outputs:

- execution plan document
- ordered tasks
- validation matrix
- risk list
- rollout strategy

Document policy:

- `light`
  - concept doc only if the user wants it or if the idea needs preservation
- `standard`
  - concept doc required
  - plan doc required
- `full`
  - concept doc required
  - plan doc required
  - explicit validation matrix required

### Phase 3: Execution and Independent Qualification

Objective:

- implement and validate the work until complete

Required actions:

- create or confirm an isolated branch/worktree when relevant
- choose exactly one execution mode:
  - `subagent-driven-development`
  - or `executing-plans`
- execute through that routed skill
- after each defined checkpoint:
  - get an independent subagent review
  - if needed, a second orthogonal review (spec or code quality or runtime safety)
- correct issues iteratively
- update docs, skills, ancillary documents, and operating notes as needed
- run the required validations
- do not claim completion without evidence

Checkpoint policy:

- if execution mode is `subagent-driven-development`
  - checkpoint at each plan task
- if execution mode is `executing-plans`
  - checkpoint at each batch or phase boundary defined by the plan

Required outputs:

- implemented code or docs
- validation evidence
- updated project documentation
- final integration-ready state

## What Else the Skill Should Include

### 1. A Reuse Map of Existing Skills

This new skill should explicitly call for the use of existing skills instead of duplicating them.

It should direct the agent to use, when relevant:

- `brainstorming`
  - for initial design exploration
- `writing-plans`
  - for plan construction
- `subagent-driven-development`
  - for execution in-session
- `verification-before-completion`
  - before declaring success
- `requesting-code-review`
  - for larger implementation reviews
- domain skills
  - only when the target problem requires them

So this skill becomes an orchestrator of skills, not a competing methodology.

### 2. Explicit Subagent Role Separation

The skill should require different review roles, not generic “extra agents”.

Minimum role families:

- **Concept reviewers**
  - architecture, domain, or runtime critique
- **Plan reviewers**
  - dependency ordering, completeness, risk, rollout sanity
- **Implementation reviewers**
  - spec compliance, code quality, runtime safety, SOTA concerns when relevant

The skill should forbid using a reviewing subagent as a rubber stamp.

The skill should also push for diversity of review angle when possible:

- architecture/runtime
- plan/dependency ordering
- implementation/spec or code quality

### 3. Review Independence Rules

The skill should define how to avoid contaminating subagent review:

- do not preload the expected answer when seeking independent critique
- pass the minimum necessary context
- ask for:
  - correct points
  - blind spots
  - required corrections
  - verdict

This is important if the method is meant to remain epistemically useful.

The skill should also preserve disagreement explicitly when it exists, instead of compressing it too early into fake consensus.

### 4. A Clear Convergence Rule

The skill should define when a phase is considered converged.

For example:

- two expert reviews reached `validable` / `approved` with no blocker;
- remaining items are only minor reservations;
- the main agent also agrees that no unresolved structural contradiction remains.

Without this, the process can oscillate indefinitely.

Suggested per-phase verdict vocabulary:

- `approved`
- `approved_with_reservations`
- `needs_reframe`
- `blocked`

### 4b. An Arbitration Rule

The skill should define what happens if the two reviewers disagree materially.

Minimum rule:

- the main agent summarizes the disagreement explicitly
- the evidence hierarchy is consulted
- then choose one of:
  - revise and re-review
  - escalate to the user if the disagreement is architectural and underdetermined
  - choose the better-supported position and explain why

### 5. An Escalation / Rights Policy

The skill should include the rule already visible in practice:

- continue autonomously until blocked by missing rights or dangerous/destructive actions
- if sandbox or system rights block necessary work:
  - request escalation cleanly
  - do not silently stop

This matters because the user explicitly wants a process that goes to completion unless rights are missing.

### 5b. A Review Loop Budget

The skill should define a finite budget so the process does not spin indefinitely.

For example:

- default max review loops per phase: `2`
- beyond that:
  - reframe
  - escalate
  - or explicitly downgrade confidence

### 5c. A Fallback Without Subagents

The skill cannot assume subagents always exist.

So it should define a fallback mode:

- preferred mode:
  - two independent subagent reviews
- fallback mode:
  - explicit self-review with structured verdicts
  - stronger evidence requirements
  - mandatory disclosure that confidence is reduced because independent reviewers were unavailable

Fallback rule:

- `light`
  - may proceed without subagents
- `standard`
  - may proceed with fallback mode if subagents are unavailable
- `full`
  - should not claim full consensus quality without subagents
  - may continue only if the user accepts reduced assurance, or if the task is reframed down from `full`

### 6. A Data / Migration Safety Layer

Because many real tasks touch live state or real data, the skill should explicitly include:

- detect whether live data is involved
- prefer read-only diagnosis first
- use additive and reversible changes when possible
- distinguish:
  - concept quality
  - code quality
  - runtime/data safety

This should be generic, not Axon-specific.

### 6b. An Evidence Hierarchy

The skill should explicitly rank evidence:

1. real runtime / real data observation
2. automated tests / validation runs
3. code and docs inspection
4. expert opinion

This prevents over-trusting reviewer consensus when runtime evidence contradicts it.

### 7. A Documentation Duty

The skill should require:

- concept docs when the idea is non-trivial
- plan docs before implementation
- updates to project docs and skills if behavior or operator doctrine changes
- keeping architectural decisions explicit when they become durable

This is central to making the method self-sustaining.

### 8. A Branch / Isolation Discipline

The skill should include:

- confirm whether the work should happen on a feature branch or worktree
- isolate risky work by default when appropriate
- avoid implementing non-trivial changes directly on the main working branch unless already intended

This is especially important for generic use outside Axon.

The skill should explicitly reuse:

- `using-git-worktrees` when isolation is needed
- `finishing-a-development-branch` when execution is complete and integration choices matter

### 9. A Validation Matrix Requirement

The skill should require every plan to include:

- what is being validated
- how
- by which tool/script/test
- what constitutes success

Otherwise “validated” remains vague.

### 10. A Completion Gate

The skill should define a strong final gate:

- functionality implemented
- tests or validations executed
- docs updated
- reviews resolved
- residual risks stated
- branch ready for integration

That keeps the method honest.

### 11. A Post-Execution Retrospective

The skill should require a brief retrospective for non-trivial work:

- what worked in the method
- what failed
- what remains risky
- whether the skill itself should be updated

## What the Skill Should Not Do

It should not:

- be Axon-specific
- hardcode one stack
- duplicate detailed TDD or debugging instructions already covered by other skills
- require subagents when the platform does not support them without defining a fallback path
- require internet access in every case
- force endless review loops when there is already convergence

## Suggested Internal Structure of the Skill

The skill should likely include:

- `SKILL.md`
  - trigger conditions
  - three-phase workflow
  - convergence rules
  - review independence rules
  - branch/doc/validation obligations
- optional `references/`
  - reusable subagent prompt patterns
  - convergence checklist
  - phase gate checklist

It may also include:

- lightweight templates for:
  - concept review request
  - plan review request
  - implementation review request

## Proposed Minimal Workflow

When triggered, the skill should enforce this sequence:

1. triage `light|standard|full`
2. inspect the real system and existing traces
3. if design is still exploratory:
   - route into `brainstorming`
   - stop until design approval exists
4. write the first concept/evaluation doc
5. get two expert reviews
6. iterate until concept convergence
7. route into `writing-plans`
8. get two expert reviews on the plan
9. iterate until plan convergence
10. isolate branch/worktree when appropriate
11. choose execution mode:
   - `subagent-driven-development`
   - or `executing-plans`
12. review defined checkpoints with independent subagents
13. validate with evidence
14. update docs/skills as needed
15. run completion gate
16. prepare integration closure
17. write short retrospective if the work was non-trivial

## Routing Rules

The skill should use these explicit routing rules:

- if the request still needs design exploration and user approval:
  - route to `brainstorming`
- once concept is approved or bounded:
  - route to `writing-plans` for `standard|full`
- for execution:
  - use `subagent-driven-development` when:
    - staying in the current session
    - task-level iteration is desired
    - per-task review checkpoints are appropriate
  - use `executing-plans` when:
    - execution should happen in a separate session
    - the work is long-running or batch-oriented
    - the plan already defines larger checkpoints
- before claiming success:
  - use `verification-before-completion`

## Open Design Choices

These still need to be decided when actually creating the skill:

1. Whether the skill should require exactly two subagents or “two by default, more if justified”.
2. Whether it should provide standard prompt templates for reviewers.

## Recommendation

The best version of this skill should be:

- a **meta-orchestrator**
- skill-agnostic and project-agnostic
- strict on convergence and review quality
- autonomous by default
- explicit about rights/escalation, docs, validation, and integration readiness
- explicit about routing to existing skills instead of reimplementing them

It should not try to replace the existing specialized skills.
It should coordinate them into a disciplined, repeatable delivery method.
