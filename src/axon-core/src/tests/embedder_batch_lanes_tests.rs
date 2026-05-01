// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{
        current_token_lane_thresholds, reset_token_lane_classifier_for_tests,
        TokenLaneThresholdSource,
    };
    use crate::test_support::env_test_lock;

    #[test]
    fn embedder_public_batch_lane_thresholds_expose_bootstrap_contract() {
        // REQ-AXO-099 Phase 2 — env_test_lock prevents concurrent
        // tests from feeding the classifier; the explicit reset
        // drops accumulated observations so the next read returns
        // the Bootstrap source contract this test asserts.
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        reset_token_lane_classifier_for_tests();

        let thresholds = current_token_lane_thresholds();
        assert!(thresholds.small_max_tokens >= 64);
        assert!(thresholds.medium_max_tokens > thresholds.small_max_tokens);
        assert_eq!(thresholds.source, TokenLaneThresholdSource::Bootstrap);
    }
}
