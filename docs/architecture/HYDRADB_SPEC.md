# Axon Knowledge Graph Schema (Nexus v3.2)

Ce document est la source de vérité pour la structure de KuzuDB. Toute modification doit être précédée d'une mise à jour de ce fichier.

## 1. Couche SOLL (Intention)

### Tables de Nœuds
- **Vision :** `(title STRING, description STRING, goal STRING, PRIMARY KEY (title))`
- **Pillar :** `(id STRING, title STRING, description STRING, PRIMARY KEY (id))`
- **Requirement :** `(id STRING, title STRING, description STRING, justification STRING, priority STRING, PRIMARY KEY (id))`
- **Concept :** `(name STRING, explanation STRING, rationale STRING, PRIMARY KEY (name))`
- **Registry :** `(id STRING, last_req INT64, last_cpt INT64, last_dec INT64, PRIMARY KEY (id))`

### Relations
- `(Pillar)-[:EPITOMIZES]->(Vision)`
- `(Requirement)-[:BELONGS_TO]->(Pillar)`
- `(Concept)-[:EXPLAINS]->(Requirement)`

## 2. Couche IST (Réalité Physique)

### Tables de Nœuds
- **Project :** `(name STRING, PRIMARY KEY (name))`
- **File :** `(path STRING, project_slug STRING, status STRING, size INT64, priority INT64, mtime INT64, worker_id INT64, PRIMARY KEY (path))`
- **Symbol :** `(id STRING, name STRING, kind STRING, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, embedding FLOAT[384], PRIMARY KEY (id))`

### Relations
- `(File)-[:BELONGS_TO]->(Project)`
- `(File)-[:CONTAINS]->(Symbol)`
- `(Symbol)-[:CALLS]->(Symbol)`
- `(Symbol)-[:CALLS_NIF]->(Symbol)`
- `(Project)-[:HAS_SUBPROJECT]->(Project)`

## 3. Le Fil Numérique (Digital Thread)
- `(Concept)-[:SUBSTANTIATES]->(Symbol)` : La preuve physique du concept.
