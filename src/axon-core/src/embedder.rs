use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use once_cell::sync::Lazy;
use std::sync::Mutex;

pub struct EmbedderState {
    model: TextEmbedding,
    batch_count: usize,
}

impl EmbedderState {
    fn new() -> Self {
        Self {
            model: TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::AllMiniLML6V2)
                    .with_show_download_progress(false)
            ).expect("Failed to initialize FastEmbed model"),
            batch_count: 0,
        }
    }
}

pub static EMBEDDER: Lazy<Mutex<EmbedderState>> = Lazy::new(|| {
    Mutex::new(EmbedderState::new())
});

pub fn batch_embed(texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    
    // 1. Check if we need to reset the arena (lock acquired and released quickly)
    let needs_reset = {
        let mut state = EMBEDDER.lock().unwrap();
        state.batch_count += 1;
        state.batch_count % 1000 == 0
    };

    // 2. If needed, build the new model WITHOUT holding the global lock to prevent deadlocks
    if needs_reset {
        log::info!("Re-initializing FastEmbed ONNX session to clear Arena allocator (Deadlock-Free)");
        let new_state = EmbedderState::new(); 
        // 3. Re-acquire lock to swap the pointer instantly
        let mut state = EMBEDDER.lock().unwrap();
        *state = new_state;
    }

    // 4. Acquire lock for actual embedding
    let mut embedder = EMBEDDER.lock().unwrap();
    let mut all_embeddings = Vec::with_capacity(texts.len());
    
    // Chunking to prevent ONNX memory explosions on files with thousands of symbols
    // AND Truncating strings because ONNX Arena Allocator never shrinks
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
        // all-MiniLM-L6-v2 produces 384 dimensions
        assert_eq!(embeddings[0].len(), 384);
    }
}
