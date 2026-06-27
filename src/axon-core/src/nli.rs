//! REQ-AXO-902096 — NLI cross-encoder for `contradiction_check` (demande Nexus).
//!
//! `tasksource/ModernBERT-base-nli` exported to ONNX under
//! `.axon/models/nli-modernbert-base/`. Judges a (premise, hypothesis) pair →
//! {entailment | neutral | contradiction} with softmax scores. ModernBERT inputs
//! are `input_ids` + `attention_mask` (no `token_type_ids`); the single output is
//! `logits[batch, 3]` with id2label `{0: entailment, 1: neutral, 2: contradiction}`.
//!
//! This is the *verdict* stage of the two-stage anti-hallucination pipeline
//! (DEC-AXO-901660): pgvector ANN shortlist → NLI re-rank/veto. A cosine proxy is
//! explicitly rejected — similarity ≠ entailment direction.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// Default on-disk location of the provisioned NLI artifact.
pub const NLI_MODEL_DIR: &str = ".axon/models/nli-modernbert-base";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NliVerdict {
    Entailment,
    Neutral,
    Contradiction,
}

impl NliVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            NliVerdict::Entailment => "entailment",
            NliVerdict::Neutral => "neutral",
            NliVerdict::Contradiction => "contradiction",
        }
    }
}

/// Softmax probabilities over the three NLI classes (sum to 1).
#[derive(Debug, Clone, Copy)]
pub struct NliScores {
    pub entailment: f32,
    pub neutral: f32,
    pub contradiction: f32,
}

impl NliScores {
    pub fn verdict(&self) -> NliVerdict {
        if self.contradiction >= self.entailment && self.contradiction >= self.neutral {
            NliVerdict::Contradiction
        } else if self.entailment >= self.neutral {
            NliVerdict::Entailment
        } else {
            NliVerdict::Neutral
        }
    }
}

/// Loaded NLI cross-encoder (ONNX session + tokenizer).
pub struct NliClassifier {
    session: Session,
    tokenizer: Tokenizer,
}

impl NliClassifier {
    /// Load from a directory containing `model.onnx` + `tokenizer.json`. Requires
    /// the ORT dynamic library to be initialised process-wide (the brain already
    /// loads it for the embedder).
    pub fn load(model_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = model_dir.as_ref();
        let model_path = dir.join("model.onnx");
        let tok_path = dir.join("tokenizer.json");
        let session = Session::builder()
            .map_err(|e| anyhow!("ORT session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("ORT optimization level: {e}"))?
            .commit_from_file(&model_path)
            .with_context(|| format!("loading NLI ONNX {}", model_path.display()))?;
        let tokenizer = Tokenizer::from_file(&tok_path)
            .map_err(|e| anyhow!("loading NLI tokenizer {}: {e}", tok_path.display()))?;
        Ok(Self { session, tokenizer })
    }

    /// Judge whether `hypothesis` is entailed / neutral / contradicted given
    /// `premise`. Takes `&mut self` because `ort::Session::run` requires a mutable
    /// borrow; callers sharing one classifier wrap it in a `Mutex` (the veto is
    /// low-volume — top-K re-rank per query — so serialised inference is fine).
    pub fn judge(&mut self, premise: &str, hypothesis: &str) -> Result<NliScores> {
        let enc = self
            .tokenizer
            .encode((premise, hypothesis), true)
            .map_err(|e| anyhow!("NLI tokenize: {e}"))?;
        let seq = enc.get_ids().len();
        let ids: Vec<i64> = enc.get_ids().iter().map(|&x| i64::from(x)).collect();
        let mask: Vec<i64> = enc
            .get_attention_mask()
            .iter()
            .map(|&x| i64::from(x))
            .collect();
        let shape = [1_usize, seq];
        let input_ids = Tensor::from_array((shape, ids)).context("NLI input_ids tensor")?;
        let attention_mask =
            Tensor::from_array((shape, mask)).context("NLI attention_mask tensor")?;
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
            ])
            .context("NLI ORT run")?;
        let (out_shape, logits) = outputs
            .get("logits")
            .ok_or_else(|| anyhow!("NLI output missing `logits`"))?
            .try_extract_tensor::<f32>()
            .context("NLI extract logits")?;
        if logits.len() < 3 {
            return Err(anyhow!(
                "NLI logits too short: shape={:?}",
                out_shape.as_ref()
            ));
        }
        // id2label: 0=entailment, 1=neutral, 2=contradiction.
        Ok(softmax3(logits[0], logits[1], logits[2]))
    }
}

fn softmax3(a: f32, b: f32, c: f32) -> NliScores {
    let m = a.max(b).max(c);
    let (ea, eb, ec) = ((a - m).exp(), (b - m).exp(), (c - m).exp());
    let s = ea + eb + ec;
    NliScores {
        entailment: ea / s,
        neutral: eb / s,
        contradiction: ec / s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure softmax check — no model needed.
    #[test]
    fn softmax_sums_to_one_and_picks_max() {
        let s = softmax3(0.2, 0.1, 5.0);
        assert!((s.entailment + s.neutral + s.contradiction - 1.0).abs() < 1e-5);
        assert_eq!(s.verdict(), NliVerdict::Contradiction);
    }

    // Real inference — needs the ORT dylib (ORT_DYLIB_PATH) + provisioned model.
    #[test]
    #[ignore = "needs ORT dylib + .axon/models/nli-modernbert-base artifact"]
    fn nli_judges_known_pairs() {
        // cargo test cwd = crate dir; the artifact lives at repo root.
        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../.axon/models/nli-modernbert-base"
        );
        let mut nli = NliClassifier::load(model_dir).expect("load NLI model");
        let contra = nli
            .judge("The sky is blue.", "The sky is green.")
            .expect("judge");
        assert_eq!(contra.verdict(), NliVerdict::Contradiction, "{contra:?}");
        let entail = nli
            .judge("A man is eating a sandwich.", "A man is eating food.")
            .expect("judge");
        assert_eq!(entail.verdict(), NliVerdict::Entailment, "{entail:?}");
    }
}
