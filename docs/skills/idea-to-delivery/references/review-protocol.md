# Review Protocol

## Required Review Angles

Prefer at least two different angles:

- architecture / runtime / migration risk
- dependency ordering / rollout / completeness
- implementation spec compliance
- implementation code quality

## Reviewer Request Contract

Ask reviewers for:

1. what is correct
2. what is missing
3. what is risky
4. what must change
5. verdict:
   - `approved`
   - `approved_with_reservations`
   - `needs_reframe`
   - `blocked`

## Independence Rules

- do not preload the expected answer
- do not tell the reviewer what you think is wrong unless the task explicitly requires validating that claim
- provide only task-local context

## Arbitration

If reviewers disagree:

1. summarize the disagreement
2. compare against evidence hierarchy
3. choose one:
   - revise and re-review
   - escalate to user
   - choose the better-supported position and explain why

## Loop Budget

Default:

- max 2 review loops per phase

If exceeded:

- reframe
- escalate
- or continue with downgraded confidence, explicitly stated

## Fallback Without Subagents

If subagents are unavailable:

- use structured self-review
- require stronger validation evidence
- state reduced confidence explicitly

Do not claim full-consensus quality in `full` mode without independent reviewers.
