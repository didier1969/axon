# SOLL Work Plan V1

## Scope

This note freezes the first implementation of the read-only Axon work-plan capability:
- MCP tool: `soll_work_plan`
- CLI wrapper: `./scripts/axon work-plan`

The feature computes an ordered execution plan from `SOLL` without mutating either `SOLL` or `IST`.
V1.1 also exposes a focused shortlist of `top_recommendations` for immediate operator action.

## Planning Model

Schedulable node types:
- `Decision`
- `Requirement`
- `Milestone`

Hard precedence edges used in V1:
- `soll.SOLVES`: `Decision -> Requirement`
- `soll.BELONGS_TO`: `Requirement -> Requirement`
- `soll.BELONGS_TO`: `Requirement -> Milestone`

Signal-only relations:
- `soll.VERIFIES`
- `IMPACTS`
- `SUBSTANTIATES`

`IST` never creates scheduling edges in V1. It only contributes risk/scoring signals.

## Algorithm

1. Load planifiable SOLL nodes for one project slug.
2. Build the bounded DAG from the allowed precedence edges.
3. Detect cycles explicitly.
4. Remove cycle nodes from scheduling and report them in `cycles`.
5. Mark downstream nodes as blockers with reason `depends_on_cycle`.
6. Build topological waves of ready nodes.
7. Order nodes inside each wave with deterministic scoring and stable tie-break.
8. Extract the first `N` actionable items as `top_recommendations`.

## Scoring

- `+40 * descendants_unlocked`
- `+20` for `P0`, `+15` for `P1`, `+8` for `P2`
- `+15` if requirement state is `missing`
- `+8` if requirement state is `partial`
- `+10` if no evidence is attached
- `+8` if `include_ist=true` and degraded IST links exist
- `+5` if project backlog is visible
- `-10` for isolated milestones

Tie-break:
1. score descending
2. descendants descending
3. type order: `Decision`, `Requirement`, `Milestone`
4. ID ascending

## Limits

- V1 is intentionally deterministic and explainable, not fully semantic.
- If the current SOLL relation set becomes insufficient, a future V2 may add a dedicated dependency relation instead of overloading `BELONGS_TO`.
