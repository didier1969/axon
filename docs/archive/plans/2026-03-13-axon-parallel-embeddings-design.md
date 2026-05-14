# Axon - Parallel Embeddings Design (FastEmbed Rust)

## Context
Afin d'atteindre notre cible de "< 10min pour 40k symboles" lors de l'indexation massive, nous migrons la génération vectorielle de l'ancien esclave Python vers notre moteur natif Rust (`axon-core`).

## Objectif
Implémenter la vectorisation des symboles en utilisant la crate Rust `fastembed` en profitant du batching intra-fichier pour un rendement maximal avec l'accélération ONNX (Option 3).

## Architecture & Approche

### 1. Intégration Modèle
- Ajout de la crate `fastembed` à `axon-core/Cargo.toml`.
- On instanciera le modèle `TextEmbedding` de manière paresseuse (lazy/singleton ou partagé via `Arc`) au démarrage de `axon-core` pour éviter les coûts de rechargement.

### 2. Stratégie de Batching (Intra-fichier)
Au lieu d'invoquer le modèle symbole par symbole, chaque analyseur (ex. Python, Rust, Elixir) produira sa liste de `Symbol`.
Ensuite, l'Orchestrateur interne (ex. `bridge.rs` ou `main.rs`) :
1. Collectera les contenus textuels (nom + docstring/définition) de tous les symboles d'un fichier donné.
2. Invoquera `model.embed(texts, batch_size)` en une seule passe. L'accélération SIMD/ONNX sous-jacente de `fastembed-rs` prendra le relais pour utiliser tous les cœurs de manière optimale.

### 3. Modèle de Données & Stockage
- Modification de la structure `Symbol` (dans `src/axon-core/src/parser/mod.rs`) pour ajouter un champ optionnel `embedding: Option<Vec<f32>>`.
- L'outil de persistence (`GraphStore` dans `src/axon-core/src/graph.rs`) n'insèrera pas les embeddings dans la table de noeuds Kuzu de base s'ils ne sont pas requis, ou nous préparerons le terrain pour l'envoi au moteur Vectoriel de HydraDB. *(Note : Dans un premier temps, nous validerons la génération et le stockage local).*

## Modifications Prévues
1. `src/axon-core/Cargo.toml` : Add `fastembed` and `once_cell` (for lazy static initialization if needed).
2. `src/axon-core/src/parser/mod.rs` : Update `Symbol` struct to hold `embedding: Option<Vec<f32>>`.
3. `src/axon-core/src/embedder.rs` (New) : Wrapper thread-safe for FastEmbed exposing a `batch_embed` function.
4. `src/axon-core/src/main.rs` : Connect the parsing output to the embedder before pushing to the graph.