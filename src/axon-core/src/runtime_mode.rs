#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxonRuntimeMode {
    Full,
    ReadOnly,
    McpOnly,
    GraphOnly,
}

impl AxonRuntimeMode {
    pub fn from_env() -> Self {
        Self::from_str(&std::env::var("AXON_RUNTIME_MODE").unwrap_or_else(|_| "full".to_string()))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "read_only" | "readonly" | "read-only" => Self::ReadOnly,
            "mcp_only" | "mcponly" | "mcp-only" => Self::McpOnly,
            "graph_only" | "graphonly" | "graph-only" => Self::GraphOnly,
            _ => Self::Full,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::ReadOnly => "read_only",
            Self::McpOnly => "mcp_only",
            Self::GraphOnly => "graph_only",
        }
    }

    pub fn ingestion_enabled(self) -> bool {
        matches!(self, Self::Full | Self::GraphOnly)
    }

    pub fn semantic_workers_enabled(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn background_vectorization_enabled(self) -> bool {
        matches!(self, Self::Full)
    }
}

pub fn graph_embeddings_enabled() -> bool {
    matches!(
        std::env::var("AXON_GRAPH_EMBEDDINGS_ENABLED")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

#[cfg(test)]
mod tests {
    use super::{graph_embeddings_enabled, AxonRuntimeMode};

    #[test]
    fn test_graph_only_mode_preserves_ingestion_without_semantic_workers() {
        let mode = AxonRuntimeMode::from_str("graph-only");
        assert_eq!(mode, AxonRuntimeMode::GraphOnly);
        assert!(mode.ingestion_enabled());
        assert!(!mode.semantic_workers_enabled());
        assert!(!mode.background_vectorization_enabled());
        assert_eq!(mode.as_str(), "graph_only");
    }

    #[test]
    fn test_full_mode_keeps_background_vectorization_enabled() {
        let mode = AxonRuntimeMode::from_str("full");
        assert_eq!(mode, AxonRuntimeMode::Full);
        assert!(mode.ingestion_enabled());
        assert!(mode.semantic_workers_enabled());
        assert!(mode.background_vectorization_enabled());
    }

    #[test]
    fn test_graph_embeddings_enabled_defaults_off_and_honors_true_values() {
        unsafe {
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        }
        assert!(!graph_embeddings_enabled());

        unsafe {
            std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        }
        assert!(graph_embeddings_enabled());

        unsafe {
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        }
    }
}
