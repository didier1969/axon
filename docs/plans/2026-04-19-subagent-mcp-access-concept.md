# Subagent MCP Access Concept

## Problem

Axon MCP is reachable from the main local operator session, but isolated subagents may fail to use it even when the server is healthy.

Current state:
- Axon binds HTTP on `0.0.0.0`, so the socket itself is not localhost-only.
- Codex MCP config currently points to `http://127.0.0.1:44129/mcp` and `http://127.0.0.1:44139/mcp`.
- Axon `status` exposes the same loopback URLs as canonical runtime identity.

Failure mode:
- In an isolated subagent/container namespace, `127.0.0.1` points to the subagent namespace, not the host Axon runtime.
- The client then sees a configured MCP server, but the advertised endpoint is not reachable from its network namespace.

## Desired Outcome

Axon must expose two URL classes clearly:

1. Internal runtime URLs
- Used by Axon local scripts and intra-host probes.
- May remain loopback-based.

2. Advertised client URLs
- Used by external clients and isolated subagents.
- Must be derived from a host-reachable address, not hardcoded loopback.

The product contract must tell the truth for both.

Interpretation rule:
- `instance_identity` is runtime-local truth only.
- Isolated clients must prefer `advertised_endpoints` when present.

## Principles

1. Minimal change
- Do not redesign MCP transport or runtime topology.
- Reuse the existing `0.0.0.0` listener.

2. Truthful identity
- `status` and diagnostics must distinguish internal vs advertised endpoints.

3. Fail-closed publication
- If no host-reachable advertised address can be derived, keep the field explicit and conservative.
- Do not silently claim that `127.0.0.1` is externally reachable for isolated clients.

4. Single publication path
- Lifecycle scripts must compute the advertised URLs once.
- Runtime and operator tooling must read the same values.

5. Reachability humility
- A non-loopback advertised URL is only a candidate reachable endpoint, not proof of reachability across namespaces.

## Proposed Model

New environment layer:
- `AXON_PUBLIC_HOST`
- `AXON_MCP_PUBLIC_URL`
- `AXON_SQL_PUBLIC_URL`
- `AXON_DASHBOARD_PUBLIC_URL`

Internal URLs remain:
- `AXON_MCP_URL`
- `AXON_SQL_URL`
- `AXON_DASHBOARD_URL`

Publication rules:
- Internal URLs stay loopback-based for host-local runtime operations.
- Public URLs use `AXON_PUBLIC_HOST` when available.
- Default host derivation must use a single explicit advertised-host resolver.
- Do not assume the current startup reporting path is already suitable for client-reachable publication.

## Surface Changes

`status`
- Keep `instance_identity` for internal/runtime truth.
- Add `advertised_endpoints` for client-facing truth.

`mcp_surface_diagnostics`
- Add an explicit section describing client reachability expectations.

Fail-closed example:
- If no non-loopback public host is available, `advertised_endpoints.available=false` and diagnostics must tell the client that only host-local access is currently guaranteed until operator configuration is provided.

Operator scripts
- Provide a canonical way to print the advertised MCP endpoints.
- Optionally support syncing client config to those advertised endpoints.

## Non-Goals

- No auth redesign.
- No MCP protocol redesign.
- No requirement that all clients use the advertised URLs if they are truly host-local.
- No automatic exposure beyond the current machine/network boundary.

## Acceptance Criteria

1. Axon can still run unchanged for host-local operator flows.
2. `status` exposes both internal and advertised endpoints.
3. Advertised endpoints are non-loopback only when derived by the advertised-host resolver or explicitly provided; otherwise they remain explicit and conservative.
4. Codex config can be aligned with advertised endpoints without hand-editing ambiguity.
5. The solution is additive and does not destabilize live/dev runtime behavior.
6. At least one isolated-client validation proves that the advertised MCP URL is reachable from a non-host namespace/session, or the system explicitly reports the advertised endpoint as unresolved.
