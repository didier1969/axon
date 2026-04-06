# Architecture & Reproductibilité : Intégration Hermétique de DuckPGQ (SQL/PGQ)

## 1. Contexte et Motivation
Le moteur d'audit d'Axon nécessite des algorithmes de théorie des graphes avancés (Reachability, Shortest Path, Cyclic Detection) pour identifier formellement la dette architecturale (Fuites de Domaine, Expositions Unsafe).
Bien que les Requêtes SQL Récursives (CTE) soient fonctionnelles, le standard ISO SQL:2023 (SQL/PGQ) implémenté par l'extension C++ **DuckPGQ** offre une expressivité (syntaxe Cypher-like `MATCH (a)-[r]->(b)`) et des performances natives indispensables pour le passage à l'échelle sur des bases de code massives.

## 2. Le Problème de l'ABI C++ (Le Crash Pointeur Nul)
L'intégration directe de l'extension a révélé des instabilités critiques :
1.  **Indisponibilité :** L'extension `duckpgq` n'est pas distribuée pré-compilée par les serveurs officiels pour DuckDB v1.5.1 (`linux_amd64`).
2.  **Crash ABI (`std::bad_array_new_length`) :** Compiler l'extension depuis les sources et la charger dans un moteur DuckDB lié statiquement en Rust (via la feature `bundled` du crate `duckdb` v1.10501.0) provoque une corruption mémoire immédiate au chargement. Les structures C++ internes ne s'alignent pas au bit près entre le compilateur Rust interne et le compilateur système.

## 3. La Solution Architecturale : Le Pontage Dynamique (Option 3)
Pour garantir une stabilité absolue (Zéro Crash), l'architecture d'Axon abandonne le couplage statique (Bundled) de sa base de données au profit d'une **Fédération Dynamique Hermétique (Dynamic Linking)**.

### 3.1. Le Laboratoire Hermétique (`duckdb-graph`)
Un projet annexe (le Laboratoire) est responsable de la compilation C++ conjointe du moteur et de l'extension à partir des mêmes sources exactes.
Voici la procédure exacte de provisionnement et de compilation :

1.  **Clonage et Configuration HTTPS :**
    Le dépôt officiel `https://github.com/cwida/duckpgq-extension.git` est cloné dans `/home/dstadel/projects/duckdb-graph`. Le fichier `.gitmodules` est modifié pour forcer l'usage de HTTPS sur les sous-modules (`duckdb-pgq.git` et `extension-ci-tools.git`), garantissant la résilience des clones automatisés en CI/CD.
2.  **Alignement ABI (Verrouillage Git) :**
    Le sous-module `duckdb` est basculé strictement sur la branche `v1.5-variegata` (correspondant au tag v1.5.1 de notre futur environnement Rust).
3.  **Compilation (Make Release) :**
    La commande `make release GEN=ninja` est exécutée. Elle génère simultanément le moteur dynamique (`build/release/src/libduckdb.so`) et l'extension algorithmique (`build/release/extension/duckpgq/duckpgq.duckdb_extension`), garantissant qu'ils partagent la même ABI, la même libc et les mêmes flags d'optimisation.

### 3.2. Le Couplage dans Axon (Rust Data Plane)
Le crate Rust d'Axon (`src/axon-plugin-duckdb/Cargo.toml`) **ne doit plus utiliser la feature `bundled`**.
```toml
[dependencies]
duckdb = { version = "1.10501.0" }
```

### 3.3. Reproductibilité du Build (Variables d'Environnement)
Pour que Cargo et l'éditeur de liens du système (Linker) utilisent la librairie compilée par le laboratoire plutôt que la version système, les variables suivantes doivent être formellement injectées dans l'environnement de développement (`devenv.nix`) et dans le pipeline CI (`.github/workflows`) :

```bash
export DUCKDB_LIB_DIR="/home/dstadel/projects/duckdb-graph/build/release/src"
export DUCKDB_INCLUDE_DIR="/home/dstadel/projects/duckdb-graph/duckdb/src/include"
export LD_LIBRARY_PATH="$DUCKDB_LIB_DIR:$LD_LIBRARY_PATH"
```

### 3.4. Chargement Sécurisé au Runtime
L'extension n'étant pas signée par la fondation DuckDB, la connexion Rust doit l'autoriser explicitement et la charger via son chemin absolu :
```rust
let config = Config::default().allow_unsigned_extensions().unwrap_or_default();
let conn = Connection::open_with_flags("soll.db", config)?;
conn.execute("LOAD '/home/dstadel/projects/duckdb-graph/build/release/extension/duckpgq/duckpgq.duckdb_extension'", [])?;
```

## 4. Le Contrat Syntaxique Absolu (Le Tiret "-")
**C'est le point d'échec le plus critique.**
Puisque l'extension est chargée dynamiquement sur le moteur au runtime, le parser SQL natif de DuckDB ne reconnaît pas les mots-clés de l'extension (`CREATE PROPERTY GRAPH`, `MATCH`). Si une requête est envoyée telle quelle, le moteur déclenche une erreur interne (`INTERNAL Error: Attempted to dereference unique_ptr that is NULL!`).

**Règle d'Ingénierie :** Toute requête exploitant la syntaxe PGQ doit **impérativement commencer par un tiret (`-`)**. Ce caractère spécial intercepte la requête et la route vers le parser personnalisé de l'extension C++.

```sql
-- ✅ VALIDE (Routé vers le parser C++ de l'extension)
-CREATE PROPERTY GRAPH axon_graph VERTEX TABLES (Symbol) EDGE TABLES (CALLS ...);

-- ✅ VALIDE
-FROM GRAPH_TABLE (axon_graph MATCH (a:Symbol)-[c:CALLS]->+(b:Symbol) COLUMNS(a.id, b.id));

-- ❌ INVALIDE (Crash C++ du Binder)
CREATE PROPERTY GRAPH axon_graph ...
```

Ce document sert de source de vérité pour toute future réinstallation, mise à jour ou migration de l'infrastructure d'intelligence structurelle d'Axon.