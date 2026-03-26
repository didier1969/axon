use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use once_cell::sync::Lazy;
use std::sync::Mutex;

pub struct EmbedderState {
    model: TextEmbedding,
    batch_count: usize,
}

impl EmbedderState {
    fn try_new() -> anyhow::Result<Self> {
        Ok(Self {
            model: TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                    .with_show_download_progress(false)
            ).map_err(|e| anyhow::anyhow!("Failed to initialize FastEmbed model: {}", e))?,
            batch_count: 0,
        })
    }
}

pub static EMBEDDER: Lazy<Mutex<Option<EmbedderState>>> = Lazy::new(|| {
    Mutex::new(EmbedderState::try_new().ok())
});

pub fn batch_embed(texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let mut lock = match EMBEDDER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if lock.is_none() {
        *lock = EmbedderState::try_new().ok();
    }

    let embedder = lock.as_mut().ok_or_else(|| anyhow::anyhow!("AI Embedder unavailable"))?;
    embedder.batch_count += 1;

    if embedder.batch_count % 1000 == 0 {
        log::info!("Re-initializing FastEmbed ONNX session to clear Arena allocator...");
        if let Ok(new_state) = EmbedderState::try_new() {
            *embedder = new_state;
        }
    }

    let mut all_embeddings = Vec::with_capacity(texts.len());
    
    for chunk in texts.chunks(64) {
        let mut truncated_chunk = Vec::new();
        for s in chunk {
            if s.len() > 1000 {
                truncated_chunk.push(s[..1000].to_string());
            } else {
                truncated_chunk.push(s.to_string());
            }
        }
        
        let texts_ref: Vec<&str> = truncated_chunk.iter().map(|s| s.as_str()).collect();
        let chunk_embeddings = embedder.model.embed(texts_ref, None)?;
        all_embeddings.extend(chunk_embeddings);
    }
    
    Ok(all_embeddings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_embed() {
        let texts = vec!["Hello world".to_string(), "Axon is great".to_string()];
        let embeddings = batch_embed(texts).unwrap();
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].len(), 384);
    }
}