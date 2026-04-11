use crate::runtime_mode::AxonRuntimeMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxonRuntimeOperationalProfile {
    GraphOnly,
    FullIsolated,
    FullAutonomous,
    ReadOnly,
    McpOnly,
}

impl AxonRuntimeOperationalProfile {
    pub fn from_mode_and_strings(runtime_mode: &str, autonomous_ingestor: Option<&str>) -> Self {
        let mode = AxonRuntimeMode::from_str(runtime_mode);
        match mode {
            AxonRuntimeMode::GraphOnly => Self::GraphOnly,
            AxonRuntimeMode::ReadOnly => Self::ReadOnly,
            AxonRuntimeMode::McpOnly => Self::McpOnly,
            AxonRuntimeMode::Full => {
                if env_bool_like(autonomous_ingestor) {
                    Self::FullAutonomous
                } else {
                    Self::FullIsolated
                }
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GraphOnly => "graph_only",
            Self::FullIsolated => "full_isolated",
            Self::FullAutonomous => "full_autonomous",
            Self::ReadOnly => "read_only",
            Self::McpOnly => "mcp_only",
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
    fn test_full_mode_without_autonomous_flag_stays_isolated() {
        let profile = AxonRuntimeOperationalProfile::from_mode_and_strings("full", None);
        assert_eq!(profile, AxonRuntimeOperationalProfile::FullIsolated);
    }

    #[test]
    fn test_full_mode_with_autonomous_flag_promotes_to_full_autonomous() {
        let profile =
            AxonRuntimeOperationalProfile::from_mode_and_strings("full", Some("true"));
        assert_eq!(profile, AxonRuntimeOperationalProfile::FullAutonomous);
    }

    #[test]
    fn test_graph_only_ignores_autonomous_flag() {
        let profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
            "graph_only",
            Some("true"),
        );
        assert_eq!(profile, AxonRuntimeOperationalProfile::GraphOnly);
    }
}
