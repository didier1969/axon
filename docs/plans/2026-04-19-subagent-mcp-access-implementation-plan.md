# Subagent MCP Access Implementation Plan

## Phase 1. Shared advertised endpoint model

Implement a single helper in shell runtime setup that resolves:
- `AXON_PUBLIC_HOST`
- `AXON_MCP_PUBLIC_URL`
- `AXON_SQL_PUBLIC_URL`
- `AXON_DASHBOARD_PUBLIC_URL`

Rules:
- keep internal `AXON_*_URL` loopback-based
- derive public URLs from `AXON_PUBLIC_HOST`
- if explicit `AXON_PUBLIC_HOST` is provided, prefer it
- otherwise derive from a dedicated advertised-host resolver
- do not reuse loopback-oriented startup env publication directly

## Phase 2. Runtime truth exposure

Update Rust MCP runtime surfaces:
- `status`
- `mcp_surface_diagnostics`

Add:
- `advertised_endpoints`
- `client_reachability_notes`

Keep:
- internal/runtime URLs in `instance_identity`

`client_reachability_notes` must explicitly tell clients not to use `instance_identity.*_url` as an external endpoint.

## Phase 3. Operator path

Add one canonical operator surface, preferably through `scripts/axon`, to:
- print the effective advertised endpoints
- refresh Codex MCP config from those values only through an explicit operator action

The refresh step must remain explicit because it writes outside the repo.

## Phase 4. Validation

Validation must cover:
- live and dev startup still export correct internal URLs
- advertised URLs are populated when host IP is available
- `status` and diagnostics return both endpoint classes
- no regression in `quality-mcp`
- advertised MCP reachability is validated from an isolated namespace/subagent context, not only from the host shell
- the stale-client/session case is documented: server truth may be fixed while client bindings still need explicit refresh or restart

## Phase 5. Documentation and skill alignment

Update:
- Axon engineering skill
- relevant operator notes

The skill must tell the operator:
- internal URLs are not the same as externally advertised endpoints
- isolated clients/subagents should prefer the advertised endpoints

## Review Gates

Gate A. Concept review
- runtime/infrastructure reviewer
- agent UX / MCP product reviewer

Gate B. Post-implementation review
- same two reviewers verify minimality, truthfulness, and no runtime regression

## Risks

1. Wrong public host derivation on some machines
- mitigate with explicit `AXON_PUBLIC_HOST` override

2. Drift between scripts and runtime surfaces
- mitigate by computing publication values once in startup env and reading them everywhere else

3. A derived host IP may still be unreachable from some isolated namespaces
- mitigate with explicit override plus validation from an isolated client context

4. Client config remains stale after server fix
- mitigate by exposing diagnostics and adding an explicit sync/print path

## Exit Criteria

1. Main local operator flow still works.
2. Advertised MCP URL is visible and non-loopback when possible.
3. The product tells the truth about what a subagent should use.
4. CDD reviewers converge on approved.
5. The advertised MCP URL is proven from at least one isolated client path, or the system reports the advertised endpoint as unresolved/conservative.
