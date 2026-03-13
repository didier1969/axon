use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use once_cell::sync::Lazy;
use std::sync::Mutex;

pub static EMBEDDER: Lazy<Mutex<TextEmbedding>> = Lazy::new(|| {
    Mutex::new(TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::AllMiniLML6V2)
            .with_show_download_progress(false)
    ).expect("Failed to initialize FastEmbed model"))
});

pub fn batch_embed(texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    // We clone the texts to slices as required by fastembed
    let texts_ref: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let mut embedder = EMBEDDER.lock().unwrap();
    let embeddings = embedder.embed(texts_ref, None)?;
    Ok(embeddings)
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
