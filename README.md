# Axon : Copilote Architectural

**L'intelligence structurelle pour les agents IA et les développeurs.**

Axon transforme n'importe quelle base de code en un **graphe de connaissances**. Il ne se contente pas de chercher du texte : il comprend les appels de fonctions, les hiérarchies de types, les flux d'exécution et les couplages historiques pour offrir une vision à 360° de votre architecture.

```bash
$ axon analyze .

Walking files...               269 files found
Parsing code...                269/269
Tracing calls...               2,487 calls resolved
Analyzing types...             534 type relationships
Detecting communities...       14 clusters found
Detecting execution flows...   42 processes found
Finding dead code...           18 unreachable symbols
Analyzing git history...       31 coupled file pairs
Generating embeddings...       1,241 vectors stored

Done in 3.8s — 1,241 symbols, 5,847 edges, 14 clusters, 42 flows
```

---

## La Vision : De la Boussole au Scalpel

Un agent IA classique (ou un développeur pressé) travaille sur du texte plat. Il fait des greps, rate des appels indirects et ignore l'impact réel de ses changements. Les fenêtres de contexte sont finies.

Axon apporte la **structure** :

1.  **La Boussole (Orientation) :** Comprendre instantanément "Où suis-je ?" et "Comment ce module s'intègre-t-il dans le tout ?" via la détection de communautés et le résumé sémantique.
2.  **Le Scalpel (Précision) :** Agir avec une confiance totale. `axon_impact` vous donne le rayon d'impact exact (direct, indirect, transitif) avant même de toucher au code.
3.  **Le Cerveau (Raisonnement) :** Tracer des flux de données complexes entre deux points du système (`axon_path`) et vérifier l'alignement entre les spécifications et l'implémentation.

---

## Pourquoi Axon est différent ?

Contrairement aux outils de recherche classiques, Axon pré-calcule l'intelligence au moment de l'indexation (pipeline en 12 phases) pour que chaque appel d'outil soit instantané et complet.

*   **Intelligence Polyglotte :** Support profond de Python, TypeScript, Rust, Elixir, Go (et bientôt Java, C#, Ruby).
*   **Transparence Inter-langages :** Capacité unique à traverser les frontières (ex: un appel Elixir vers un NIF Rust).
*   **Zéro Cloud :** Tout tourne localement (parsing, graphe, embeddings). Vos données ne quittent jamais votre machine.
*   **Optimisé pour les Agents :** Conçu spécifiquement pour être exposé via le protocole **MCP** (Model Context Protocol).

---

## Installation Rapide

```bash
pip install axoniq            # 1. Installer
cd votre-projet && axon analyze .  # 2. Indexer (rapide, incrémental)
axon daemon start             # 3. Lancer le moteur de fond (cache LRU)
```

Ensuite, configurez votre agent (Claude Code, Cursor) en utilisant `axon serve --watch`.

---

## Fonctionnalités Clés

### 🔍 Recherche Hybride
Fusion de BM25 (texte exact), Vecteur (sémantique) et Fuzzy (fautes de frappe). Les résultats sont boostés par la **centralité (PageRank)** : les fonctions les plus importantes de votre architecture remontent en premier.

### 🛡️ Analyse d'Impact
Ne devinez plus ce qui va casser. Axon remonte l'arbre d'appel et les références de types pour vous donner la liste des symboles affectés par une modification, groupés par profondeur.

### 💀 Détection de Code Mort
Une analyse multi-passes qui comprend votre framework (FastAPI, Express, Phoenix) pour identifier les fonctions réellement inutilisées, en ignorant intelligemment les points d'entrée, les décorateurs et les protocoles.

### 🗺️ Détection de Communautés
Utilise l'algorithme de Leiden pour découvrir automatiquement les clusters fonctionnels de votre code. Utile pour comprendre un projet legacy sans documentation.

### 🔄 Diff Structurel
Comparez deux branches Git au niveau des symboles et des relations, pas seulement des lignes de texte.

---

## Roadmap v0.9+ : Vers le Copilote Architectural

*   **v0.9 : Couverture Langage Massive** (Java, C#, Ruby, Kotlin, PHP).
*   **v0.10 : Moteur HydraDB + Dolt**
    *   Migration vers un backend multi-modèle (SQL/Graphe/Vecteur).
    *   **Time Travel Architecture :** Versionnage de votre index architectural avec Dolt. Comparez l'architecture d'aujourd'hui avec celle d'il y a 6 mois.
    *   **Data Flow Tracing :** Suivez la donnée de l'API jusqu'au disque avec explication sémantique par étape.

---

## Licence

Propriétaire — Tous droits réservés.
Bâti avec passion par [@harshkedia177](https://github.com/harshkedia177) et l'équipe Axon.
