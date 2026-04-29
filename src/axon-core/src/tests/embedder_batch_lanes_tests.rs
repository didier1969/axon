// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{current_token_lane_thresholds, TokenLaneThresholdSource};

    #[test]
    fn embedder_public_batch_lane_thresholds_expose_bootstrap_contract() {
        let thresholds = current_token_lane_thresholds();
        assert!(thresholds.small_max_tokens >= 64);
        assert!(thresholds.medium_max_tokens > thresholds.small_max_tokens);
        assert_eq!(thresholds.source, TokenLaneThresholdSource::Bootstrap);
    }
}
