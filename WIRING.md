# WIRING — MBX-6 Agent Cards (REQ-AXO-902118)

Orchestrator-only edits (deferred to avoid worktree merge conflicts in catalog.rs / mcp.rs).
The agent did NOT touch these files. Apply the three edits below, then build.

## 1. `src/axon-core/src/mcp.rs` — module declaration

Add next to the other `mod tools_*;` (e.g. right after `mod tools_mailbox;`, line ~24):

```rust
mod tools_agent_card;
```

## 2. `src/axon-core/src/mcp.rs` — dispatch arm

In the `match normalized_name { … }` block (the mailbox arms are at lines ~1459-1460),
add alongside them:

```rust
            "mcp_agent_card" => self.axon_mcp_agent_card(arguments),
```

## 3. `src/axon-core/src/mcp/catalog.rs` — tool catalog entry

Add a new object to the tools array (place it right after the `mcp_inbox_read`
entry, which ends ~line 900):

```json
            {
                "name": "mcp_agent_card",
                "description": "[MAILBOX] REQ-AXO-902118 (MBX-6) — A2A capability discovery. A project publishes its A2A AgentCard, peers read it + discover by skill. action=set (OWNER publishes its own card — owner-write ACL, project resolved from `from`/cwd; signed via HMAC over a deterministic key-sorted canonicalisation), get (fetch one project's card + `signature_verified`), list (discover cards, optional `skill` tag filter via GIN containment on card->'skills'). A2A well-known path = /.well-known/agent-card.json. Signature reuses the internal mailbox HMAC for interop; true A2A integrity is JWS (deliberate MVP gap, sig column is forward-compatible).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["set", "get", "list"], "description": "Default get." },
                        "from": { "type": "string", "description": "set: owner project publishing the card. Default: cwd-resolved." },
                        "project": { "type": "string", "description": "get: project whose card to fetch (default cwd-resolved). set: optional self-target (must equal owner — owner-write ACL)." },
                        "card": { "type": "object", "description": "set: A2A AgentCard { name, description, url, version, protocolVersion, capabilities{...}, defaultInputModes, defaultOutputModes, skills:[{id,name,description,tags}] }." },
                        "skill": { "type": "string", "description": "list: filter to cards exposing a skill carrying this tag." }
                    },
                    "required": []
                }
            },
```

## Notes
- New DDL: `db/ddl/17_agent_card.sql` (16 = practice, 18 = sweep; 17 free).
- New helper: `mailbox::canonical_card(project, &card)` + `canonicalize_json` (BTreeMap key-sort).
- New handler file: `src/axon-core/src/mcp/tools_agent_card.rs` (`axon_mcp_agent_card`).
- Tests added in `mailbox.rs`: `canonical_card_is_key_order_independent`, `canonical_card_sign_verify_round_trip`.
- After wiring: `cargo build` + `cargo test --lib` (mailbox tests) + DDL applied on next start.
