use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};


const VECTOR_BATCH_LANE_WINDOW_CAPACITY: usize = 512;
const VECTOR_BATCH_LANE_LIVE_SAMPLE_MIN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VectorBatchLane {
    Small,
    Medium,
    Large,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenLaneThresholdSource {
    Bootstrap,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenLaneThresholds {
    pub(crate) small_max_tokens: usize,
    pub(crate) medium_max_tokens: usize,
    pub(crate) sample_count: usize,
    pub(crate) source: TokenLaneThresholdSource,
}

impl TokenLaneThresholds {
    pub(crate) fn classify(self, token_count: usize) -> VectorBatchLane {
        if token_count <= self.small_max_tokens {
            VectorBatchLane::Small
        } else if token_count <= self.medium_max_tokens {
            VectorBatchLane::Medium
        } else {
            VectorBatchLane::Large
        }
    }
}

#[derive(Debug, Default)]
struct TokenLaneClassifier {
    recent: VecDeque<usize>,
}

impl TokenLaneClassifier {
    fn thresholds(&self) -> TokenLaneThresholds {
        if self.recent.len() >= VECTOR_BATCH_LANE_LIVE_SAMPLE_MIN {
            let mut ordered = self.recent.iter().copied().collect::<Vec<_>>();
            ordered.sort_unstable();
            let small_idx = ((ordered.len().saturating_sub(1)) as f64 * 0.33).round() as usize;
            let medium_idx = ((ordered.len().saturating_sub(1)) as f64 * 0.66).round() as usize;
            let small = ordered
                .get(small_idx)
                .copied()
                .unwrap_or_else(|| bootstrap_token_lane_thresholds().small_max_tokens)
                .max(1);
            let medium = ordered
                .get(medium_idx)
                .copied()
                .unwrap_or_else(|| bootstrap_token_lane_thresholds().medium_max_tokens)
                .max(small.saturating_add(1));
            return TokenLaneThresholds {
                small_max_tokens: small,
                medium_max_tokens: medium,
                sample_count: ordered.len(),
                source: TokenLaneThresholdSource::Live,
            };
        }

        let mut bootstrap = bootstrap_token_lane_thresholds();
        bootstrap.sample_count = self.recent.len();
        bootstrap
    }

    fn observe(&mut self, token_counts: &[usize]) -> TokenLaneThresholds {
        for token_count in token_counts {
            self.recent.push_back((*token_count).max(1));
            while self.recent.len() > VECTOR_BATCH_LANE_WINDOW_CAPACITY {
                self.recent.pop_front();
            }
        }
        self.thresholds()
    }
}

static TOKEN_LANE_CLASSIFIER: OnceLock<Mutex<TokenLaneClassifier>> = OnceLock::new();

fn bootstrap_token_lane_thresholds() -> TokenLaneThresholds {
    let max_tokens = super::configured_embedding_max_length().max(3);
    let small = (max_tokens / 3).max(64);
    let medium = ((max_tokens * 2) / 3).max(small.saturating_add(1));
    TokenLaneThresholds {
        small_max_tokens: small,
        medium_max_tokens: medium,
        sample_count: 0,
        source: TokenLaneThresholdSource::Bootstrap,
    }
}

fn token_lane_classifier() -> &'static Mutex<TokenLaneClassifier> {
    TOKEN_LANE_CLASSIFIER.get_or_init(|| Mutex::new(TokenLaneClassifier::default()))
}

pub(crate) fn current_token_lane_thresholds() -> TokenLaneThresholds {
    token_lane_classifier()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .thresholds()
}

pub(crate) fn observe_token_lane_thresholds(token_counts: &[usize]) -> TokenLaneThresholds {
    token_lane_classifier()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .observe(token_counts)
}

#[cfg(test)]
pub(crate) fn reset_token_lane_classifier_for_tests() {
    if let Some(classifier) = TOKEN_LANE_CLASSIFIER.get() {
        classifier
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .recent
            .clear();
    }
}
