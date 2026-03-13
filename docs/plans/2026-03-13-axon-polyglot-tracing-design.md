# Polyglot Tracing Design (Elixir ↔ Rust NIFs)

## Context
Dans les architectures hybrides comme celle d'Axon, les frontières de langage (FFI) posent un problème d'observabilité. Actuellement, si une fonction Elixir appelle une fonction NIF écrite en Rust, le graphe est brisé. L'objectif est de recoller ces morceaux automatiquement.

## Approche : Analyse Statique Basée sur les "Hints" de Macro (Option 1)
Nous allons modifier nos extracteurs AST (Parseurs) pour qu'ils soient sensibles aux conventions de ponts polyglottes (en particulier `Rustler` pour l'axe Elixir/Rust). 

### 1. Extension du Parseur Rust
Le parseur Rust parcourt l'AST. Lorsqu'il rencontre une fonction précédée par la macro `#[rustler::nif]`, il marquera ce symbole avec des propriétés spéciales.
- Ajout de la propriété `is_nif: "true"`.
- Cela transforme la fonction d'un simple nœud local en un "Endpoint Public" virtuel.

### 2. Extension du Parseur Elixir
Le parseur Elixir détecte les appels à `:erlang.nif_error(:nif_not_loaded)`. C'est le marqueur idiomatique d'un point d'entrée NIF en Elixir.
- Lorsqu'il parse une telle fonction, il crée automatiquement une relation "CALLS" vers le nom de cette fonction, mais en la marquant comme un pont externe.
- La résolution exacte du nom (ex: appel de `scan` en Elixir -> recherche du NIF `scan` en Rust) se fera naturellement lors de la requête de graphe, car les noms correspondront dans la base de données Kuzu.

### 3. Exploitation dans le MCP / GraphStore
Pour tester cela, nous utiliserons l'outil `axon_impact`. Si on demande le rayon d'impact de la fonction NIF Rust `scan`, la requête remontera automatiquement jusqu'au composant Elixir qui l'appelle.

## Plan d'implémentation
1. **`src/axon-core/src/parser/rust.rs`** : Ajouter la détection de l'attribut `rustler::nif`.
2. **`src/axon-core/src/parser/elixir.rs`** : Ajouter la détection du body contenant `nif_not_loaded`.
3. **Tests TDD** : Écrire les tests unitaires pour valider l'extraction de ces propriétés spécifiques.
4. **Validation E2E** : Tester avec `axon_impact` ou une requête Cypher brute pour prouver la liaison.