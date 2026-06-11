---
name: axon-mcp-tool-authoring
description: Use when adding a new Axon MCP tool/command, refactoring an existing one, or rolling out the optimal-for-LLM surface — anything touching src/axon-core/src/mcp/ (catalog.rs, dispatch.rs, tool_contracts.rs) or a tool's inputSchema/response shape.
---

# Axon MCP Tool Authoring (optimal-for-LLM)

Axon MCP tools are consumed by LLMs. Optimal = the LLM gets the **first call right without relying on its memory**, pays **minimum tokens**, and its **context is not polluted** (1M context does NOT cure lost-in-the-middle — it widens it).

Canonical contract + rationale + acceptance: **SOLL `GUI-AXO-1026`** (read via `sql SELECT description FROM soll.Node WHERE id='GUI-AXO-1026'`). Reference implementation: `src/axon-core/src/mcp/tool_contracts.rs` (embryo, commits 45f0864a/bedb5106/494b830e). This file is the terse HOW; never duplicate the WHY.

## 7 invariants

| # | Invariant | One line |
|---|---|---|
| 1 | Derived single-source | input = `#[derive(JsonSchema)]` struct in `tool_contracts.rs`; catalog literal = `$comment` marker; override injects. NEVER hand-write inputSchema. |
| 2 | Correct-by-construction | conditional requirements in the schema (`if`/`then`/`required`), not prose. Wrong call structurally impossible. |
| 3 | Enforced, not advertised | validate args vs schema at dispatch (`jsonschema`) → structured error. Schema is a gate, not a doc. |
| 4 | Just-in-time minimal | terse by default (answer + one `next`). Schema/rationale/alternatives = pull (`detail=full`). NEVER push the guidance envelope on every response (token tax + pollutes the LLM's mid-context). |
| 5 | Repair-as-data on error | return the corrected call / real values **inline**, not prose. The lost-in-the-middle safety net (fires at the recency edge). |
| 6 | Single-source routing | follow_ups/goal/stage/token_hint/use_when in `tool_contracts::tool_routing`, not 5 scattered match arms. |
| 7 | Zero `$ref`/`$defs` | inline enums (a `$ref` to resolve is friction for an LLM). |

## Procedure (per tool / per new tool)

1. Define the input struct in `tool_contracts.rs` (+ conditional schema if requirements are action-dependent).
2. `catalog.rs`: replace the literal with a `$comment` marker; the override pass injects the derived schema.
3. Add a co-located `tool_routing` record (values = the pre-refactor match arms exactly — co-location, not behaviour change).
4. Terse-default response + minimal `next`; everything else opt-in.
5. Wire validation + repair-as-data (field_in_error + real values + corrected_call).
6. Tests: schema derivation, conditional requirements, repair envelope, terse-default, `tools_catalog()` integration (pure fn, no DB).
7. Build release green + dev smoke on the **fresh binary** (tools/list + a faulty call) + qualify-mcp before rollout. Dev-first; never promote without it.

## Anti-patterns (each = the baseline `catalog.rs` had before GUI-AXO-1026)

Hand-written `inputSchema` · field requirements in prose · pushing guidance on every response · betting on the LLM's memory of an earlier call · advertised-but-unenforced schema · `$ref` enums · partial rollout left inconsistent (e.g. 3/67 = unpredictable surface).

## The skill applies to itself

This skill stays terse, procedural, just-in-time — a verbose skill pollutes the invoking LLM's context, which is invariant #4. Keep it that way on every edit.
