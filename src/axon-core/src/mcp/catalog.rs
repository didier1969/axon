use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use crate::runtime_topology::current_runtime_process_role;
use serde_json::{json, Value};

fn is_public_tool(name: &str) -> bool {
    !matches!(name, "resume_vectorization")
}

pub(crate) fn requires_indexed_runtime(name: &str) -> bool {
    let _ = name;
    false
}

fn tool_available_in_runtime(name: &str) -> bool {
    let runtime_mode = AxonRuntimeMode::from_env();
    if name == "resume_vectorization" && matches!(runtime_mode, AxonRuntimeMode::BrainOnly) {
        return false;
    }
    let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
        runtime_mode.as_str(),
        std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
            .ok()
            .as_deref(),
    );
    let split_brain_public_authority = matches!(
        current_runtime_process_role(),
        crate::runtime_topology::AxonProcessRole::Brain
    ) && matches!(
        std::env::var("AXON_SPLIT_SHADOW_ONLY")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("yes") | Some("on")
    );

    if requires_indexed_runtime(name) {
        return split_brain_public_authority
            || matches!(
                runtime_profile,
                AxonRuntimeOperationalProfile::IndexerFullAutonomous
            );
    }

    match runtime_mode {
        AxonRuntimeMode::BrainOnly
        | AxonRuntimeMode::IndexerGraph
        | AxonRuntimeMode::IndexerVector
        | AxonRuntimeMode::IndexerFull => true,
    }
}

pub(crate) fn tools_catalog(include_internal: bool) -> Value {
    let mut catalog = json!({
        "tools": [
            {
                "name": "help",
                "description": "[LLM-only] Return tool routing, input schemas, usage examples. Use help(tool=X) for any tool's contract. Call first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "enum": ["overview", "routing", "soll", "delivery", "runtime"],
                            "description": "Optional topic. Default: compact overview."
                        },
                        "intent": {
                            "type": "string",
                            "enum": ["understand_symbol", "prepare_edit", "commit_work", "stabilize_soll", "runtime_check"],
                            "description": "Optional LLM intent. Returns a minimal machine-actionable protocol."
                        },
                        "tool": {
                            "type": "string",
                            "description": "MCP tool name. Returns its input contract, compact examples, and recommended next action."
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "refine_lattice",
                "description": "[SYSTEM] Advanced post-ingestion graph refinement to link cross-language boundaries (e.g. Elixir NIF -> native Rust) and deepen structural analysis.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "fs_read",
                "description": "[DX] Read file content by path. Use after query/inspect identifies the target URI.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Full path to the file (e.g. 'src/main.rs')" },
                        "start_line": { "type": "integer", "description": "Optional start line" },
                        "end_line": { "type": "integer", "description": "Optional end line" }
                    },
                    "required": ["uri"]
                }
            },
            {
                "name": "soll_manager",
                "description": "[SOLL] Create/update/link intent entities. Server assigns canonical IDs. Requires: action, entity, data.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["create", "update", "link"], "description": "The operation to perform." },
                        "entity": { "type": "string", "enum": ["vision", "pillar", "requirement", "concept", "milestone", "decision", "stakeholder", "validation", "guideline"], "description": "The target entity type." },
                        "data": {
                            "type": "object",
                            "description": "JSON data. \n- create (vision/pillar/requirement/concept/decision/milestone/stakeholder/validation/guideline) with `project_code`; server assigns canonical ID `TYPE-CODE-NNN`.\n- update (canonical id required, status/desc/etc).\n- link (canonical source_id, target_id)."
                        }
                    },
                    "required": ["action", "entity", "data"]
                }
            },
            {
                "name": "infer_soll_mutation",
                "description": "[SOLL] Read-only assistive analysis. Proposes entity type, impacted canonical IDs, suggested operation, confidence level, and ambiguities before mutation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code." },
                        "statement": { "type": "string", "description": "Nuance, constraint, or clarification to stabilize." }
                    },
                    "required": ["project_code", "statement"]
                }
            },
            {
                "name": "entrench_nuance",
                "description": "[SOLL] Bounded high-level workflow to stabilize a nuance on existing canonical entities. Proposes only by default; requires `confirm=true` to write.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code." },
                        "statement": { "type": "string", "description": "Nuance or constraint to entrench." },
                        "target_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Target canonical IDs. If omitted, the server reuses inferred candidates."
                        },
                        "confirm": { "type": "boolean", "description": "Must be `true` to apply updates in wave 1." }
                    },
                    "required": ["project_code", "statement"]
                }
            },
            {
                "name": "axon_init_project",
                "description": "[DX/SOLL] Initializes a new Axon project. The server assigns the canonical `project_code` and immediately returns `project_code`, `project_name`, and `project_path` in the same response.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_name": { "type": "string", "description": "Optional. Display name will be derived from the last segment of `project_path`." },
                        "project_code": { "type": "string", "description": "Optional, reserved for internal compatibility. In normal usage, omit it: the server assigns and returns the canonical code." },
                        "project_path": { "type": "string", "description": "Canonical absolute path of the project (e.g. /home/dstadel/projects/BookingSystem)." },
                        "concept_document_url_or_text": { "type": "string", "description": "Optional: text or link to the project vision." },
                        "session_pointer": {
                            "type": "object",
                            "description": "REQ-AXO-143: workflow-agnostic onboarding pointer persisted on the project. Surfaced by `axon_init_project.data.kickoff_bundle.session_pointer` and `status.data.instance_identity.session_pointer`. Pass null to clear. `kind=file` → value is a path; `kind=url` → value is a URL (Linear/Notion/etc.); `kind=soll_node` → value is a canonical SOLL ID (e.g. CPT-AXO-019); `kind=none` → declares no pointer (value optional).",
                            "properties": {
                                "kind": { "type": "string", "enum": ["file", "url", "soll_node", "none"] },
                                "value": { "type": "string", "description": "Required when kind is file|url|soll_node." },
                                "label": { "type": "string", "description": "Optional human-friendly label." }
                            },
                            "required": ["kind"]
                        }
                    },
                    "required": ["project_path"]
                }
            },
            {
                "name": "axon_apply_guidelines",
                "description": "[DX/SOLL] Instantiates selected global rules for a specific project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "3-character canonical code of the target project." },
                        "accepted_global_rule_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of canonical global rule IDs to apply (e.g. GUI-PRO-001)."
                        }
                    },
                    "required": ["project_code", "accepted_global_rule_ids"]
                }
            },
            {
                "name": "axon_commit_work",
                "description": "[DX/SOLL] MANDATORY tool to validate and commit work. Evaluates modified files against SOLL Guidelines. NEVER use git commit via shell.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of modified file paths."
                        },
                        "message": { "type": "string", "description": "Commit message (Conventional Commits)." },
                        "dry_run": { "type": "boolean", "description": "If true, validates only without committing." },
                        "project_path": { "type": "string", "description": "REQ-AXO-191: absolute path of the project to commit in. When set, git commands run with this directory as cwd. Required for cross-project commits; otherwise the brain's cwd (typically the Axon repo) is used and the commit lands in the wrong tree." },
                        "project_code": { "type": "string", "description": "REQ-AXO-191: alternative to project_path — server resolves the path via the registry. Falls back to brain cwd when neither is supplied." }
                    },
                    "required": ["diff_paths", "message"]
                }
            },
            {
                "name": "axon_pre_flight_check",
                "description": "[DX/SOLL] Mandatory dry-run validation before commit. Checks modified files against SOLL Guidelines without creating a commit.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of modified file paths."
                        },
                        "message": { "type": "string", "description": "Optional message to log the validation. Default: 'pre-flight-check'." },
                        "incremental": { "type": "boolean", "description": "If true (default false), validate each file individually and return per-file violations. Use to detect a TDD-gate failure on file 1 without first authoring files 2..N." }
                    },
                    "required": ["diff_paths"]
                }
            },
            {
                "name": "soll_apply_plan",
                "description": "[SOLL] Idempotent high-level wrapper to apply a SOLL plan (pillars, requirements, decisions, milestones, visions, concepts) with dry-run, canonical relations, and created/updated/skipped/errors report. Async mutation: poll `job_status` until terminal. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Existing canonical project code (e.g. AXO). The server then assigns `preview_id` and created canonical IDs." },
                        "author": { "type": "string", "description": "Author of the preview/revision. Recommended for audit." },
                        "dry_run": { "type": "boolean", "description": "If true, makes no changes and only produces the action plan." },
                        "plan": {
                            "type": "object",
                            "description": "Optional collections by type. Each item accepts `logical_key`, `title`, `description`, `status`, `metadata`, and type-specific business fields. `logical_key` makes the operation idempotent.",
                            "properties": {
                                "pillars": { "type": "array", "items": { "type": "object" } },
                                "requirements": { "type": "array", "items": { "type": "object" } },
                                "decisions": { "type": "array", "items": { "type": "object" } },
                                "milestones": { "type": "array", "items": { "type": "object" } },
                                "visions": { "type": "array", "items": { "type": "object" } },
                                "concepts": { "type": "array", "items": { "type": "object" } }
                            }
                        },
                        "relations": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Links to create: `{source_id,target_id,relation_type}`. `source_id`/`target_id` can reference a `logical_key` created in the same plan."
                        },
                        "reserved_preview_id": {
                            "type": "string",
                            "description": "Optional internal/tests. In normal usage, omit: the server assigns `preview_id`."
                        }
                    },
                    "required": ["project_code", "plan"]
                }
            },
            {
                "name": "soll_commit_revision",
                "description": "[SOLL] Atomic commit of a SOLL preview into a journaled revision. Client provides `preview_id`; server assigns `revision_id`. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "preview_id": { "type": "string" },
                        "author": { "type": "string" }
                    },
                    "required": ["preview_id"]
                }
            },
            {
                "name": "soll_query_context",
                "description": "[SOLL] Return compact project intent: visions, requirements, decisions, revisions. LLM-ready.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_work_plan",
                "description": "[SOLL] Produces a read-only work plan from the canonical intentional graph, with parallel waves, blockers, cycles, and validation gates. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" },
                        "limit": { "type": "integer" },
                        "top": { "type": "integer" },
                        "include_ist": { "type": "boolean" },
                        "include_validation_details": { "type": "boolean", "description": "Default false to preserve LLM context. Set true only if full `soll_verify_requirements` details are needed." },
                        "include_decay": { "type": "boolean", "description": "REQ-AXO-144: temporal score decay. Default true. Set false to disable (benchmarking, A/B). Decays score by exp(-age_days/half_life_days) when the node carries `updated_at` metadata." },
                        "half_life_days": { "type": "number", "description": "REQ-AXO-144: half-life (in days) for the temporal decay curve. Default 30." },
                        "format": { "type": "string", "enum": ["brief", "verbose", "json"] }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "soll_attach_evidence",
                "description": "[SOLL] Attaches evidence (file/test/metric/dashboard) to a SOLL entity. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity_type": { "type": "string" },
                        "entity_id": { "type": "string" },
                        "artifacts": { "type": "array", "items": { "type": "object" } }
                    },
                    "required": ["entity_type", "entity_id", "artifacts"]
                }
            },
            {
                "name": "document_intent",
                "description": "[DX/SOLL] Records an LLM-observed intent (requirement/decision/concept/guideline) into SOLL with auto-classification. Universal entry point for 'document this' / 'documente' / 'save observation' workflows; discoverable via tools_catalog so a fresh LLM can log to SOLL without per-client prompt configuration. Server-side classifier picks `requirement` (problem/gap/friction), `decision` (choice/picks/we will), `concept` (mental model / how this works), or `guideline` (rule/method/style) when `suggest_type` is omitted. Returns canonical SOLL ID + entity_type chosen + classification reason.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "intent": { "type": "string", "description": "One-line summary used as the SOLL title." },
                        "body": { "type": "string", "description": "Full description / rationale / acceptance text." },
                        "suggest_type": { "type": "string", "enum": ["requirement", "decision", "concept", "guideline"], "description": "Optional hint; omit to let the server classify." },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tag list, persisted to metadata.tags." },
                        "project_code": { "type": "string", "description": "Optional canonical project code; resolved from cwd if omitted." }
                    },
                    "required": ["intent", "body"]
                }
            },
            {
                "name": "soll_verify_requirements",
                "description": "[SOLL] Verify requirement coverage: done/partial/missing with top gaps and next-to-close.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_rollback_revision",
                "description": "[SOLL] Best-effort rollback of a SOLL revision via the RevisionChange journal. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "revision_id": { "type": "string" }
                    },
                    "required": ["revision_id"]
                }
            },
            {
                "name": "soll_export",
                "description": "[SOLL] Exports the entire canonical intentional graph into a timestamped Markdown document. Canonical exports live under `docs/vision/`; read-only historical snapshots live under `docs/archive/soll-exports/`. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filters export to the requested project." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_generate_docs",
                "description": "[SOLL] Generates navigable human documentation derived from SOLL as a static HTML+Mermaid site. Maintains derived output under `docs/derived/soll/`, including the global multi-project root. This output is explicitly non-canonical: read yes, restore no. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project to document." },
                        "output_dir": { "type": "string", "description": "Optional project root directory. If provided alone, generates only the targeted project site." },
                        "site_root_dir": { "type": "string", "description": "Optional site root. Generates <site_root_dir>/index.html and <site_root_dir>/<project_code>/..." }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "restore_soll",
                "description": "[SOLL] Restores conceptual entities from an official SOLL Markdown export. Operates in merge mode without implicit destructive purge; reserved for explicit restoration flows. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Optional path to a SOLL export. Default: latest docs/vision/SOLL_EXPORT_*.md file." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_validate",
                "description": "[SOLL] Read-only validation of the intentional graph: structural coherence, completeness, and repair_guidance, without modifying SOLL. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filters validation to the requested project." }
                    },
                    "required": []
                }
            },
            {
                "name": "job_status",
                "description": "[SYSTEM] Returns detailed state of a mutator MCP job accepted by the shared server. Canonical async mutation tracking: read `data.state`, `data.result`, `data.error_text`. REQ-AXO-146: pass `wait: true` to block until terminal (completed|failed) or `timeout_ms` elapses, eliminating polling round-trips.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Job identifier (e.g. JOB-1712851200000)." },
                        "wait": { "type": "boolean", "description": "REQ-AXO-146: when true, blocks the call until the job reaches a terminal state or `timeout_ms` elapses. Default false (polling). When wait completes, `data.wait_metadata` reports polls/elapsed_ms/timed_out/reached_terminal." },
                        "timeout_ms": { "type": "integer", "description": "REQ-AXO-146: max time (ms) to wait when `wait=true`. Default 30000. On timeout the response carries the latest snapshot plus `data.next_action.kind = continue_polling_until_terminal_state` so existing polling guidance still applies." },
                        "poll_interval_ms": { "type": "integer", "description": "REQ-AXO-146: internal sleep (ms) between snapshot reads when `wait=true`. Default 250. Floor 10." }
                    },
                    "required": ["job_id"]
                }
            },
            {
                "name": "status",
                "description": "[SYSTEM] Return runtime mode, profile, public tools, pressure signals, auto-detected project. Call second after help().",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "mcp_surface_diagnostics",
                "description": "[SYSTEM] Public MCP surface diagnostic: server truth on exposed tools, critical tools, canonical async contract, and explicit guidance if a client appears to use a stale or incomplete binding.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["brief", "json"] }
                    },
                    "required": []
                }
            },
            {
                "name": "project_status",
                "description": "[SYSTEM/SOLL] Return project vision, SOLL coverage, runtime state, diagnostics. Use for project-scoped truth.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "project_registry_lookup",
                "description": "[SYSTEM/SOLL] Resolves a canonical project from `project_code`, `project_name`, or `project_path`. Returns stable project identity without indirect lookup.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code if known." },
                        "project_name": { "type": "string", "description": "Expected project name, typically the last path segment." },
                        "project_path": { "type": "string", "description": "Canonical absolute path of the project." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_relation_schema",
                "description": "[SOLL] Exposes the canonical SOLL relation policy for a source/target pair or from a source type/id. Discovers valid links without trial and error.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_type": { "type": "string", "description": "Short canonical type e.g. VIS, PIL, REQ, DEC." },
                        "target_type": { "type": "string", "description": "Short canonical type e.g. VIS, PIL, REQ, DEC, ART." },
                        "source_id": { "type": "string", "description": "Optional source canonical ID." },
                        "target_id": { "type": "string", "description": "Optional target canonical ID." }
                    },
                    "required": []
                }
            },
            {
                "name": "snapshot_history",
                "description": "[SYSTEM] Non-canonical derived history of structural snapshots exported by `project_status` for a project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "limit": { "type": "integer", "description": "Maximum number of snapshots returned (default 10)." }
                    },
                    "required": []
                }
            },
            {
                "name": "snapshot_diff",
                "description": "[SYSTEM] Derived diff between two non-canonical structural snapshots of a project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "from_snapshot_id": { "type": "string", "description": "Optional source snapshot; default: previous." },
                        "to_snapshot_id": { "type": "string", "description": "Optional target snapshot; default: latest." }
                    },
                    "required": []
                }
            },
            {
                "name": "conception_view",
                "description": "[SYSTEM/DX] Read-only derived conception view: modules, interfaces, contracts, flows, and suspected boundary violations.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "mode": { "type": "string", "enum": ["brief", "full"] }
                    },
                    "required": []
                }
            },
            {
                "name": "change_safety",
                "description": "[SYSTEM/DX/SOLL] Summarizes change safety of a target via tests, traceability, and derived validation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "target": { "type": "string", "description": "Target symbol, file, or entity." },
                        "target_type": { "type": "string", "enum": ["symbol", "file", "intent"] },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["target"]
                }
            },
            {
                "name": "why",
                "description": "[DX/SOLL] Return governing rationale for a symbol/file. Links code evidence to SOLL intent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Target symbol or entity." },
                        "question": { "type": "string", "description": "Free-form question if the symbol alone is not enough." },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "path",
                "description": "[DX] Trace execution/dependency path between two symbols. Single anchor: topological neighborhood.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string", "description": "Source symbol or starting anchor." },
                        "sink": { "type": "string", "description": "Optional target symbol." },
                        "project": { "type": "string" },
                        "depth": { "type": "integer", "description": "Maximum depth (default 6)." },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["source"]
                }
            },
            {
                "name": "anomalies",
                "description": "[GOVERNANCE] Return structural anomalies: cycles, god objects, wrappers, orphans. Ranked by severity.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "retrieve_context",
                "description": "[DX] Assemble bounded evidence packet for a question. Returns: answer sketch, direct evidence, SOLL rationale.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] },
                        "token_budget": { "type": "integer" },
                        "top_k": { "type": "integer" },
                        "include_soll": { "type": "boolean" },
                        "include_graph": { "type": "boolean" }
                    },
                    "required": ["question"]
                }
            },
            {
                "name": "query",
                "description": "[DX] Search symbols by name/kind/path. Returns ranked matches. Use first for code discovery.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "inspect",
                "description": "[DX] Inspect symbol detail: source, callers, callees, stats. Use after query identifies target.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "diagnose_indexing",
                "description": "[SYSTEM] Day-1 indexing diagnostic per project: probable causes, dominant reasons, parser/runtime errors, and remediations.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Project slug or '*' for global." }
                    },
                    "required": []
                }
            },
            {
                "name": "audit",
                "description": "[GOVERNANCE] In-depth compliance check (security, quality, anti-patterns, technical debt).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "impact",
                "description": "[RISK] Predictive analysis (blast radius and critical paths).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "depth": { "type": "integer" },
                        "project": { "type": "string" },
                        "symbol": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "health",
                "description": "[GOVERNANCE] Aggregated health report (dead code, test gaps, entry points).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "diff",
                "description": "[RISK] Semantic analysis of changes (Git Diff -> affected symbols).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_content": { "type": "string" },
                        "limit": { "type": "integer", "description": "Max symbols per file (default 120, clamped 10..500)" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["diff_content"]
                }
            },
            {
                "name": "batch",
                "description": "[SYSTEM] Multi-call orchestration to optimize performance or drive multiple tools.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "calls": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "tool": { "type": "string" },
                                    "args": { "type": "object", "additionalProperties": true }
                                },
                                "required": ["tool", "args"]
                            }
                        }
                    },
                    "required": ["calls"]
                }
            },
            {
                "name": "semantic_clones",
                "description": "[GOVERNANCE] Finds semantically similar functions (logic clones) in the project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Source symbol name" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "architectural_drift",
                "description": "[GOVERNANCE] Checks architecture violations between two layers (e.g. 'ui' directly calling 'db').",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_layer": { "type": "string", "description": "Source layer (e.g. 'ui', 'frontend')" },
                        "target_layer": { "type": "string", "description": "Forbidden layer (e.g. 'db', 'repository')" }
                    },
                    "required": ["source_layer", "target_layer"]
                }
            },
            {
                "name": "bidi_trace",
                "description": "[DX] Bidirectional trace: climbs to Entry Points (up) and lists deep calls (down).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Starting symbol" },
                        "depth": { "type": "integer", "description": "Maximum depth (default: unlimited for exhaustiveness, but capped by the engine)" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "api_break_check",
                "description": "[RISK] Checks whether modifying a public symbol impacts external components.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "simulate_mutation",
                "description": "[RISK] Dry-run: computes the impact volume of a modification before coding.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "symbol": { "type": "string" },
                        "depth": { "type": "integer", "description": "Impact depth (optional)" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "schema_overview",
                "description": "[LLM/ADVANCED] Axon SQL schema overview for structured exploration when product tools (`query`, `inspect`, `retrieve_context`, `soll_*`) are insufficient. Read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "list_labels_tables",
                "description": "[LLM/ADVANCED] Compact inventory of tables/labels and key columns. Use before raw `cypher`/SQL to avoid fabricated queries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "query_examples",
                "description": "[LLM/ADVANCED] Ready-to-use query examples for structured exploration, backlog, errors, and cross-language bridges. Acts as a guardrail before raw queries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "cypher",
                "description": "[LLM/ADVANCED] Raw read-only graph query interface. Use only after `schema_overview`, `list_labels_tables`, or `query_examples`, when the product surface does not answer precisely enough.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cypher": { "type": "string" }
                    },
                    "required": ["cypher"]
                }
            },
            json!({
                "name": "debug",
                "description": "[SYSTEM] Advanced system diagnostic: Axon V2 engine state (RAM, DB, architecture, indexing) for deep runtime understanding.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            }),
            json!({
                "name": "truth_check",
                "description": "[SYSTEM] Reader-path vs canonical writer coherence check on critical counters (File/Symbol/CALLS...).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }),
            json!({
                "name": "resume_vectorization",
                "description": "[SYSTEM] Explicitly recreates the missing vectorization queue from already graph_indexed files.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            })
        ]
    });

    if let Some(tools) = catalog
        .get_mut("tools")
        .and_then(|value| value.as_array_mut())
    {
        tools.retain(|tool| {
            tool.get("name")
                .and_then(|value| value.as_str())
                .is_some_and(|name| {
                    tool_available_in_runtime(name) && (include_internal || is_public_tool(name))
                })
        });
    }

    catalog
}
