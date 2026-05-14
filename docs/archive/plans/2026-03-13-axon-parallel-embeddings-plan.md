# Parallel Embeddings (FastEmbed Rust) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implémenter la vectorisation des symboles via la crate Rust `fastembed`, en tirant parti du batching par fichier et de l'accélération ONNX, pour atteindre l'objectif de <10min pour 40k symboles.

**Architecture:** Ajout du moteur d'embedding au Data Plane (Pod B/Rust). Chaque lot de symboles extraits d'un fichier sera converti en une liste de textes et envoyé à l'embedder en un seul appel. Le modèle est chargé dynamiquement et de manière thread-safe via `once_cell` / `lazy_static`.

**Tech Stack:** Rust, fastembed-rs.

---

### Task 1: Add Dependencies

**Files:**
- Modify: `src/axon-core/Cargo.toml`

**Step 1: Modify Cargo.toml**
Ajouter `fastembed` et `once_cell` aux dépendances :
```toml
fastembed = "4.4.0"
once_cell = "1.19.0"
```

**Step 2: Check Compilation**
Lancer `cd src/axon-core && cargo build` pour s'assurer que les dépendances sont résolues.

**Step 3: Commit**
```bash
git add src/axon-core/Cargo.toml
git commit -m "chore(core): add fastembed and once_cell dependencies"
```

---

### Task 2: Extend Symbol Model

**Files:**
- Modify: `src/axon-core/src/parser/mod.rs`

**Step 1: Add embedding field**
Dans la struct `Symbol`, ajouter le champ optionnel pour stocker le vecteur.
```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub docstring: Option<String>,
    pub is_entry_point: bool,
    pub properties: std::collections::HashMap<String, String>,
    pub embedding: Option<Vec<f32>>, // <- Add this
}
```

**Step 2: Fix Initialization across Parsers**
Il va falloir parcourir tous les parseurs (`elixir.rs`, `go.rs`, `rust.rs`, `python.rs`, etc.) pour ajouter `embedding: None` lors de la création d'un `Symbol`.

**Step 3: Verify and Commit**
Lancer `cd src/axon-core && cargo check` et corriger tous les appels d'instanciation de `Symbol`.
```bash
git add src/axon-core/src/parser/
git commit -m "refactor(parser): add optional embedding field to Symbol model"
```

---

### Task 3: Create the Embedder Module

**Files:**
- Create: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/main.rs` (to declare the module)

**Step 1: Write TDD test & Implementation**
Dans `src/axon-core/src/embedder.rs` :
```rust
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use once_cell::sync::Lazy;

pub static EMBEDDER: Lazy<TextEmbedding> = Lazy::new(|| {
    TextEmbedding::try_new(InitOptions {
        model_name: EmbeddingModel::AllMiniLML6V2,
        show_download_progress: false,
        ..Default::default()
    }).expect("Failed to initialize FastEmbed model")
});

pub fn batch_embed(texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    // We clone the texts to slices as required by fastembed
    let texts_ref: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let embeddings = EMBEDDER.embed(texts_ref, None)?;
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
```

Dans `src/axon-core/src/main.rs` ajouter :
```rust
mod embedder;
```

**Step 2: Test**
Lancer `cd src/axon-core && cargo test embedder::tests`.

**Step 3: Commit**
```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/main.rs
git commit -m "feat(core): implement lazy fastembed singleton for batch vectorization"
```

---

### Task 4: Connect Embedder to Parser Output

**Files:**
- Modify: `src/axon-core/src/main.rs`

**Step 1: Apply batching before storing**
Dans `tokio::task::spawn_blocking(move || { ... })`, juste après `let mut result = parser.parse(&content);`, insérer la logique :
```rust
let texts_to_embed: Vec<String> = result.symbols.iter()
    .map(|s| {
        let doc = s.docstring.as_deref().unwrap_or("");
        format!("Symbol: {} Kind: {} Doc: {}", s.name, s.kind, doc)
    })
    .collect();

if let Ok(embeddings) = crate::embedder::batch_embed(texts_to_embed) {
    for (sym, emb) in result.symbols.iter_mut().zip(embeddings.into_iter()) {
        sym.embedding = Some(emb);
    }
}
```

**Step 2: Commit**
```bash
git add src/axon-core/src/main.rs
git commit -m "feat(core): compute parallel embeddings for AST symbols before graph ingestion"
```

---

### Task 5: Roadmap Update

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Write minimal implementation**
Marquer la stratégie "Embeddings Parallélisés" comme terminée.

```markdown
- [x] **Embeddings Parallélisés :** Réduction radicale du temps d'indexation (Cible : < 10min pour 40k symboles).
```

**Step 2: Commit**
```bash
git add ROADMAP.md
git commit -m "docs: mark Parallel Embeddings phase as complete"
```