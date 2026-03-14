# Axon - Strict API Break Check (is_public AST Extraction)

## Goal
Supprimer l'heuristique (simplification) du MCP `axon_api_break_check` et de `axon_audit`. Pour cela, refactoriser le parseur central Tree-sitter de tous les langages pour extraire rigoureusement le flag de visibilité `is_public` ou `exported` depuis l'AST, le stocker dans KuzuDB, et adapter la requête Cypher pour être déterministe.

## Phases

### Phase 1: Rust Core `Symbol` Structure
- [ ] Modifier la structure `Symbol` dans `src/axon-core/src/parser/mod.rs` pour ajouter `pub is_public: bool`.
- [ ] Mettre à jour `GraphStore::init_schema` et `insert_file_data` dans `src/axon-core/src/graph.rs` pour inclure la colonne `is_public BOOLEAN` dans la table KuzuDB.

### Phase 2: Tree-sitter Parsers Update
Mettre à jour l'extraction des symboles pour extraire `is_public` :
- [ ] **Rust** (`src/axon-core/src/parser/rust.rs`) : Détecter `pub` modifier.
- [ ] **TypeScript/JS** (`src/axon-core/src/parser/typescript.rs`) : Détecter le mot-clé `export`.
- [ ] **Java** (`src/axon-core/src/parser/java.rs`) : Détecter le modifier `public`.
- [ ] **Go** (`src/axon-core/src/parser/go.rs`) : Vérifier si la première lettre du symbole est majuscule.
- [ ] **Python** (`src/axon-core/src/parser/python.rs`) : Mettre `true` par défaut, ou `false` si ça commence par `_`.
- [ ] **Elixir** (`src/axon-core/src/parser/elixir.rs`) : Détecter `def` au lieu de `defp`.
- [ ] Mettre à jour les autres parseurs (CSS, HTML, Markdown, SQL, YAML) en mettant `is_public: true` ou `false` par défaut pour s'assurer que ça compile.

### Phase 3: MCP Tool Refactor
- [ ] Modifier `axon_api_break_check` dans `src/axon-core/src/mcp.rs` pour utiliser `MATCH (s:Symbol {name: '...', is_public: true})`.
- [ ] Mettre à jour la synthèse "Macro API Break Check" dans `axon_audit` pour filtrer sur `is_public = true` au lieu de chercher les appels inter-dossiers.

### Phase 4: Quality & Tests
- [ ] Lancer `cargo check`.
- [ ] Adapter tous les tests unitaires affectés dans `mcp.rs` et `parser/*.rs` qui mockent ou valident la structure `Symbol`.
- [ ] Lancer `cargo test` et s'assurer du 100% de réussite.
- [ ] Mettre à jour `progress.md`.
