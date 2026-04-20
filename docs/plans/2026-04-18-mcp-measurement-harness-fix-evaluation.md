# MCP Measurement Harness Fix Evaluation

Date: 2026-04-18
Status: draft-for-review
Scope: `scripts/mcp_probe_common.py` and the latency probe scripts that depend on it

## Summary

The live MCP latency warning is currently caused by a measurement harness defect, not by confirmed user-facing MCP regressions.

The immediate failure happens during MCP session initialization:

- the harness sends `initialize`
- then sends `notifications/initialized`
- the server correctly returns `202 Accepted` with an empty body for the notification path
- the harness still tries to parse that empty body as JSON
- all three latency scripts then crash before measuring any tools

## Evidence

Failing scripts:
- `measure_mcp_core_latency.py`
- `measure_project_status_stack.py`
- `measure_symbol_flow_tools.py`

Shared failure point:
- [scripts/mcp_probe_common.py](/home/dstadel/projects/axon/scripts/mcp_probe_common.py)

Current behavior:
- `rpc_call()` always runs `json.loads(raw)`
- `initialize_session()` uses `rpc_call()` for both:
  - `initialize`
  - `notifications/initialized`

But `notifications/initialized` is intentionally accepted with an empty response body by Axon MCP.

## Root Cause

The probe harness models all MCP HTTP calls as JSON-returning RPC responses.

That assumption is false for:
- notification-style calls with no response body

So the harness is stricter than the MCP behavior it is probing.

## Required Fix

1. Keep normal JSON parsing for request/response calls that must return JSON.
2. Add only an opt-in empty-body path for notification-style calls that legitimately return no body.
3. Make `initialize_session()` explicitly handle the notification step without requiring a JSON body.
4. Keep failing on unexpected empty bodies for `initialize`, `tools/call`, and SQL probe paths.

## Non-Goals

- do not change the server to return fake JSON for `notifications/initialized`
- do not weaken parsing for real MCP responses that must remain valid JSON
- do not redesign the full qualification stack in this wave

## Expected Outcome

After the fix:
- the latency probes should complete
- `quality-mcp` should report real latency results instead of a harness failure
- any remaining `warn` should reflect real measurement data
