# Review Prompt Patterns

Use these as compact templates, not rigid scripts.

## Concept Review

Review this concept critically.

Deliver:

1. what is correct
2. blind spots or errors
3. what must change before validation
4. verdict: `approved`, `approved_with_reservations`, `needs_reframe`, or `blocked`

Focus on:

- architecture
- runtime assumptions
- data safety
- operator clarity

## Plan Review

Review this implementation plan critically.

Deliver:

1. what is structurally sound
2. missing dependencies or ordering problems
3. risky rollout or validation gaps
4. what must change before validation
5. verdict

Focus on:

- topological ordering
- rollback / containment
- validation matrix
- hidden singleton assumptions

## Implementation Review

Review this implementation and its evidence.

Deliver:

1. what matches the intended spec
2. what is missing or incorrect
3. code or runtime quality risks
4. required corrections
5. verdict

Focus on:

- spec compliance
- code quality
- runtime correctness
- evidence quality
