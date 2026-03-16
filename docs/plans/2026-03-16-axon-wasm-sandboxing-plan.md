# Plan d'Architecture v3.0 : Sandboxing WASM pour Tree-sitter (Style "Tracker 3")

## 🎯 L'Objectif
Atteindre le niveau de fiabilité et d'isolation de *Tracker 3* en isolant chaque opération de parsing sémantique dans une machine virtuelle **WebAssembly (WASM)**.
Actuellement, les grammaires Tree-sitter sont compilées en bibliothèques C natives et liées dynamiquement au processus Rust. Si un fichier malicieux ou corrompu déclenche un Segfault (dépassement de tampon) dans le code C généré par Tree-sitter, c'est tout le démon `axon-core` (Data Plane) qui crashe.

Le passage à WASM garantit :
1. **Zéro Crash (Sandboxing) :** Une erreur mémoire dans une grammaire WASM lève un `Trap` récupérable par Rust. Le fichier est ignoré, le système continue sans perte de données.
2. **Performance :** L'instanciation d'un module WASM coûte quelques microsecondes, évitant le gouffre de latence de la création de mini-processus OS (Erlang Ports/System.cmd).
3. **Découplage :** Possibilité de mettre à jour une grammaire (ex: ajouter le support PHP) en glissant un simple fichier `.wasm` sans recompiler tout `axon-core`.

---

## 🛠️ État des Lieux & Opportunité Technique
L'écosystème Rust de `tree-sitter` (que nous utilisons déjà en version `0.23`) **supporte nativement WASM** via la feature `wasm` (basée sur le moteur *Wasmtime*). 
Nous n'avons pas besoin de réinventer la roue, mais de modifier notre pipeline d'ingestion.

## 🚀 Plan d'Implémentation (Phases)

### Phase 1 : Outillage et Compilation WASM (DevEnv)
- **Objectif :** Générer les binaires `.wasm` au lieu de lier les crates C natives.
- **Tâches :**
  - [ ] Ajouter `tree-sitter-cli` dans le `devenv.nix` (avec la cible compilation WASM `emscripten` ou `wasi`).
  - [ ] Créer un script `scripts/build-wasm-parsers.sh` qui télécharge ou compile les grammaires principales (`python`, `elixir`, `rust`, etc.) en fichiers `.wasm` (ex: `tree-sitter-python.wasm`).
  - [ ] Placer ces artefacts dans un dossier dédié (`src/axon-core/parsers/`).

### Phase 2 : PoC et Adaptation du Moteur Rust (Data Plane)
- **Objectif :** Activer la feature WASM et charger une grammaire dynamiquement.
- **Tâches :**
  - [ ] Modifier `src/axon-core/Cargo.toml` : Activer la feature `wasm` sur la dépendance `tree-sitter` et ajouter `wasmtime`.
  - [ ] Modifier `src/axon-core/src/parser/mod.rs` : Créer un moteur WASM partagé (`wasmtime::Engine`).
  - [ ] Implémenter le chargement d'un langage via `Language::from_wasm_file("parsers/tree-sitter-python.wasm")`.
  - [ ] Écrire un test unitaire prouvant qu'un parsing WASM extrait exactement les mêmes symboles que l'ancien parseur natif.

### Phase 3 : Sandboxing Global & Gestion des Erreurs (Traps)
- **Objectif :** Isoler le parsing de chaque fichier et récupérer les plantages.
- **Tâches :**
  - [ ] Refactoriser l'usine à parseurs `get_parser_for_file` pour utiliser l'instance WASM.
  - [ ] Encapsuler l'appel `parser.parse(...)` dans un bloc de gestion d'erreur robuste capable d'attraper les `Traps` WASM (les fautes de segmentation virtualisées).
  - [ ] Si un `Trap` survient, émettre un message `Telemetry` vers le Control Plane Elixir : `[WARN] Fichier lib/bad.py ignoré (WASM Trap : Segfault)`.

### Phase 4 : Nettoyage et Coupe des Liens Natifs
- **Objectif :** Purger la base de code de la dette technique C-FFI.
- **Tâches :**
  - [ ] Supprimer toutes les dépendances natives (`tree-sitter-python`, `tree-sitter-elixir`, `cc`, `cmake`, `cxx-build`) du `Cargo.toml`.
  - [ ] Supprimer les imports `unsafe { tree_sitter_python() }` de tous les fichiers du dossier `src/axon-core/src/parser/*.rs`.
  - [ ] Vérifier que le temps de compilation (Rust `cargo build`) chute drastiquement, car nous ne compilons plus des dizaines de mégaoctets de code C.

---

## 📈 Critères de Succès (Definition of Done)
*   **Crash Test :** Un fichier C ou Python volontairement formaté pour déclencher un dépassement de tampon dans l'AST ne fait pas crasher le processus `axon-core`.
*   **Temps de build :** Le temps de compilation du backend Rust passe de ~2 minutes (compilation C croisée) à < 10 secondes.
*   **Silence maintenu :** Les performances I/O et CPU restent strictement bridées à 40% (conformément au patch précédent) car l'overhead WASM est négligeable (0.5% max de pénalité).