# Gouvernance du Stockage PostgreSQL Axon & Prévention du Bloat (REQ-AXO-902611)

## 1. Contexte Médico-Légal de l'Incident du 2026-09-03

Lors de la qualification du 2026-09-03, Nexus a rejeté l'exécution avec le verdict `disk_below_reserve`. L'hôte ne disposait plus que de ~3,9 GiB d'espace libre sur la partition de données.
L'analyse forensique a établi que la base de données `axon_live` occupait ~29 GiB (mesurée à ~30 GiB lors de l'audit), répartis de façon asymétrique entre données brutes et index de recherche.

## 2. Anatomie Structurelle du Stockage `axon_live`

L'empreinte disque de la base de données est dominée par trois composants critiques :

| Relation / Index | Type d'Objet | Empreinte Physique | Fonction Système |
|---|---|---|---|
| `ist.idx_chunk_content_tsv` | GIN Index | ~12,7 GiB | Recherche plein texte lexicale (FTS `tsvector`). |
| `ist.chunk_embedding_hnsw_idx` | HNSW Index (pgvector) | ~5,8 GiB | Recherche sémantique par plus proches voisins (ANN cosine 1024-dim). |
| `ist.edge` (`edge_pkey`, `edge_fwd_idx`, `edge_rev_idx`) | B-Tree Indexes | ~3,3 GiB (1,1 GiB / index) | Navigation bidirectionnelle dans le graphe de dépendances de code. |
| `ist.chunk` (table heap) | Table Heap | ~1,3 GiB | Stockage des métadonnées et fragments de code indexés. |
| `pgmq.a_tsv_pending` | Table Heap + Index | ~1,7 GiB | File de messages PGMQ pour la mise à jour asynchrone des vecteurs FTS. |

## 3. Loi d'Architecture : Interdiction Formelle de Suppression d'Index à l'Aveugle

Il est formellement interdit de procéder à un `DROP INDEX` sur les index `idx_chunk_content_tsv`, `chunk_embedding_hnsw_idx` ou les index de graphe `edge_*` sous prétexte de libérer du disque pour débloquer un build ou une qualification :
1. La suppression de `idx_chunk_content_tsv` dégrade silencieusement toute requête FTS en sequential scan de 1,3 GiB, provoquant des timeouts sur l'outil MCP `query`.
2. La suppression de `chunk_embedding_hnsw_idx` invalide la recherche vectorielle sous SLA sub-seconde et impose une reconstruction GPU/CPU complète de plusieurs dizaines de minutes lors de la réindexation.
3. Les structures d'indexation font partie intégrante du contrat de service de la couche IST.

## 4. Politique de Maintenance sans Interruption & Contrôle du Bloat

Pour réduire l'empreinte physique et récupérer l'espace fragmenté sans verrouillage exclusif bloquant (`ACCESS EXCLUSIVE`) :

### A. Recyclage des Pages Mortes (Dead Tuples)
Les opérations massives d'ingestion et de réindexation produisent des tuples morts (mesurés à >240k sur `ist.edge`).
- Exécution de `VACUUM (ANALYZE)` sur les tables à fort taux d'écriture.
- Commande outillée :
  ```bash
  devenv shell -- bash scripts/governance/vacuum_bloat_audit.sh --vacuum
  ```

### B. Réindexation Concurrente sans Coupure (Zero-Downtime Reindex)
Pour éliminer le bloat structurel accumulé dans les index B-Tree et GIN sans bloquer les lectures ni les écritures :
- Utilisation stricte de la commande PostgreSQL `REINDEX TABLE CONCURRENTLY` ou `REINDEX INDEX CONCURRENTLY`.
- Commande outillée :
  ```bash
  devenv shell -- bash scripts/governance/vacuum_bloat_audit.sh --reindex-table ist.edge
  ```

### C. Réglages Autovacuum Ciblés
Les tables à renouvellement élevé (`ist.edge`, `ist.chunk`, `pgmq.a_tsv_pending`) doivent faire l'objet de seuils autovacuum agressifs pour empêcher l'accumulation de bloat :
```sql
ALTER TABLE ist.edge SET (
    autovacuum_vacuum_scale_factor = 0.05,
    autovacuum_vacuum_threshold = 1000
);
ALTER TABLE ist.chunk SET (
    autovacuum_vacuum_scale_factor = 0.05,
    autovacuum_vacuum_threshold = 1000
);
```

## 5. Métrologie et Surveillance Continue

Le script canonique `scripts/governance/vacuum_bloat_audit.sh` est l'instrument de référence pour auditer la situation de stockage :
- **Mode Diagnostic Passif :**
  ```bash
  devenv shell -- bash scripts/governance/vacuum_bloat_audit.sh --check
  ```
- **Mode Télémesure JSON :**
  ```bash
  devenv shell -- bash scripts/governance/vacuum_bloat_audit.sh --json
  ```
- **Seuils de Réserve Nexus :**
  - Seuil Warning : Espace libre < 10 GiB.
  - Seuil Critique : Espace libre < 5 GiB (déclenchement de la protection fail-closed Nexus `disk_below_reserve`).
