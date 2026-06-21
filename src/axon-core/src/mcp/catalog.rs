use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
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
    if requires_indexed_runtime(name) {
        return matches!(
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
                            "enum": ["understand_symbol", "prepare_edit", "commit_work", "stabilize_soll", "author_soll", "runtime_check"],
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
                "description": "[SOLL] Create/update/link/unlink intent entities. Server assigns canonical IDs. Requires: action, entity, data. MIL-AXO-020: `id` is DB-allocated for action=create — supplying `data.id` or `reserved_id` is rejected with `id_field_forbidden`. Vision creation forbidden outside `axon_init_project`. REQ-AXO-91592: action=unlink removes one SOLL edge with audit (soll.Revision + soll.RevisionChange) — symmetric to action=link.",
                // REQ-AXO-901949 — inputSchema derived from
                // tool_contracts::SollManagerInput (single source); the override
                // pass injects it post-build. The per-action field-routing
                // guidance lives in this tool's `description` above (DRY — it was
                // duplicated verbatim in the old `data.description`).
                "inputSchema": { "$comment": "derived from tool_contracts::SollManagerInput — injected post-build" }
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
                "name": "axon_apply_methodology_bundle",
                "description": "[DX/SOLL] REQ-AXO-276 — Apply a versioned methodology bundle (methodology-{semver}.json) to live SOLL. Reads bundle file, validates schema (`axon-methodology-bundle-v1`), then composes soll_apply_plan + soll_manager calls to seed pillars/concepts/decisions/requirements/guidelines for the bundle's target project_code (typically PRO). Idempotent : regularization stanzas (`regularization=true`) are skipped to avoid duplicating existing canonical nodes. Relations are NOT auto-applied in v1.0 (canonical relation policy gaps tracked in REQ-AXO-274) — apply manually via soll_manager(action=link) after.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "bundle_path": { "type": "string", "description": "Absolute path to the methodology bundle JSON file (e.g. /home/user/projects/axon-methodology/methodology-1.0.0.json)." },
                        "dry_run": { "type": "boolean", "description": "If true, no SOLL writes; returns the would-be apply summary." },
                        "force": { "type": "boolean", "description": "If true, bypasses axon_min_version compatibility check. Admin/migration use." }
                    },
                    "required": ["bundle_path"]
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
                        "dry_run": { "type": "boolean", "description": "If true, makes no changes and only produces the action plan. Default `false` (REQ-AXO-901625): `succeeded` means applied, mirroring `soll_manager` contract. Opt-in to preview by passing `dry_run=true` explicitly." },
                        "plan": {
                            "type": "object",
                            "description": "Collections by type. Each item accepts `logical_key`, `title`, `description`, `status`, `metadata`, and type-specific business fields. `logical_key` makes the operation idempotent. REQ-AXO-901992 B3 — EVERY non-Vision item ALSO REQUIRES `attach_to` + `relation_type` (the commit composes soll_manager(create) which enforces both). `attach_to` MUST be an ALREADY-PERSISTED canonical id (e.g. PIL-AXO-001) — it does NOT accept a `logical_key` from the same plan, so you cannot create a parent and its child in one plan: persist the parent first, then attach children in a second call. To wire two nodes created in the SAME plan, use top-level `relations` (which DOES accept same-plan logical_keys), not `attach_to`.",
                            "properties": {
                                "pillars": { "type": "array", "items": { "type": "object", "description": "Non-Vision item: requires logical_key/title + attach_to (existing canonical id) + relation_type." } },
                                "requirements": { "type": "array", "items": { "type": "object", "description": "Non-Vision item: requires logical_key/title + attach_to (existing canonical id) + relation_type." } },
                                "decisions": { "type": "array", "items": { "type": "object", "description": "Non-Vision item: requires logical_key/title + attach_to (existing canonical id) + relation_type." } },
                                "milestones": { "type": "array", "items": { "type": "object", "description": "Non-Vision item: requires logical_key/title + attach_to (existing canonical id) + relation_type." } },
                                "visions": { "type": "array", "items": { "type": "object", "description": "Vision items are exempt from attach_to/relation_type (seeded by axon_init_project)." } },
                                "concepts": { "type": "array", "items": { "type": "object", "description": "Non-Vision item: requires logical_key/title + attach_to (existing canonical id) + relation_type." } }
                            }
                        },
                        "relations": {
                            "type": "array",
                            "items": { "type": "object" },
                            "description": "Links to create: `{source_id,target_id,relation_type}`. UNLIKE plan-item `attach_to`, `source_id`/`target_id` here CAN reference a `logical_key` created in the same plan — use `relations` to wire two same-plan nodes."
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
                "name": "skill_list",
                "description": "[SOLL/SKI] List available SKI (Skill) entities for invocation. REQ-AXO-91580. Filter by `applicable_to` (task domain) or `mode_filter` (MANDATED|RECOMMENDED|OPTIONAL). Default project_code=PRO (cross-tenant methodology surface per PIL-AXO-9003 Two-Sided Identity). Cheap discovery — call FIRST in a session before invoking skills.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Defaults to 'PRO' (cross-tenant)." },
                        "applicable_to": { "type": "string", "description": "Optional task-domain filter (intersection with metadata.applicable_to)." },
                        "mode_filter": { "type": "string", "enum": ["MANDATED", "RECOMMENDED", "OPTIONAL"], "description": "Optional invocation_mode filter." }
                    },
                    "required": []
                }
            },
            {
                "name": "skill_invoke",
                "description": "[SOLL/SKI] Resolve a SKI (Skill) by canonical id and return its body + metadata. REQ-AXO-91580. Pass `id` (e.g. SKI-PRO-001) ; LLM reads body and executes procedure. Optional `context` captured for audit (future : mid-task drift telemetry per REQ-AXO-91583).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Canonical SKI id (e.g. SKI-PRO-001). Call `skill_list` to enumerate." },
                        "context": { "type": "object", "description": "Optional opaque invocation context (captured for audit)." }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "prompt_template_get",
                "description": "[SOLL/PRT] Resolve a PRT (PromptTemplate) by canonical id, validate `params` against the typed `metadata.parameters` sidecar (CPT-AXO-90017), and return the Mustache-rendered text. REQ-AXO-91581 slice 2 — required/type/default/validation_rule enforcement + Mustache logic-less rendering (engine=mustache_v1). Validation failures surface as isError with a structured `parameter_repair.errors` envelope.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Canonical PRT id (e.g. PRT-PRO-001)." },
                        "params": { "type": "object", "description": "Mustache substitution scope. Validated against metadata.parameters (required/type/default/validation_rule). Extra keys not in the sidecar are passed through unchanged." }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "re_anchor",
                "description": "[SOLL] Single-call 'where am I' packet for LLM autonomy + memory refresh per CPT-AXO-90018 design (re-anchor pattern). REQ-AXO-91582. Returns active Pillars + recent Decisions + MANDATED skills + recent SOLL revisions + canonical session_pointer body + work_plan_top in ONE envelope. Replaces 4-6 sequential MCP calls. Cheap (~10ms localhost via SOLL-RAM). LLM should call when context degrades (fill %, repeated errors, drift signal) OR periodically every K turns.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "reason": { "type": "string", "description": "Audit string — why the re-anchor is requested (drift_detected, periodic, completion_check, etc.). Echoed in response for telemetry." },
                        "project_code": { "type": "string", "description": "Defaults to auto-resolved project (cwd-based)." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_query_context",
                "description": "[SOLL] Return compact project intent: visions, requirements, decisions, revisions. LLM-ready. Pass `search` for FTS over SOLL title+description (ranked by ts_rank).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" },
                        "limit": { "type": "integer" },
                        "search": { "type": "string", "description": "REQ-AXO-901757: full-text search query. When set, returns SOLL nodes whose title+description match (to_tsvector @@ plainto_tsquery), ranked by ts_rank, instead of the project overview." }
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
                        "actionable": { "type": "boolean", "description": "REQ-AXO-346 Slice 2 + REQ-AXO-901617: when true (default), return open Requirement leaves ordered by (parent_score DESC, priority ASC, score DESC). Pass false explicitly to surface parent Decisions/Milestones (audit / priority-debug surface)." },
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
                "name": "soll_remove_evidence",
                "description": "[SOLL] Removes Traceability rows linking a SOLL entity to evidence artifacts. REQ-AXO-254 closure of MIL-AXO-015 wave G followup (broken_file_evidence cleanup). Two modes: (1) `broken_only=true` (default) removes ONLY rows whose `artifact_ref` no longer resolves to an existing file/document on disk — safe maintenance; (2) `broken_only=false` removes the explicit `artifact_refs` regardless of disk state — exact match required. Returns count + list of removed rows for audit. Operator guide: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity_id": { "type": "string", "description": "Canonical SOLL entity id (e.g. REQ-AXO-013) whose Traceability rows are candidates for removal." },
                        "broken_only": { "type": "boolean", "description": "When true (default), only remove rows whose artifact_ref does NOT exist on disk. When false, remove the explicit `artifact_refs` exactly." },
                        "artifact_refs": { "type": "array", "items": { "type": "string" }, "description": "Optional explicit list of artifact_ref values to remove (only consulted when `broken_only=false`)." }
                    },
                    "required": ["entity_id"]
                }
            },
            {
                "name": "document_intent",
                "description": "[DX/SOLL] Records an LLM-observed intent (requirement/decision/concept/guideline) into SOLL with auto-classification. Universal entry point for 'document this' / 'documente' / 'save observation' workflows; discoverable via tools_catalog so a fresh LLM can log to SOLL without per-client prompt configuration. Server-side classifier picks `requirement` (problem/gap/friction), `decision` (choice/picks/we will), `concept` (mental model / how this works), or `guideline` (rule/method/style) when `suggest_type` is omitted. REQ-AXO-901615 — when `attach_to`/`relation_type` are omitted, the server auto-infers the project's lowest-id current Pillar as fallback parent (BELONGS_TO). Returns canonical SOLL ID + entity_type chosen + classification reason + attach_source (`explicit_argument` | `auto_inferred_project_pillar`).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "intent": { "type": "string", "description": "One-line summary used as the SOLL title." },
                        "body": { "type": "string", "description": "Full description / rationale / acceptance text." },
                        "suggest_type": { "type": "string", "enum": ["requirement", "decision", "concept", "guideline"], "description": "Optional hint; omit to let the server classify." },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional tag list, persisted to metadata.tags." },
                        "project_code": { "type": "string", "description": "Optional canonical project code; resolved from cwd if omitted." },
                        "attach_to": { "type": "string", "description": "Optional canonical parent id (e.g. PIL-AXO-002, CPT-AXO-018). REQ-AXO-901615 — when omitted, the server auto-infers the lowest-id `current` Pillar in the project." },
                        "relation_type": { "type": "string", "description": "Optional canonical relation type (e.g. BELONGS_TO, EXPLAINS, REFINES). Defaults to BELONGS_TO when omitted." }
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
                        "project_code": { "type": "string", "description": "Filters validation to the requested project." },
                        "statuses_to_check": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "REQ-AXO-901602 — restrict coherence checks (orphan_requirements / decisions_without_links / uncovered_requirements / duplicate_titles) to nodes whose `status` is in this list. Default: `['current','planned']` (terminal statuses delivered/superseded/rejected/completed/accepted/archived excluded as noise). Pass `['*']` for the legacy full sweep."
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "tech_debt_inventory",
                "description": "[SOLL] REQ-AXO-901727/902031 — queryable inventory of TechnologyMigration nodes and their HAS_REMNANT remnants (per-file/symbol/chunk leftover code of an incomplete migration) + progression. Replaces accidental shell re-discovery of residue. Read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "migration_id": { "type": "string", "description": "Optional: scope to one TechnologyMigration id (TMG-…)." },
                        "from_tech": { "type": "string", "description": "Optional filter: source technology (matched against migration metadata.from_tech)." },
                        "to_tech": { "type": "string", "description": "Optional filter: target technology (matched against migration metadata.to_tech)." },
                        "status": { "type": "string", "description": "Optional migration-status filter (e.g. active, complete). Default: all." },
                        "group_by": { "type": "string", "enum": ["file", "symbol", "chunk"], "description": "Optional: restrict the listed remnants to one IST target kind." }
                    },
                    "required": []
                }
            },
            {
                "name": "data_catalog",
                "description": "[DATA] REQ-AXO-902017 — inventory of a DATA-CENTRIC project's data artifacts (CSV lakes, fixtures, manifests) from its normalized pivot catalog `data/CATALOG.json`. Answers 'how many assets, what kinds, how many rows, which lack a manifest' in one call instead of a shell dredge (ls/head/wc). Read-only; code↔data cross-reference is a later slice.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. OPV). Default: AXO." },
                        "catalog_path": { "type": "string", "description": "Optional override for the catalog location (absolute, or relative to the project root). Default: data/CATALOG.json." }
                    },
                    "required": []
                }
            },
            {
                "name": "detect_remnants",
                "description": "[SOLL] REQ-AXO-902051 — advisory scan of the IST for code-anchored residue of seeded TechnologyMigration nodes (comments excluded; sanctioned-permanent tokens like pipeline_v2 / WITH RECURSIVE never flagged). (Re)creates idempotent HAS_REMNANT edges so tech_debt_inventory + pre-flight + work-plan surface the residue. Advisory only — never a gate. Runs off the ingestion hot-path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (default: AXO)." },
                        "detect_key": { "type": "string", "description": "Optional: scope to one migration ruleset (pipeline_v1_to_v2 | nvidia_smi_to_nvml | duckdb_to_pg | age_to_pg)." },
                        "reset_baseline": { "type": "boolean", "description": "Optional (default false): re-record baseline_remnants to the current count even if one exists. Use after cleaning residue or a ruleset fix so progress is measured honestly." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_acyclic_audit",
                "description": "[SOLL] REQ-AXO-91492 — enumerate strongly-connected components (size>1) and self-loops in the SOLL intentional graph for one project. Pre-requisite for activating the DEC-AXO-098 cycle validator on `soll_manager(action=link)`. Read-only ; no mutation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO). Required." }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "ist_snapshot_warm",
                "description": "[IST] REQ-AXO-91486 — cold-load the IST CSR snapshot for one project and publish it into the process cache. Subsequent calls to migrated call-sites (impact / collect_structural_neighbors / get_circular_dependency_count_fast) dispatch to RAM unconditionally (REQ-AXO-901952 — RAM is the single source, no opt-out). Returns nodes_loaded / edges_loaded / approximate_bytes / load_ms.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO). Required." }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "ist_centrality_pagerank",
                "description": "[IST] REQ-AXO-91488 — PageRank centrality over the in-memory IST CSR. Requires `ist_snapshot_warm` first. Returns top-N nodes by score. Damping default 0.85, iterations default 50.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO)." },
                        "top": { "type": "integer", "description": "Top-N nodes returned. Default 20." },
                        "damping": { "type": "number", "description": "PageRank damping factor. Default 0.85." },
                        "iterations": { "type": "integer", "description": "PageRank iterations. Default 50." }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "ist_structural_sccs",
                "description": "[IST] REQ-AXO-91488 — Tarjan SCC over the in-memory IST CSR. Returns SCCs with size>1 (true cycles) sorted by descending size. Requires `ist_snapshot_warm` first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO)." }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "ist_shortest_path",
                "description": "[IST] REQ-AXO-91488 — bidirectional BFS shortest path between two canonical IST ids. Requires `ist_snapshot_warm`. Max radius default 20.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO)." },
                        "from": { "type": "string", "description": "Source canonical IST id." },
                        "to": { "type": "string", "description": "Target canonical IST id." },
                        "max_radius": { "type": "integer", "description": "BFS depth cap. Default 20." }
                    },
                    "required": ["project_code", "from", "to"]
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
                "name": "soll_id_registry",
                "description": "[SOLL] Returns the per-type `soll.Registry` allocation counters and the NEXT canonical id each `soll_manager(create)` would assign, so an id can be referenced in a doc/memo before it is allocated (REQ-AXO-901618). `next_id` is a lower bound (allocate_node_id gap-skips).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. \"AXO\")." }
                    },
                    "required": ["project_code"]
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
                "description": "[DX] Trace execution/dependency path between two symbols. Single anchor: topological neighborhood. With a sink, returns the shortest path plus node-disjoint alternates in `detours[]` and a `multiplicity` verdict (>1 route = redundancy candidate).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string", "description": "Source symbol or starting anchor." },
                        "sink": { "type": "string", "description": "Optional target symbol." },
                        "project": { "type": "string" },
                        "depth": { "type": "integer", "description": "Maximum depth (default 6)." },
                        "max_paths": { "type": "integer", "description": "REQ-AXO-902019 — how many node-disjoint routes to enumerate (default 3, max 5). >1 route surfaces in `detours[]` + `multiplicity` as a redundancy signal." },
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
                        "include_graph": { "type": "boolean" },
                        "semantic": { "type": "string", "enum": ["auto", "lexical", "semantic"], "description": "REQ-AXO-901978 — `lexical` skips the question embedding (FTS + structural only, fastest); default embeds (question-oriented)." },
                        "wait_for_semantic": { "type": "integer", "description": "REQ-AXO-902023 tier C — opt-in bounded wait (ms, clamped to 3000). When service pressure would degrade the corpus-wide semantic ANN, poll until it recovers to Healthy/Recovering instead of degrading immediately. `true` = 1000ms; absent = no wait (one sample)." }
                    },
                    "required": ["question"]
                }
            },
            {
                "name": "retrieve_context_layered",
                "description": "[DX/Phase-A] Multi-resolution retrieval (REQ-AXO-264, CPT-AXO-050). Returns three bands in one call: intent_band (SOLL concepts/decisions/requirements), code_band (chunks via pgvector ANN), recent_band (git log + cwd, v0 stub). Backward-compat: existing `retrieve_context` unchanged.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] },
                        "token_budget": { "type": "integer" },
                        "top_k": { "type": "integer" },
                        "semantic": { "type": "string", "enum": ["auto", "lexical", "semantic"], "description": "REQ-AXO-901978 — `lexical` skips the question embedding (FTS + structural only, fastest); default embeds. Propagated to the inner retrieve_context." },
                        "bands": {
                            "type": "object",
                            "properties": {
                                "intent": { "type": "object" },
                                "code": { "type": "object" },
                                "recent": { "type": "object" }
                            }
                        }
                    },
                    "required": ["question"]
                }
            },
            {
                "name": "query",
                "description": "[DX] Search symbols by name/kind/path. Returns ranked matches. Use first for code discovery.",
                // REQ-AXO-901949 — inputSchema derived from tool_contracts::QueryInput
                // (single source); the override pass injects it post-build.
                "inputSchema": { "$comment": "derived from tool_contracts::QueryInput — injected post-build" }
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
                "name": "embedding_status",
                "description": "[SYSTEM] Indexation counters: disk files, eligible files, indexed files, chunks, embeddings (per-project breakdown on global view) + pipeline A/B configuration. Filesystem counts cached 60s. Pair with diagnose_indexing for full triage.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Project code or '*' for global (default '*')." }
                    },
                    "required": []
                }
            },
            {
                "name": "embed_provider",
                "description": "[SYSTEM] REQ-AXO-901984 — get/set the query-embed provider at RUNTIME without a restart. action=get (default) reports override + effective provider + live worker compute. action=set, provider=cpu|gpu|auto flips it (rebuilds the query worker model on the next query). Use `cpu` to release the GPU for Axon Live / another service, `gpu` to re-grab it, `auto` for GPU-when-free.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["get", "set"], "description": "Default get." },
                        "provider": { "type": "string", "enum": ["cpu", "gpu", "auto"], "description": "Required for action=set." }
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
                "name": "fuse",
                "description": "[DX] Fused WHY+HOW for one symbol: its governing SOLL intent (REQ/DEC/PIL) AND its IST impact radius in a single RAM read. WHY-primary. RAM-only (cold/unscoped → degraded, no PG fallback).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Symbol name or canonical id to fuse." },
                        "project": { "type": "string", "description": "Project scope; auto-resolved from the symbol when omitted." },
                        "depth": { "type": "integer", "description": "Impact reverse-reach depth (default 3, clamped 1..5)." },
                        "mode": { "type": "string", "enum": ["brief", "verbose"], "description": "verbose adds the impacted-symbol list." }
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
                "description": "[GOVERNANCE] Finds semantically similar functions (logic clones). Tri-modal envelope: pgvector cosine pre-filter + VF2 graph isomorphism confirmation on per-symbol neighborhood sub-graph (REQ-AXO-91518 slice 2).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol":    { "type": "string",  "description": "Source symbol name" },
                        "project":   { "type": "string",  "description": "Optional project_code filter (e.g. 'AXO'). When provided + RAM snapshot warm, enables VF2 structural confirmation." },
                        "limit":     { "type": "integer", "description": "Max clones returned (default 5, max 1000).", "default": 5 },
                        "offset":    { "type": "integer", "description": "Pagination offset.", "default": 0 },
                        "max_depth": { "type": "integer", "description": "Neighborhood radius for VF2 sub-graph extraction (default 1, max 3).", "default": 1 }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "architectural_drift",
                "description": "[GOVERNANCE] Checks architecture violations between two layers (e.g. 'ui' directly calling 'db'). RAM-first via IstGraphView + `layer_violations` algorithm (REQ-AXO-91516, MIL-AXO-019).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_layer": { "type": "string",  "description": "Source layer prefix (e.g. 'ui', 'frontend')" },
                        "target_layer": { "type": "string",  "description": "Forbidden target layer prefix (e.g. 'db', 'repository')" },
                        "project":      { "type": "string",  "description": "Project code (required for RAM snapshot lookup, e.g. 'AXO')" },
                        "limit":        { "type": "integer", "description": "Max violations returned (default 20, max 1000).", "default": 20 },
                        "offset":       { "type": "integer", "description": "Pagination offset.", "default": 0 },
                        "sort_by":      { "type": "string",  "description": "Sort key: 'severity' (default — biggest layer gap first) or 'alphabetical'.", "default": "severity" }
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
                        "project": { "type": "string", "description": "Optional project_code (e.g. AXO). Auto-resolved from cwd/AXON_PROJECT_ROOT when omitted; required for the RAM CSR snapshot lookup." },
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
                "name": "query_examples",
                "description": "[LLM/ADVANCED] Ready-to-use query examples for structured exploration, backlog, errors, and cross-language bridges. Acts as a guardrail before raw queries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "mcp_friction_report",
                "description": "[SYSTEM/SOLL] REQ-AXO-901957 — closed-loop MCP friction log. Returns top OPEN friction signatures by frequency (rollout priorities) + RESOLVED ones with their REQ/VAL links and regression flags. Signatures record only the problem SHAPE (project_code, tool, problem_class, field name) — NEVER any argument content. Optional `mark_resolved` closes a signature against the SOLL fix that resolved it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filter to one tenant; omit for the cross-tenant aggregate." },
                        "limit": { "type": "integer", "description": "Max signatures per section (default 15)." },
                        "mark_resolved": {
                            "type": "object",
                            "description": "Close a signature: {id, resolved_by_req, resolved_by_val, note}.",
                            "properties": {
                                "id": { "type": "integer" },
                                "resolved_by_req": { "type": "string" },
                                "resolved_by_val": { "type": "string" },
                                "note": { "type": "string" }
                            },
                            "required": ["id"]
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "mcp_telemetry_report",
                "description": "[SYSTEM] REQ-AXO-901961 — MCP usage + latency analytics over the per-call rollup (axon.mcp_call_stat). Returns per-tool call volume, error rate, average + max latency over a window. Signature-only (tool + ok/error + project) — NEVER any argument content. PG-native (no external analytics tool). Args: optional `project_code` (tenant filter), `window_hours` (default 168 = 7d), `limit` (default 20).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filter to one tenant; omit for the cross-tenant aggregate." },
                        "window_hours": { "type": "integer", "description": "Look-back window in hours (default 168 = 7 days)." },
                        "limit": { "type": "integer", "description": "Max tools returned, busiest first (default 20)." }
                    },
                    "required": []
                }
            },
            {
                "name": "mcp_feedback",
                "description": "[SYSTEM] REQ-AXO-901966 — voluntary LLM feedback / doléance. Call this whenever an Axon MCP tool felt buggy, under-documented, unclear, too slow, incomplete, or too verbose. Content-rich + voluntary (complements the silent, signature-only friction log). Tell us who you are, what you hit, how serious it was, your proposed fix, and your satisfaction — it directly drives product optimization. Args: `problem` (required), optional `severity` (blocking|token_cost|minor — is it a hard blocker, or does it only cost extra tokens?), `category` (bug|unclear_doc|undocumented|too_slow|incomplete|too_verbose|other), `tool`, `proposed_solution`, `satisfaction` (1-5), `llm_identity`, `project_code`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "problem": { "type": "string", "description": "What went wrong / was unclear / slow / missing / too verbose. Required." },
                        "severity": { "type": "string", "enum": ["blocking", "token_cost", "minor"], "description": "How serious: 'blocking' (could NOT complete the task), 'token_cost' (worked but wasted significant tokens/turns), or 'minor' (cosmetic). Default 'minor'. Lets us triage graver problems first." },
                        "category": { "type": "string", "enum": ["bug", "unclear_doc", "undocumented", "too_slow", "incomplete", "too_verbose", "other"], "description": "Kind of friction (default 'other')." },
                        "tool": { "type": "string", "description": "Which Axon tool the feedback is about (optional)." },
                        "proposed_solution": { "type": "string", "description": "How you'd fix or improve it (optional)." },
                        "satisfaction": { "type": "integer", "description": "Satisfaction with the tool, 1 (poor) to 5 (excellent), optional." },
                        "llm_identity": { "type": "string", "description": "Who you are, e.g. 'Claude Opus 4.8' (optional but encouraged)." },
                        "project_code": { "type": "string", "description": "Project scope (optional)." }
                    },
                    "required": ["problem"]
                }
            },
            {
                "name": "mcp_feedback_report",
                "description": "[SYSTEM/SOLL] REQ-AXO-902020 — content-rich READ/triage counterpart to `mcp_feedback` (which was write-only). Lists voluntary LLM doléances (problem / proposed_solution / severity / satisfaction) newest-first, OPEN by default, with severity/category/tool/project filters. Optional `mark_resolved` closes an item against the SOLL fix that resolved it — symmetric to `mcp_friction_report`. Use this to triage feedback instead of raw SQL on axon.llm_feedback.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filter to one tenant; omit for the cross-tenant aggregate." },
                        "category": { "type": "string", "enum": ["bug", "unclear_doc", "undocumented", "too_slow", "incomplete", "too_verbose", "other"], "description": "Filter by feedback category (optional)." },
                        "severity": { "type": "string", "enum": ["blocking", "token_cost", "minor"], "description": "Filter by severity (optional)." },
                        "tool": { "type": "string", "description": "Filter to feedback about one Axon tool (optional)." },
                        "window_hours": { "type": "integer", "description": "Look-back window in hours (default 168 = 7 days)." },
                        "limit": { "type": "integer", "description": "Max items returned, newest-first (default 30)." },
                        "include_resolved": { "type": "boolean", "description": "Include already-triaged items (default false = open only)." },
                        "mark_resolved": {
                            "type": "object",
                            "description": "Close a feedback item: {id, resolved_by_req, note}.",
                            "properties": {
                                "id": { "type": "integer" },
                                "resolved_by_req": { "type": "string" },
                                "note": { "type": "string" }
                            },
                            "required": ["id"]
                        }
                    },
                    "required": []
                }
            },
            {
                "name": "sql",
                "description": "[LLM/ADVANCED] Raw READ-ONLY SQL query interface (SELECT / WITH / EXPLAIN / SHOW / DESCRIBE / PRAGMA only — mutations are rejected). PG-only backend post-MIL-AXO-017. Use only after `schema_overview` or `query_examples`, when the product surface does not answer precisely enough. To write intent use `soll_manager`; to report a tool problem use `mcp_feedback`.",
                // REQ-AXO-901949 — inputSchema derived from tool_contracts::SqlInput
                // (single source); the override pass injects it post-build.
                "inputSchema": { "$comment": "derived from tool_contracts::SqlInput — injected post-build" }
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
            }),
            // REQ-AXO-901676 — proportionate recovery surface for cases
            // where the indexer's incremental state is suspected stale
            // (git pull massif, backup restore, inotify drop, watcher
            // crash). Returns synchronously with `files_scheduled` +
            // `projection_eta_ms` ; the actual scan runs asynchronously
            // via the existing `axon_registry_changed` NOTIFY listener
            // (REQ-AXO-901675) so no indexer restart is required.
            json!({
                "name": "rescan_project",
                "description": "[SYSTEM] REQ-AXO-901676 — force a delta or full re-scan of a project's source tree. Use after git pull massif / backup restore / inotify drop / watcher crash. Returns `{status, files_scheduled, projection_eta_ms, project_code, mode}` in <500 ms ; triggers the indexer's existing subtree-scan plumbing (NOTIFY axon_registry_changed → record_subtree_hint). `full=false` (default) keeps IndexedFile content_hash cache so only diffs are re-parsed ; `full=true` wipes IndexedFile rows for the project so every file is forced through A1/A2/A3 + B1/B2/B3 again.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Canonical project code (e.g. AXO). Must be present in soll.ProjectCodeRegistry." },
                        "full": { "type": "boolean", "description": "When true, wipes IndexedFile rows under the project_path so the next scanner pass re-parses + re-embeds every file regardless of cached content_hash. Default false (delta scan only re-touches files whose hash changed)." }
                    },
                    "required": ["project_code"]
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

        // REQ-AXO-901949 — single source of truth: for tracer-bullet tools the
        // advertised inputSchema is derived from the Rust struct (schemars),
        // never the hand-written literal above. The literal `description` stays
        // (tool docs); only the schema is overridden so it can never drift from
        // the type the handler reads. Slice-2 rolls this over the remaining
        // catalog literals.
        for tool in tools.iter_mut() {
            let Some(name) = tool.get("name").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            if let Some(derived) = super::tool_contracts::derived_input_schema(&name) {
                if let Some(obj) = tool.as_object_mut() {
                    obj.insert("inputSchema".to_string(), derived);
                }
            }
        }
    }

    catalog
}
