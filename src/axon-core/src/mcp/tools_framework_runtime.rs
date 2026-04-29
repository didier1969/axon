use serde_json::Value;

use super::McpServer;

impl McpServer {
    pub(super) fn axon_status_impl(&self, args: &Value) -> Option<Value> {
        self.axon_status_status_impl(args)
    }
}
