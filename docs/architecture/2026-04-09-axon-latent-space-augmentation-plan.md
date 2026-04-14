# Stratégie d'Augmentation Axon : Espace Latent et Gouvernance LLM (2025-2026)

**Statut :** Plan d'Architecture & Recommandations d'Implémentation
**Contexte :** Axon est une infrastructure déterministe (Couches SOLL/IST) dédiée à la manipulation, la génération et la vérification de code par des agents IA (LLMs). L'objectif est d'exploiter la vectorisation SOTA (State of the Art) non pas uniquement pour la recherche (RAG), mais comme moteur d'optimisation et de détection structurelle.

---

## Partie 1 : Synthèse de la Recherche SOTA et Conclusions

L'analyse de l'état de l'art 2025-2026 sur la vectorisation de code (AST Embeddings) et les espaces latents (Latent Space Search) démontre une convergence entre la recherche d'information et l'optimisation mathématique.

1.  **De la Syntaxe à la Sémantique (AST Embeddings) :** Le découpage du code basé sur des tokens fixes est obsolète. L'ingestion s'effectue via des parseurs AST (Tree-Sitter). Chaque nœud logique (fonction, classe) est plongé dans un espace vectoriel continu.
2.  **L'Optimisation dans le Continu (Latent Space Search) :** La projection de structures discrètes (le code) dans un espace continu (embeddings) permet d'utiliser des algorithmes d'optimisation (Bayésienne, Descente de gradient). Il est mathématiquement possible de calculer la distance entre l'intention (documentation SOLL vectorisée) et l'implémentation (code IST vectorisé).
3.  **Détection de la Dette Sémantique et des Vulnérabilités :** L'analyse des clusters vectoriels permet de détecter le couplage sémantique invisible (fichiers logiquement liés mais syntaxiquement isolés) et la dette de qualité induite par les LLMs (code redondant ou fragile formant des clusters aberrants).
4.  **Paramétrage Optimal de l'Ingestion (Performance/Temps) :**
    *   **Modèle :** `bge-base` (baseline validée) ou encodeurs spécialisés code (`Nomic-Embed-Code`, `Qwen3-Embedding-4B`).
    *   **Quantification :** Troncature MRL à 512 dimensions, stockage en `FLOAT16` ou `INT8` dans DuckDB pour maximiser le débit matériel (SIMD).
    *   **Architecture :** Hybrid GraphRAG. Les vecteurs agissent comme point d'entrée (30% de la décision), le graphe déterministe assure la résolution des dépendances exactes (50%), et les documents d'architecture fixent les invariants (20%).

---

## Partie 2 : Recommandations pour l'Augmentation d'Axon (Environnement LLM)

Puisque Axon est l'infrastructure hôte pour des agents IA, le système ne doit pas seulement "servir" du contexte aux LLMs, il doit **juger** la qualité mathématique de leurs productions.

### 2.1. Implémenter un Oracle de Qualité Latent (Quality Gate)
**Concept :** Avant d'autoriser l'intégration d'un code généré par un LLM (lors de `axon_pre_flight_check`), le système vectorise le nouveau code AST et calcule sa distance sémantique par rapport :
1.  À la documentation d'architecture cible (SOLL).
2.  Aux modules centraux du projet (Core IST).

**Règle Physique :** Si la distance cosine entre le nouveau module et la spécification dépasse un seuil delta $\Delta$, le code est rejeté pour "Dérive Architecturale" (Architectural Drift), même si les tests unitaires passent.

### 2.2. Cartographie des Anomalies Sémantiques (Outlier Detection)
**Concept :** L'espace latent DuckDB doit être scanné périodiquement par un script de clustering (ex: DBSCAN ou K-Means via scikit-learn).
**Objectif :**
*   Identifier les fonctions isolées sémantiquement (vulnérabilités potentielles ou code mort).
*   Identifier le code "idiot" généré par IA (réinvention de la roue) qui forme des micro-clusters denses loin des abstractions validées de l'entreprise.

### 2.3. Remplacement du RAG Naïf par le Multi-Hop Traversal (MCP)
**Concept :** Lors d'une requête MCP d'un agent LLM, ne jamais renvoyer une liste plate de vecteurs.
**Flux Requis :**
1.  Recherche vectorielle (DuckDB) pour identifier le nœud de départ (Entrypoint).
2.  Interrogation du graphe déterministe pour extraire les dépendances à profondeur $N=1$ ou $N=2$ (appels entrants, sortants, types impliqués).
3.  Renvoi au LLM d'un "Community Graph" consolidé.

---

## Partie 3 : Implémentations Logicielles (Modifications Requises)

### 3.1. Évolution du Schéma DuckDB pour l'Analyse Latente

Le stockage brut des vecteurs ne suffit pas pour l'analyse des distances. Il faut instrumenter la base de données.

*Implémentation recommandée dans `tests/test_db_init.py` ou le module d'init DB :*
```sql
-- Création de la table des anomalies sémantiques (Outliers)
CREATE SEQUENCE seq_semantic_outlier_id START 1;

CREATE TABLE SemanticOutlier (
    id INTEGER DEFAULT nextval('seq_semantic_outlier_id') PRIMARY KEY,
    chunk_id INTEGER REFERENCES Chunk(id),
    isolation_score FLOAT, -- Distance moyenne par rapport aux K plus proches voisins
    cluster_id INTEGER,    -- Assignation post-clustering
    detected_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Indexation HNSW optimisée pour la distance Cosine (Half-precision)
CREATE INDEX idx_chunk_embedding_hnsw 
ON ChunkEmbedding USING HNSW (embedding) 
WITH (metric = 'cosine');
```

### 3.2. Script : Détection de la Dérive Sémantique (Architectural Drift)

Ce script Python analyse l'espace latent pour mesurer l'écart entre le code (IST) et la spécification (SOLL).

*Nouveau fichier recommandé : `scripts/qualify_semantic_drift.py`*
```python
#!/usr/bin/env python3
import sys
import duckdb
import numpy as np

def calculate_drift(db_path: str):
    conn = duckdb.connect(db_path)
    
    # 1. Calcul du centre de gravité (Centroid) de la couche SOLL
    # (Les documents d'architecture, les règles, le README)
    centroid_query = """
    SELECT list_aggregate(list(embedding), 'avg') as soll_centroid
    FROM ChunkEmbedding ce
    JOIN Chunk c ON ce.chunk_id = c.id
    JOIN File f ON c.file_path = f.path
    WHERE f.path LIKE 'docs/architecture/%' OR f.path LIKE '%.paul/%'
    """
    soll_centroid = conn.execute(centroid_query).fetchone()[0]
    if not soll_centroid:
         sys.exit("Erreur : Aucun embedding SOLL trouvé.")
    
    centroid_vec = np.array(soll_centroid)
    
    # 2. Mesure de la distance de chaque module code (IST) par rapport à l'intention (SOLL)
    # Plus le score cosine distance est proche de 1, plus le code est distant sémantiquement.
    distance_query = f"""
    SELECT 
        f.path, 
        c.type,
        c.name,
        list_cosine_distance(ce.embedding, {soll_centroid}) as drift_score
    FROM ChunkEmbedding ce
    JOIN Chunk c ON ce.chunk_id = c.id
    JOIN File f ON c.file_path = f.path
    WHERE f.path LIKE 'src/%'
    ORDER BY drift_score DESC
    LIMIT 20;
    """
    
    outliers = conn.execute(distance_query).fetchall()
    
    print("=== Rapport de Dérive Architecturale (SOLL vs IST) ===")
    print("Seuil critique de distance : > 0.40\n")
    for path, type_name, name, score in outliers:
        alert = "⚠️ DANGER" if score > 0.40 else "OK"
        print(f"[{alert}] Drift: {score:.4f} | {type_name} {name} | {path}")

if __name__ == "__main__":
    db_url = "tmp/duckdb.db" # Remplacer par $AXON_SQL_URL
    calculate_drift(db_url)
```

### 3.3. Intégration au Pre-Flight Check (Quality Gate)

L'outil MCP `axon_pre_flight_check` (utilisé par l'agent IA avant chaque validation) doit être mis à jour pour bloquer les commits générant une dette de qualité IA.

*Modification logique requise dans la procédure `mcp_quality_gate.sh` ou `scripts/mcp_validate.py` :*
```bash
# Dans le cycle de validation, après les tests unitaires
echo "Execution de l'audit sémantique dans l'espace latent..."
DRIFT_OUTPUT=$(python3 scripts/qualify_semantic_drift.py)

# Vérification stricte
if echo "$DRIFT_OUTPUT" | grep -q "⚠️ DANGER"; then
    echo "ÉCHEC PRE-FLIGHT: La modification introduit une dérive architecturale sémantique inacceptable."
    echo "Veuillez refactoriser le code pour qu'il s'aligne sémantiquement avec la documentation SOLL."
    exit 1
fi
```

## Conclusion Exécutoire
L'implémentation de ces mécanismes transforme Axon d'un simple index de code en un **garant mathématique de l'intégrité architecturale**. L'agent IA n'est plus libre de contourner les directives ; son code est physiquement mesuré par sa distance vectorielle par rapport aux lois fondamentales (SOLL) du projet.