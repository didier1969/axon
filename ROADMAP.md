# Roadmap: axon

## Overview

Axon évolue d'un simple indexeur de code vers un **Copilote Architectural** (AI-powered structure engine). Notre priorité est d'offrir une compréhension profonde de n'importe quel projet à n'importe quelle échelle, en intégrant la sémantique multi-langage, le versionnage d'architecture et l'analyse de flux.

## État Actuel

**v0.9 : Couverture Langage & Intelligence (En cours)**
Status: 🚧 Phase 3 — En cours de planification
Phases complétées : 0/3

| Phase | Nom | Statut | Objectif |
|-------|------|--------|-----------|
| **3** | Parsers Part 1 (Java, C#, Ruby) | TBD | Couvrir l'entreprise et le web classique. |
| **4** | Parsers Part 2 (Kotlin, PHP, C++) | TBD | Systèmes, mobile et legacy. |
| **5** | Intelligence & DX | TBD | Améliorations MCP et intégration agent renforcée. |

---

### Phase 3 : Langages d'Entreprise
- Java (tree-sitter-java)
- C# (tree-sitter-c-sharp)
- Ruby (tree-sitter-ruby)

## Milestones Suivantes

### v0.10 : Moteur HydraDB & Copilote Architectural

Focus: Migration infrastructure, versionnage Dolt, traçage de flux, raisonnement polyglotte.

- **Backend HydraDB :** Utiliser les moteurs Rust/C++ de HydraDB pour le graphe (CozoDB) et le vecteur (HNSW).
- **Time-Travel (Dolt) :** Versionner l'index structurel. `axon diff` au niveau architectural.
- **Data Flow Tracing :** Suivre une donnée d'un point A à un point B (ex: API -> Disk).
- **Transparence Polyglotte :** Traversée automatique des frontières (ex: Elixir ↔ Rust NIFs).
- **Audit d'Alignement :** Vérification automatique de l'alignement entre Code et Documentation (GEMINI.md/Specs).

---

## Milestones Complétées

<details>
<summary>v0.8 Graph Intelligence — 2026-03-07</summary>
Centralité PageRank, Hybrid Search, axon_path, axon_find_usages, axon_lint, axon_summarize.
958 tests passants.
</details>

<details>
<summary>v0.7 Quality & Security — 2026-03-04</summary>
Sécurisation Cypher, byte offsets précis, axon_read_symbol.
</details>

<details>
<summary>v0.6 Daemon & Centralisation — 2026-03-02</summary>
Daemon central avec cache LRU, stockage ~/.axon/repos/.
</details>
