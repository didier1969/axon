# Axon - TypeQL & Datalog Integrations (Python Strategy)

## Goal
Permettre à Axon d'être l'unique porte d'entrée (Gateway MCP) en intégrant le support de Datalog et TypeQL. Puisque les crates Rust sont défaillants ou inadaptés, nous allons utiliser l'écosystème Python (qui possède des drivers/parseurs stables pour TypeDB et Datalog) via un appel système depuis le parseur Rust.

## Phases

### Phase 1: Python Parsing Micro-Service
- [ ] Créer le fichier `src/axon-core/src/parser/python_bridge/typeql_parser.py` qui utilise `typeql` ou `typedb-driver` en Python pour parser une ontologie et renvoyer un JSON compatible avec `ExtractionResult`.
- [ ] Créer `src/axon-core/src/parser/python_bridge/datalog_parser.py` qui parse Datalog (via expressions régulières ou librairie) et renvoie le JSON.

### Phase 2: Rust Adapter
- [ ] Créer `src/axon-core/src/parser/typeql.rs`.
- [ ] Créer `src/axon-core/src/parser/datalog.rs`.
- [ ] Ces deux parseurs Rust n'analyseront pas eux-mêmes le code, mais invoqueront `std::process::Command` pour exécuter le script Python correspondant et désérialiseront le résultat JSON en `ExtractionResult`.

### Phase 3: Integration
- [ ] Ajouter les extensions `.tql`, `.typeql`, `.dl`, `.datalog` dans `mod.rs` et `main.rs`.
- [ ] Écrire un test unitaire qui valide que le pont Python fonctionne et extrait bien les entités et relations.
