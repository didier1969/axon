use serde_json::{json, Value};

use super::McpServer;

impl McpServer {
    pub(crate) fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
        let params = params?;
        let name = params.get("name")?.as_str()?;
        let normalized_name = name.strip_prefix("axon_").unwrap_or(name);
        let arguments = params.get("arguments")?;

        match normalized_name {
            "refine_lattice" => self.axon_refine_lattice(arguments),
            "fs_read" => self.axon_fs_read(arguments),
            "restore_soll" => self.axon_restore_soll(arguments),
            "validate_soll" => self.axon_validate_soll(arguments),
            "soll_apply_plan" => self.axon_soll_apply_plan(arguments),
            "soll_apply_plan_v2" => self.axon_soll_apply_plan_v2(arguments),
            "soll_commit_revision" => self.axon_soll_commit_revision(arguments),
            "soll_query_context" => self.axon_soll_query_context(arguments),
            "soll_work_plan" => self.axon_soll_work_plan(arguments),
            "soll_attach_evidence" => self.axon_soll_attach_evidence(arguments),
            "soll_verify_requirements" => self.axon_soll_verify_requirements(arguments),
            "soll_rollback_revision" => self.axon_soll_rollback_revision(arguments),
            "query" => self.axon_query(arguments),
            "soll_manager" => self.axon_soll_manager(arguments),
            "export_soll" => self.axon_export_soll(arguments),
            "diagnose_indexing" => self.axon_diagnose_indexing(arguments),
            "inspect" => self.axon_inspect(arguments),
            "audit" => self.axon_audit(arguments),
            "impact" => self.axon_impact(arguments),
            "health" => self.axon_health(arguments),
            "diff" => self.axon_diff(arguments),
            "batch" => self.axon_batch(arguments),
            "cypher" => self.axon_cypher(arguments),
            "semantic_clones" => self.axon_semantic_clones(arguments),
            "architectural_drift" => self.axon_architectural_drift(arguments),
            "bidi_trace" => self.axon_bidi_trace(arguments),
            "api_break_check" => self.axon_api_break_check(arguments),
            "simulate_mutation" => self.axon_simulate_mutation(arguments),
            "debug" => self.axon_debug_with_args(arguments),
            "schema_overview" => self.axon_schema_overview(arguments),
            "list_labels_tables" => self.axon_list_labels_tables(arguments),
            "query_examples" => self.axon_query_examples(arguments),
            "truth_check" => self.axon_truth_check(arguments),
            "resume_vectorization" => self.axon_resume_vectorization(arguments),
            _ => Some(
                json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true }),
            ),
        }
    }
}
