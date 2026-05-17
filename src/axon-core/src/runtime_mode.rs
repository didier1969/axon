use crate::runtime_topology::AxonProcessRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxonRuntimeMode {
    BrainOnly,
    IndexerGraph,
    IndexerVector,
    IndexerFull,
}

impl AxonRuntimeMode {
    pub fn from_env() -> Self {
        Self::from_str(
            &std::env::var("AXON_RUNTIME_MODE").unwrap_or_else(|_| "indexer_full".to_string()),
        )
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "brain_only" | "brain-only" | "brainonly" => Self::BrainOnly,
            "indexer_graph" | "indexer-graph" | "indexergraph" => Self::IndexerGraph,
            "indexer_vector" | "indexer-vector" | "indexervector" => Self::IndexerVector,
            "indexer_full" | "indexer-full" | "indexerfull" => Self::IndexerFull,
            _ => Self::IndexerFull,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BrainOnly => "brain_only",
            Self::IndexerGraph => "indexer_graph",
            Self::IndexerVector => "indexer_vector",
            Self::IndexerFull => "indexer_full",
        }
    }

    pub fn declared_process_role(self) -> AxonProcessRole {
        match self {
            Self::BrainOnly => AxonProcessRole::Brain,
            Self::IndexerGraph | Self::IndexerVector | Self::IndexerFull => {
                AxonProcessRole::Indexer
            }
        }
    }

    pub fn ingestion_enabled(self) -> bool {
        matches!(self, Self::IndexerGraph | Self::IndexerFull)
    }

    pub fn semantic_workers_enabled(self) -> bool {
        matches!(self, Self::IndexerVector | Self::IndexerFull)
    }

    pub fn background_vectorization_enabled(self) -> bool {
        self.semantic_workers_enabled()
    }

    pub fn control_plane_enabled(self) -> bool {
        matches!(self, Self::BrainOnly)
    }

    pub fn serves_public_mcp(self) -> bool {
        matches!(self, Self::BrainOnly)
    }

    pub fn indexer_workload(self) -> Option<&'static str> {
        match self {
            Self::BrainOnly => None,
            Self::IndexerGraph => Some("graph"),
            Self::IndexerVector => Some("vector"),
            Self::IndexerFull => Some("full"),
        }
    }
}

pub fn canonical_embedding_provider_request_for_mode(
    runtime_mode: AxonRuntimeMode,
    gpu_present: bool,
) -> String {
    if !runtime_mode.semantic_workers_enabled() {
        return "cpu".to_string();
    }

    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if gpu_present {
                "cuda".to_string()
            } else {
                "cpu".to_string()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::AxonRuntimeMode;
    use crate::runtime_topology::AxonProcessRole;

    #[test]
    fn test_indexer_graph_mode_preserves_ingestion_without_semantic_workers() {
        let mode = AxonRuntimeMode::from_str("indexer-graph");
        assert_eq!(mode, AxonRuntimeMode::IndexerGraph);
        assert!(mode.ingestion_enabled());
        assert!(!mode.semantic_workers_enabled());
        assert!(!mode.background_vectorization_enabled());
        assert_eq!(mode.as_str(), "indexer_graph");
        assert_eq!(mode.declared_process_role(), AxonProcessRole::Indexer);
    }

    #[test]
    fn test_indexer_full_mode_keeps_background_vectorization_enabled() {
        let mode = AxonRuntimeMode::from_str("indexer_full");
        assert_eq!(mode, AxonRuntimeMode::IndexerFull);
        assert!(mode.ingestion_enabled());
        assert!(mode.semantic_workers_enabled());
        assert!(mode.background_vectorization_enabled());
        assert_eq!(mode.indexer_workload(), Some("full"));
    }

    #[test]
    fn test_brain_only_mode_has_public_control_plane_without_ingestion() {
        let mode = AxonRuntimeMode::from_str("brain_only");
        assert_eq!(mode, AxonRuntimeMode::BrainOnly);
        assert!(mode.control_plane_enabled());
        assert!(mode.serves_public_mcp());
        assert!(!mode.ingestion_enabled());
        assert_eq!(mode.declared_process_role(), AxonProcessRole::Brain);
    }

}
