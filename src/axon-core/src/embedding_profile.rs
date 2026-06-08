use crate::embedding_contract::MAX_LENGTH;
use anyhow::{anyhow, Result as AnyhowResult};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokenizers::{AddedToken, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingChunkProfile {
    pub model_max_tokens: usize,
    pub target_chunk_tokens: usize,
    pub overlap_tokens: usize,
    pub small_symbol_char_fast_path: usize,
    pub gray_zone_char_threshold: usize,
    pub token_bucket_size: usize,
}

pub fn configured_embedding_max_length() -> usize {
    std::env::var("AXON_EMBED_MAX_LENGTH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 32)
        .unwrap_or(MAX_LENGTH)
        .min(MAX_LENGTH)
}

pub fn configured_embedding_token_bucket_size() -> usize {
    std::env::var("AXON_EMBED_TOKEN_BUCKET_SIZE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 8)
        .unwrap_or(64)
        .min(configured_embedding_max_length().max(8))
}

pub fn runtime_chunk_profile() -> EmbeddingChunkProfile {
    let model_max_tokens = configured_embedding_max_length();
    let target_chunk_tokens = std::env::var("AXON_TARGET_CHUNK_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 64)
        .unwrap_or_else(|| ((model_max_tokens * 3) / 4).clamp(128, model_max_tokens));
    let overlap_tokens = std::env::var("AXON_CHUNK_OVERLAP_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| (target_chunk_tokens / 8).clamp(24, 96));
    let small_symbol_char_fast_path = std::env::var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 64)
        .unwrap_or_else(|| target_chunk_tokens.saturating_mul(2));
    let gray_zone_char_threshold = std::env::var("AXON_GRAY_ZONE_CHAR_THRESHOLD")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= small_symbol_char_fast_path)
        .unwrap_or_else(|| target_chunk_tokens.saturating_mul(4));

    EmbeddingChunkProfile {
        model_max_tokens,
        target_chunk_tokens: target_chunk_tokens.min(model_max_tokens),
        overlap_tokens: overlap_tokens.min(target_chunk_tokens.saturating_sub(1).max(1)),
        small_symbol_char_fast_path,
        gray_zone_char_threshold,
        token_bucket_size: configured_embedding_token_bucket_size(),
    }
}

pub fn embedding_model_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var("FASTEMBED_CACHE_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }

    if let Some(path) = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path).join("axon").join("fastembed");
    }

    if let Some(home) = std::env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(home)
            .join(".cache")
            .join("axon")
            .join("fastembed");
    }

    PathBuf::from("/tmp/axon-fastembed")
}

pub fn runtime_embedding_snapshot_dir() -> AnyhowResult<PathBuf> {
    let model_root = embedding_model_cache_dir().join("models--Xenova--bge-large-en-v1.5");
    let snapshot_ref = model_root.join("refs").join("main");
    let snapshot = fs::read_to_string(&snapshot_ref)
        .map_err(|err| anyhow!("failed to read {}: {}", snapshot_ref.display(), err))?;
    Ok(model_root.join("snapshots").join(snapshot.trim()))
}

fn load_runtime_embedding_tokenizer_uncached() -> AnyhowResult<Tokenizer> {
    let snapshot_dir = runtime_embedding_snapshot_dir()?;
    let tokenizer_json = snapshot_dir.join("tokenizer.json");
    let config_json = snapshot_dir.join("config.json");
    let special_tokens_map_json = snapshot_dir.join("special_tokens_map.json");
    let tokenizer_config_json = snapshot_dir.join("tokenizer_config.json");

    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(&config_json)
            .map_err(|err| anyhow!("failed to read {}: {}", config_json.display(), err))?,
    )
    .map_err(|err| anyhow!("failed to parse {}: {}", config_json.display(), err))?;
    let special_tokens_map: serde_json::Value =
        serde_json::from_slice(&fs::read(&special_tokens_map_json).map_err(|err| {
            anyhow!(
                "failed to read {}: {}",
                special_tokens_map_json.display(),
                err
            )
        })?)
        .map_err(|err| {
            anyhow!(
                "failed to parse {}: {}",
                special_tokens_map_json.display(),
                err
            )
        })?;
    let tokenizer_config: serde_json::Value =
        serde_json::from_slice(&fs::read(&tokenizer_config_json).map_err(|err| {
            anyhow!(
                "failed to read {}: {}",
                tokenizer_config_json.display(),
                err
            )
        })?)
        .map_err(|err| {
            anyhow!(
                "failed to parse {}: {}",
                tokenizer_config_json.display(),
                err
            )
        })?;

    let model_max_length = tokenizer_config["model_max_length"]
        .as_f64()
        .ok_or_else(|| anyhow!("tokenizer_config.json missing model_max_length"))?
        as usize;
    let max_length = configured_embedding_max_length().min(model_max_length);
    let pad_id = config["pad_token_id"].as_u64().unwrap_or(0) as u32;
    let pad_token = tokenizer_config["pad_token"]
        .as_str()
        .ok_or_else(|| anyhow!("tokenizer_config.json missing pad_token"))?
        .to_string();

    let mut tokenizer = Tokenizer::from_file(&tokenizer_json)
        .map_err(|err| anyhow!("{}: {}", tokenizer_json.display(), err))?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        pad_token,
        pad_id,
        ..Default::default()
    }));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length,
            ..Default::default()
        }))
        .map_err(|err| anyhow!("failed to configure tokenizer truncation: {}", err))?;

    if let serde_json::Value::Object(root_object) = special_tokens_map {
        for value in root_object.values() {
            if let Some(content) = value.as_str() {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    special: true,
                    ..Default::default()
                }]);
            } else if let (
                Some(content),
                Some(single_word),
                Some(lstrip),
                Some(rstrip),
                Some(normalized),
            ) = (
                value.get("content").and_then(|v| v.as_str()),
                value.get("single_word").and_then(|v| v.as_bool()),
                value.get("lstrip").and_then(|v| v.as_bool()),
                value.get("rstrip").and_then(|v| v.as_bool()),
                value.get("normalized").and_then(|v| v.as_bool()),
            ) {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    single_word,
                    lstrip,
                    rstrip,
                    normalized,
                    special: true,
                    ..Default::default()
                }]);
            }
        }
    }

    Ok(tokenizer)
}

pub fn load_runtime_embedding_tokenizer() -> AnyhowResult<Arc<Tokenizer>> {
    static TOKENIZER: OnceLock<Result<Arc<Tokenizer>, String>> = OnceLock::new();
    match TOKENIZER.get_or_init(|| {
        load_runtime_embedding_tokenizer_uncached()
            .map(Arc::new)
            .map_err(|err| err.to_string())
    }) {
        Ok(tokenizer) => Ok(Arc::clone(tokenizer)),
        Err(err) => Err(anyhow!(err.clone())),
    }
}

pub fn token_count_for_text(text: &str) -> AnyhowResult<usize> {
    let tokenizer = load_runtime_embedding_tokenizer()?;
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|err| anyhow!("failed to encode chunk text for token counting: {}", err))?;
    Ok(encoding.len())
}

/// Exact content-token count for a *fragment* — special tokens EXCLUDED.
///
/// Used by the chunker's Knuth-Plass DP to cost individual body lines (and
/// the fixed per-chunk prefix). Special tokens ([CLS]/[SEP]) are added once
/// per emitted chunk, not per line, so counting them per fragment would
/// over-estimate every line by a constant and distort the segmentation.
/// Per-line / prefix texts are far below the 512-token model window, so the
/// tokenizer's truncation never bites here.
///
/// On tokenizer-load/encode error, falls back to a conservative char/3
/// heuristic (BGE averages ~3-4 chars/token on source code) so the chunker
/// stays linear and correct even without the model cache.
pub fn content_token_count(text: &str) -> usize {
    // REQ-AXO-901895 — per-line tokenizer memoization (the chunker throughput
    // lever). The Knuth-Plass DP costs every body line via this function, and
    // `build_symbol_chunks` runs per symbol — so a file's lines are re-encoded
    // once per ENCLOSING symbol (nested fns/classes/macros share spans),
    // making the HuggingFace `encode` the dominant per-file cost (~1.5 files/s
    // pre-fix). Source lines repeat heavily within a file (and common lines —
    // braces, imports — across files), so a thread-local memo collapses the
    // O(symbols × lines) encodes to O(unique lines). Deterministic: the cache
    // returns the exact same count, only the recompute is skipped.
    //
    // Bypass for long fragments: giant minified lines (which the chunker
    // char-windows) are one-shot and would just bloat the cache, so only short
    // fragments (body lines, per-chunk prefixes) are memoized.
    if text.len() > TOKEN_COUNT_CACHE_MAX_KEY_BYTES {
        return content_token_count_uncached(text);
    }
    if let Some(hit) = TOKEN_COUNT_CACHE.with(|c| c.borrow().get(text).copied()) {
        return hit;
    }
    let count = content_token_count_uncached(text);
    TOKEN_COUNT_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        // Simple bound: code lines repeat heavily so a periodic clear refills
        // cheaply and keeps per-thread memory flat (no LRU dependency).
        if cache.len() >= TOKEN_COUNT_CACHE_MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(text.to_owned(), count);
    });
    count
}

fn content_token_count_uncached(text: &str) -> usize {
    match load_runtime_embedding_tokenizer() {
        Ok(tokenizer) => match tokenizer.encode(text, false) {
            Ok(encoding) => encoding.len(),
            Err(_) => text.chars().count().div_ceil(3).max(1),
        },
        Err(_) => text.chars().count().div_ceil(3).max(1),
    }
}

/// REQ-AXO-901895 — max fragment length (bytes) memoized by
/// [`content_token_count`]. Longer fragments bypass the cache (one-shot giant
/// lines would only bloat it). Body lines + per-chunk prefixes sit well under.
const TOKEN_COUNT_CACHE_MAX_KEY_BYTES: usize = 2048;
/// REQ-AXO-901895 — per-thread memo entry cap; cleared wholesale on overflow
/// (code-line repetition makes refill cheap, keeps memory flat without an LRU).
const TOKEN_COUNT_CACHE_MAX_ENTRIES: usize = 200_000;

thread_local! {
    static TOKEN_COUNT_CACHE: std::cell::RefCell<std::collections::HashMap<String, usize>> =
        std::cell::RefCell::new(std::collections::HashMap::with_capacity(4096));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-AXO-901895 — the per-line memo must return the EXACT same count as
    /// the uncached encode (determinism), and a repeat call must hit the cache.
    /// Holds whether or not the tokenizer model is present (both paths share the
    /// same `_uncached` fallback), so it is hermetic.
    #[test]
    fn content_token_count_cache_matches_uncached() {
        for s in [
            "fn main() {}",
            "    let x = 1;",
            "}",
            "use std::collections::HashMap;",
        ] {
            let first = content_token_count(s);
            let second = content_token_count(s); // cache hit
            let uncached = content_token_count_uncached(s);
            assert_eq!(first, second, "repeat call must be stable for {s:?}");
            assert_eq!(first, uncached, "cache must equal the uncached count for {s:?}");
        }
    }

    #[test]
    fn runtime_chunk_profile_defaults_from_model_cap() {
        unsafe {
            std::env::remove_var("AXON_EMBED_MAX_LENGTH");
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_CHUNK_OVERLAP_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
        let profile = runtime_chunk_profile();
        assert_eq!(profile.model_max_tokens, MAX_LENGTH);
        assert_eq!(profile.target_chunk_tokens, 384);
        assert!(profile.overlap_tokens > 0);
        assert!(profile.small_symbol_char_fast_path < profile.gray_zone_char_threshold);
    }
}
