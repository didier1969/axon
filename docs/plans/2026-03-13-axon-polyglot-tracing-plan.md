# Polyglot Tracing Implementation Plan (Elixir/Rust NIFs)

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implémenter le pont sémantique entre les langages (Traçage Polyglotte) en se concentrant sur la relation Elixir ↔ Rust NIFs. En étendant la logique locale des parseurs (approche 1), nous permettrons au graphe HydraDB de remonter les dépendances nativement entre les modules OTP et les bibliothèques C-FFI.

**Architecture:** 
1. Le parseur Rust ajoute la propriété `is_nif="true"` aux fonctions décorées par `#[rustler::nif]`.
2. Le parseur Elixir crée une relation sémantique vers la fonction cible lorsqu'il identifie le marqueur d'erreur d'Erlang `:erlang.nif_error(:nif_not_loaded)`.

**Tech Stack:** Rust (Tree-sitter extractors).

---

### Task 1: Update Rust Parser for NIF Detection

**Files:**
- Modify: `src/axon-core/src/parser/rust.rs`

**Step 1: Write the failing test**
Dans `src/axon-core/src/parser/rust.rs`, ajouter le test unitaire suivant :
```rust
    #[test]
    fn test_parse_rustler_nif() {
        let code = r#"
            #[rustler::nif]
            pub fn compute_hash(data: String) -> String {
                "hash".to_string()
            }
        "#;
        let parser = RustParser::new();
        let result = parser.parse(code);

        let nif_sym = result.symbols.iter().find(|s| s.name == "compute_hash").unwrap();
        assert_eq!(nif_sym.properties.get("is_nif").map(|s| s.as_str()), Some("true"));
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test parser::rust::tests::test_parse_rustler_nif`
Expected: FAIL (property doesn't exist).

**Step 3: Write minimal implementation**
Dans `rust.rs`, modifier `extract_function` pour inspecter les attributs. Chercher un attribut qui contient la chaîne "rustler::nif".
Si trouvé, ajouter `"is_nif": "true"` à `properties`.

```rust
    fn extract_function<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, _class_name: &str) {
        if let Some(name_node) = self.find_child_by_type(node, "identifier") {
            if let Ok(name) = name_node.utf8_text(source) {
                let start_line = node.start_position().row + 1;
                let end_line = node.end_position().row + 1;
                let is_entry = self.has_visibility(node);
                
                let mut properties = std::collections::HashMap::new();
                
                // NIF Detection (Polyglot Tracing)
                // In tree-sitter-rust, attributes are usually children of the function_item or attached nearby.
                // An easy approximation without a deep cursor search is string scanning the source snippet, 
                // but let's do it cleanly by checking the function_item children.
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "attribute_item" {
                        if let Ok(attr_text) = child.utf8_text(source) {
                            if attr_text.contains("rustler::nif") {
                                properties.insert("is_nif".to_string(), "true".to_string());
                            }
                        }
                    }
                }

                result.symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "function".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: is_entry,
                    properties,
                    embedding: None,
                });
            }
        }
    }
```

**Step 4: Run test to verify it passes**
Run: `cd src/axon-core && cargo test parser::rust::tests::test_parse_rustler_nif`
Expected: PASS.

**Step 5: Commit**
```bash
git add src/axon-core/src/parser/rust.rs
git commit -m "feat(parser): rust extractor detects and tags rustler NIF macros"
```

---

### Task 2: Update Elixir Parser for NIF Resolution

**Files:**
- Modify: `src/axon-core/src/parser/elixir.rs`

**Step 1: Write the failing test**
Ajouter dans `elixir.rs` :
```rust
    #[test]
    fn test_elixir_nif_resolution() {
        let code = r#"
            defmodule Axon.Scanner do
              use Rustler, otp_app: :axon_watcher, crate: "axon_scanner"
              def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
            end
        "#;
        let mut parser = ElixirParser::new();
        let result = parser.parse(code);

        // It should have created a relation connecting the Elixir wrapper to the underlying Rust NIF name
        let has_nif_call = result.relations.iter().any(|r| {
            r.rel_type == "calls_nif" && r.to == "scan"
        });
        
        assert!(has_nif_call, "Elixir NIF resolution failed");
    }
```

**Step 2: Run test to verify it fails**
Run: `cd src/axon-core && cargo test parser::elixir::tests::test_elixir_nif_resolution`

**Step 3: Write minimal implementation**
Dans `elixir.rs`, la fonction `extract_def_name` ou le parseur global devrait identifier ce pattern. On peut faire simple : scanner le contenu de la fonction pour `:erlang.nif_error(:nif_not_loaded)`.
Dans `extract_function_def` (ou équivalent) de `elixir.rs` :
```rust
                    if let Ok(body_text) = node.utf8_text(source_bytes) {
                        if body_text.contains(":erlang.nif_error(:nif_not_loaded)") {
                            // C'est un pont vers Rust (NIF)
                            result.relations.push(Relation {
                                from: self.current_module.clone(),
                                to: name.clone(), // The name is identical to the C-FFI Rust exported function by Rustler convention
                                rel_type: "calls_nif".to_string(),
                                properties: std::collections::HashMap::new(),
                            });
                        }
                    }
```
L'ajouter là où `name` de la fonction est extrait et validé.

**Step 4: Run test to verify it passes**
Run test again. Expected: PASS.

**Step 5: Commit**
```bash
git add src/axon-core/src/parser/elixir.rs
git commit -m "feat(parser): elixir extractor bridges local definitions to external NIFs via CALLS_NIF relations"
```

---

### Task 3: Roadmap Update

**Files:**
- Modify: `ROADMAP.md`

**Step 1: Write minimal implementation**
Marquer la stratégie "Traçage Polyglotte" comme terminée.

```markdown
- [x] **Traçage Polyglotte :** Traversée automatique des frontières (ex: Elixir ↔ Rust NIFs).
```

**Step 2: Commit**
```bash
git add ROADMAP.md
git commit -m "docs: mark Polyglot Tracing phase as complete"
```