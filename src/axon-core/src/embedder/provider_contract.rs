#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionLane {
    Graph,
    Vector,
}

impl ProductionLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Vector => "vector",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSupportRole {
    QuerySupport,
    VectorGpuService,
    ProviderResolver,
    TelemetryReporter,
}

impl ProviderSupportRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QuerySupport => "query_support",
            Self::VectorGpuService => "vector_gpu_service",
            Self::ProviderResolver => "provider_resolver",
            Self::TelemetryReporter => "telemetry_reporter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStrategy {
    Cpu,
    Cuda,
    TensorRt,
    Unavailable,
}

impl ProviderStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::TensorRt => "tensorrt",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn from_effective_label(label: &str) -> Self {
        let normalized = label.trim().to_ascii_lowercase();
        if normalized.starts_with("tensorrt") {
            Self::TensorRt
        } else if normalized.starts_with("cuda") {
            Self::Cuda
        } else if normalized == "unavailable" {
            Self::Unavailable
        } else {
            Self::Cpu
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResolution {
    pub requested_strategy: ProviderStrategy,
    pub effective_strategy: ProviderStrategy,
    pub production_lane: Option<ProductionLane>,
    pub support_role: Option<ProviderSupportRole>,
    pub effective_label: String,
    pub reason: Option<String>,
    pub artifact_manifest: Option<String>,
    pub provider_libraries: Vec<String>,
    pub fallback_origin: Option<String>,
}

impl ProviderResolution {
    pub fn for_production_lane(
        production_lane: ProductionLane,
        requested_strategy: ProviderStrategy,
        effective_label: impl Into<String>,
        reason: Option<String>,
    ) -> Self {
        let effective_label = effective_label.into();
        Self {
            requested_strategy,
            effective_strategy: ProviderStrategy::from_effective_label(&effective_label),
            production_lane: Some(production_lane),
            support_role: None,
            effective_label,
            reason,
            artifact_manifest: None,
            provider_libraries: Vec::new(),
            fallback_origin: None,
        }
    }

    pub fn for_support_role(
        support_role: ProviderSupportRole,
        requested_strategy: ProviderStrategy,
        effective_label: impl Into<String>,
        reason: Option<String>,
    ) -> Self {
        let effective_label = effective_label.into();
        Self {
            requested_strategy,
            effective_strategy: ProviderStrategy::from_effective_label(&effective_label),
            production_lane: None,
            support_role: Some(support_role),
            effective_label,
            reason,
            artifact_manifest: None,
            provider_libraries: Vec::new(),
            fallback_origin: None,
        }
    }
}

pub fn requested_strategy_from_label(label: &str) -> ProviderStrategy {
    match label.trim().to_ascii_lowercase().as_str() {
        "cuda" => ProviderStrategy::Cuda,
        "tensorrt" | "tensorrt_service" => ProviderStrategy::TensorRt,
        "unavailable" => ProviderStrategy::Unavailable,
        _ => ProviderStrategy::Cpu,
    }
}
