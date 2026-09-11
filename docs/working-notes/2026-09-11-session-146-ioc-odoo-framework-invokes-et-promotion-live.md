# Session 146 — le câblage IoC, la promotion live et les 422 arêtes de production

**2026-09-11T14:30:00+02:00** · HEAD `2c8f4641` · promote live `v0.8.0-1865-g2c8f4641` (md5 `d0d3b54b2a67cd13805f3b39d68188ce`) · porte `GUI-AXO-1034` **VERTE**.

## 1. Ce que la session a fait

Livraison complète et déploiement en production de **`REQ-AXO-902330`** (câblage des frameworks à Inversion de Contrôle : Python Odoo/Celery/Django/Flask et vues/boutons XML).
Le projet client BKB (Odoo 19, 347 fichiers, 17 modules) souffrait de faux positifs d'orphelins dus aux appels implicites du framework non matérialisés dans le graphe IST.

Arbitrage formel sur décision opérateur :
- `REQ-AXO-902435` (macro déclarative pour unifier l'identité des 114 outils MCP) $\rightarrow$ `rejected` (surcoût de maintenance sans gain structurel immédiat).
- `REQ-AXO-902326` (verrou d'environnement pour tests parallèles) $\rightarrow$ `rejected` (faible valeur économique au regard des suites ciblées déterministes).

## 2. Modifications structurelles & validation technique

1. **IST Snapshot & Code Smells (`src/axon-core/src/ist_snapshot/`) :**
   - Variant `RelationType::FrameworkInvokes = 10` ajouté dans `snapshot.rs` avec bijection PG `"FRAMEWORK_INVOKES"`.
   - Correction de `relation_from_u8(10)` pour éviter toute retombée vers `Other`.
   - Résolution du nom terminal (`leaf`) dans `name_to_func` pour lier les identifiants textuels (boutons XML, compute fields) aux définitions de méthodes.
   - `code_smells.rs` : exemption d'orphelin pour tout symbole cible d'une arête `FrameworkInvokes` ou déclaré point d'entrée ; prise en compte dans `wiring_classify_node` et `orphan_clusters`.
   - `algorithms.rs` : parcours de découvrabilité `dead_clusters` étendu à `FrameworkInvokes`.

2. **Parser Python (`src/axon-core/src/parser/python.rs`) :**
   - Détection des décorateurs IoC (`@api.*`, `@task`, `@receiver`, `@app.route`).
   - Analyse des assignations de champs Odoo avec arguments nominatifs (`compute=`, `inverse=`, `search=`, `default=`, `selection=`) émettant des arêtes `framework_invokes`.

3. **Nouveau Parser XML (`src/axon-core/src/parser/xml.rs`) :**
   - Extraction des balises `<record>`, `<template>`, `<menuitem>` avec `is_entry_point = true`.
   - Extraction des actions `<button type="object" name="...">` et liaisons de code Python `<field name="code">`.
   - Enregistrement de l'extension `"xml"` dans `config.rs`, `parser/mod.rs` et validation de l'invariant d'admission du scanner (`le_scanner_admet_tout_ce_que_le_parser_sait_lire`).

4. **Ingestion Graphe (`src/axon-core/src/graph_ingestion.rs`) :**
   - Routage de `"framework_invokes"` et `"framework-invokes"` vers la table de relations `FRAMEWORK_INVOKES`.
   - Résolution cross-fichiers étendue pour lier les méthodes réceptrices.

## 3. Déploiement en Production & Preuve Physique (`GUI-AXO-1037`)

- **Promotion live :** Exécution de `scripts/release/promote_live_safe.sh --project AXO` (Exit code 0).
  - Portes de qualification : `qualify_mcp` (`verdict=ok`), `qualify_indexer_truth` (`ratio=14.83`), `apply_ddl_live` et `cutover_finalize`.
- **Preuve physique :** `strings bin/axon-brain | grep -c "2c8f4641"` = `1` sur binaire `bin/axon-brain` (md5 `d0d3b54b2a67cd13805f3b39d68188ce`).
- **Drainage & Base Live :**
  - **422 arêtes `FRAMEWORK_INVOKES`** réelles insérées en base PostgreSQL :
    - `BKB` : 140 arêtes (méthodes `_compute_wht_amount`, `_compute_wht_cert_data`, etc.).
    - `BKS` : 144 arêtes.
    - `ODM` : 104 arêtes.
    - `APS` : 34 arêtes.
  - **387 550 arêtes `CONTAINS`** actives.
  - BKB re-dépouillé : neutralisation effective des faux positifs d'orphelins.

## 4. Communication & Clôture Gouvernance

- **Fan-out clients :** Broadcast émis vers 66 projets clients via `mcp_outbox_send(to_project="*")` détaillant les fonctionnalités v0.8.1 et ouvrant la collecte de retours.
- **Capitalisation :** Pratique durable `id=2388` enregistrée via `practice_put`.
- **SOLL :** 6 preuves formelles attachées à `REQ-AXO-902330` (`delivered`), `re_anchor` et mise à jour de `CPT-AXO-052` effectuée.
- **Dette technique (`debt_digest`) :** 0 stub, 2315 paires SIMILAR_TO (clones de parsers), 94 validations SOLL ouvertes de fond, 116 symboles découplés répertoriés.
