use serde_json::{json, Value};

use super::McpServer;

impl McpServer {
    pub(crate) fn handle_call_tool(&self, params: Option<Value>) -> Option<Value> {
        let params = params?;
        let name = params.get("name")?.as_str()?;
        let arguments = params.get("arguments")?;

        match name {
            "axon_refine_lattice" => self.axon_refine_lattice(arguments),
            "axon_fs_read" => self.axon_fs_read(arguments),
            "axon_restore_soll" => self.axon_restore_soll(arguments),
            "axon_validate_soll" => self.axon_validate_soll(),
            "axon_query" => self.axon_query(arguments),
            "axon_soll_manager" => self.axon_soll_manager(arguments),
            "axon_export_soll" => self.axon_export_soll(),
            "axon_inspect" => self.axon_inspect(arguments),
            "axon_audit" => self.axon_audit(arguments),
            "axon_impact" => self.axon_impact(arguments),
            "axon_health" => self.axon_health(arguments),
            "axon_diff" => self.axon_diff(arguments),
            "axon_batch" => self.axon_batch(arguments),
            "axon_cypher" => self.axon_cypher(arguments),
            "axon_semantic_clones" => self.axon_semantic_clones(arguments),
            "axon_architectural_drift" => self.axon_architectural_drift(arguments),
            "axon_bidi_trace" => self.axon_bidi_trace(arguments),
            "axon_api_break_check" => self.axon_api_break_check(arguments),
            "axon_simulate_mutation" => self.axon_simulate_mutation(arguments),
            "axon_debug" => self.axon_debug(),
            _ => Some(
                json!({ "content": [{ "type": "text", "text": "Tool not found" }], "isError": true }),
            ),
        }
    }
}
