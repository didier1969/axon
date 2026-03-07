# Contrat de Données Axon v1.0 ↔ Astral DB

## 1. Mapping des Types (Axon Python → Astral Elixir)

| Concept Axon (Python) | Node Label (Astral) | Edge Type (Astral) | Propriétés (JSON) |
|-----------------------|---------------------|--------------------|-------------------|
| `SymbolInfo` | `Node: Symbol` | - | `name`, `kind`, `file_path`, `start_line`, `end_line`, `content` |
| `FileEntry` | `Node: File` | - | `path`, `hash`, `language`, `size` |
| `RelType.CALLS` | - | `Edge: CALLS` | `count` (Optionnel) |
| `RelType.IMPORTS` | - | `Edge: IMPORTS` | `alias` (Optionnel) |
| `RelType.HERITAGE` | - | `Edge: HERITAGE` | - |
| `RelType.TYPE_REF` | - | `Edge: TYPE_REF` | - |

## 2. Ingestion Batch (The Bridge Protocol)

Pour atteindre les **900k ops/s** ciblés, Axon doit pousser des batches de taille fixe :

```elixir
# Format Elixir attendu pour Astral.Core.Graph.API.add_nodes_batch/2
[
  {"symbol_123", %{"name" => "my_func", "kind" => "function", ...}},
  {"file_abc", %{"path" => "src/main.py", ...}}
]
```

## 3. Communication : MsgPack over ErlPort

*   **Python (Producer) :** Sérialise les objets avec `msgpack`.
*   **Elixir (Consumer) :** Reçoit le binaire via `ErlPort` et le désérialise nativement.

## 4. Garanties de Conformité (v1.0 Compliance)
1.  **Atomicité :** Un batch de fichiers est soit entièrement intégré au graphe Astral, soit rejeté.
2.  **Idempotence :** Pousser deux fois le même symbole (même hash) ne doit pas créer de doublon dans Astral.
3.  **HNSW Sync :** La création d'un nœud `Symbol` dans le graphe déclenche automatiquement (ou via batch) son indexation dans `Astral.Core.Graph.VectorNIF`.
