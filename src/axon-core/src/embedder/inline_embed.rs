//! Inline embed channel — H.1 foundation (DEC-AXO-071).
//!
//! Lets graph projection workers route embed requests synchronously through
//! the single vector-lane BGE-Large model instead of relying on the
//! `FileVectorizationQueue`. Keeping a single GPU model load is the contract
//! that prevents the multi-worker OOM cascade described in REQ-AXO-181 step
//! 4. The vector lane drains inline requests between queue fetches with
//! implicit priority.
//!
//! H.1 wires only the channel: there are zero senders by default, so
//! `inline_pipeline_enabled()` returns false and graph workers continue to
//! use the queue (DEC-AXO-070 commit G behavior preserved). H.2 will add the
//! `graph_ingestion` call site, gated by `AXON_VECTOR_PIPELINE_INLINE=true`.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result as AnyhowResult};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};

// H.1 foundation — channel plumbing only. H.2 will add graph_ingestion
// call sites. With zero senders by default, all symbols are unused at
// compile time. Module-level allow covers the staged infrastructure.
#[allow(dead_code)]
const INLINE_INBOX_CAPACITY: usize = 64;

#[allow(dead_code)]
const INLINE_EMBED_TIMEOUT: Duration = Duration::from_secs(30);

#[allow(dead_code)]
pub(crate) struct InlineEmbedRequest {
    pub(crate) texts: Vec<String>,
    pub(crate) respond_to: Sender<AnyhowResult<Vec<Vec<f32>>>>,
}

static INLINE_TX: OnceLock<Sender<InlineEmbedRequest>> = OnceLock::new();

#[allow(dead_code)]
pub(crate) fn create_vector_lane_inbox() -> (Sender<InlineEmbedRequest>, Receiver<InlineEmbedRequest>) {
    bounded(INLINE_INBOX_CAPACITY)
}

#[allow(dead_code)]
pub(crate) fn register_vector_lane_inbox(tx: Sender<InlineEmbedRequest>) -> bool {
    INLINE_TX.set(tx).is_ok()
}

#[allow(dead_code)]
pub fn inline_pipeline_enabled() -> bool {
    std::env::var("AXON_VECTOR_PIPELINE_INLINE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[allow(dead_code)]
pub fn embed_via_vector_lane(texts: Vec<String>) -> AnyhowResult<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let tx = INLINE_TX
        .get()
        .ok_or_else(|| anyhow!("inline_embed: vector lane inbox not registered"))?;
    let (resp_tx, resp_rx) = bounded::<AnyhowResult<Vec<Vec<f32>>>>(1);
    tx.send(InlineEmbedRequest {
        texts,
        respond_to: resp_tx,
    })
    .map_err(|_| anyhow!("inline_embed: vector lane inbox channel closed"))?;
    match resp_rx.recv_timeout(INLINE_EMBED_TIMEOUT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            Err(anyhow!("inline_embed: vector lane response timeout"))
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err(anyhow!("inline_embed: vector lane response channel closed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn inline_pipeline_disabled_without_env() {
        let prev = std::env::var("AXON_VECTOR_PIPELINE_INLINE").ok();
        std::env::remove_var("AXON_VECTOR_PIPELINE_INLINE");
        assert!(!inline_pipeline_enabled());
        if let Some(v) = prev {
            std::env::set_var("AXON_VECTOR_PIPELINE_INLINE", v);
        }
    }

    #[test]
    fn inline_pipeline_enabled_for_truthy_values() {
        let prev = std::env::var("AXON_VECTOR_PIPELINE_INLINE").ok();
        for v in ["true", "TRUE", "1", "yes", "on"] {
            std::env::set_var("AXON_VECTOR_PIPELINE_INLINE", v);
            assert!(inline_pipeline_enabled(), "expected enabled for {v}");
        }
        for v in ["", "false", "0", "no", "off", "garbage"] {
            std::env::set_var("AXON_VECTOR_PIPELINE_INLINE", v);
            assert!(!inline_pipeline_enabled(), "expected disabled for {v:?}");
        }
        match prev {
            Some(v) => std::env::set_var("AXON_VECTOR_PIPELINE_INLINE", v),
            None => std::env::remove_var("AXON_VECTOR_PIPELINE_INLINE"),
        }
    }

    #[test]
    fn embed_via_vector_lane_short_circuits_on_empty_input() {
        // Empty input never touches the global inbox.
        assert_eq!(
            embed_via_vector_lane(Vec::new()).unwrap(),
            Vec::<Vec<f32>>::new()
        );
    }

    #[test]
    fn channel_round_trip_with_stub_lane() {
        // Local inbox — does NOT touch the global OnceLock so we stay
        // independent of any other lane registered in the same process.
        let (tx, rx) = create_vector_lane_inbox();
        let lane = thread::spawn(move || {
            let req = rx.recv().expect("inline request received");
            let embeddings: Vec<Vec<f32>> =
                req.texts.iter().map(|_| vec![0.5_f32; 4]).collect();
            req.respond_to
                .send(Ok(embeddings))
                .expect("response sent back");
        });

        let (resp_tx, resp_rx) = bounded::<AnyhowResult<Vec<Vec<f32>>>>(1);
        tx.send(InlineEmbedRequest {
            texts: vec!["alpha".into(), "beta".into()],
            respond_to: resp_tx,
        })
        .expect("inline request enqueued");
        let result = resp_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("response received")
            .expect("embed Ok");

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec![0.5_f32; 4]);
        assert_eq!(result[1], vec![0.5_f32; 4]);
        lane.join().expect("stub lane thread joined");
    }

    #[test]
    fn channel_round_trip_propagates_lane_error() {
        let (tx, rx) = create_vector_lane_inbox();
        let lane = thread::spawn(move || {
            let req = rx.recv().expect("inline request received");
            req.respond_to
                .send(Err(anyhow!("simulated embed failure")))
                .expect("response sent back");
        });

        let (resp_tx, resp_rx) = bounded::<AnyhowResult<Vec<Vec<f32>>>>(1);
        tx.send(InlineEmbedRequest {
            texts: vec!["alpha".into()],
            respond_to: resp_tx,
        })
        .expect("inline request enqueued");
        let err = resp_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("response received")
            .expect_err("embed Err propagated");

        assert!(err.to_string().contains("simulated embed failure"), "{err}");
        lane.join().expect("stub lane thread joined");
    }
}
