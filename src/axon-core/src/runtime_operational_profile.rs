use crate::runtime_mode::AxonRuntimeMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxonRuntimeOperationalProfile {
    BrainOnly,
    IndexerGraph,
    IndexerVector,
    IndexerFullIsolated,
    IndexerFullAutonomous,
}

impl AxonRuntimeOperationalProfile {
    pub fn from_mode_and_strings(runtime_mode: &str, autonomous_ingestor: Option<&str>) -> Self {
        let mode = AxonRuntimeMode::from_str(runtime_mode);
        match mode {
            AxonRuntimeMode::BrainOnly => Self::BrainOnly,
            AxonRuntimeMode::IndexerGraph => Self::IndexerGraph,
            AxonRuntimeMode::IndexerVector => Self::IndexerVector,
            AxonRuntimeMode::IndexerFull => {
                if env_bool_like(autonomous_ingestor) {
                    Self::IndexerFullAutonomous
                } else {
                    Self::IndexerFullIsolated
                }
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BrainOnly => "brain_only",
            Self::IndexerGraph => "indexer_graph",
            Self::IndexerVector => "indexer_vector",
            Self::IndexerFullIsolated => "indexer_full_isolated",
            Self::IndexerFullAutonomous => "indexer_full_autonomous",
        }
    }
}

fn env_bool_like(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

#[cfg(test)]
mod tests {
    use super::AxonRuntimeOperationalProfile;

    #[test]
    fn test_indexer_full_without_autonomous_flag_stays_isolated() {
        let profile = AxonRuntimeOperationalProfile::from_mode_and_strings("indexer_full", None);
        assert_eq!(profile, AxonRuntimeOperationalProfile::IndexerFullIsolated);
    }

    #[test]
    fn test_indexer_full_with_autonomous_flag_promotes_to_autonomous() {
        let profile =
            AxonRuntimeOperationalProfile::from_mode_and_strings("indexer_full", Some("true"));
        assert_eq!(
            profile,
            AxonRuntimeOperationalProfile::IndexerFullAutonomous
        );
    }

    #[test]
    fn test_indexer_graph_ignores_autonomous_flag() {
        let profile =
            AxonRuntimeOperationalProfile::from_mode_and_strings("indexer_graph", Some("true"));
        assert_eq!(profile, AxonRuntimeOperationalProfile::IndexerGraph);
    }
}
