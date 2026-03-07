# Axon : Copilote Architectural

**L'intelligence structurelle pour les agents IA et les développeurs.**

Axon transforme n'importe quelle base de code en un **graphe de connaissances**. Il ne se contente pas de chercher du texte : il comprend les appels de fonctions, les hiérarchies de types, les flux d'exécution et les couplages historiques pour offrir une vision à 360° de votre architecture.

---

## 🚀 Installation & Commandes Rapides (Docker-style UX)

```bash
uv tool install --editable /path/to/axon  # 1. Installation globale
```

Désormais, utilisez Axon avec des commandes ultra-simplifiées :

| Commande | Action | Description |
|:---|:---|:---|
| `axon start` | **Daemon Start** | Lance le moteur en arrière-plan pour des requêtes instantanées. |
| `axon up` | **Deep Index** | Ré-indexation experte complète du projet actuel. |
| `axon check` | **Expert Audit** | Lance le système immunitaire architectural (Audit OWASP). |
| `axon stop` | **Daemon Stop** | Arrête le moteur de fond. |

---

## 🛡️ Bouclier Architectural (OWASP Expert)

Axon intègre nativement un moteur d'audit de sécurité structurel. Contrairement aux scanners classiques, il utilise le graphe pour tracer le **chemin d'exposition** réel d'une faille :

- **OWASP A01 (Access Control)** : Détection d'opérations sensibles (delete, update) sans garde d'authentification.
- **OWASP A03 (Injection)** : Suivi des données depuis un point d'entrée public jusqu'à un "Sink" dangereux (SQL, Eval, System).
- **OWASP A07 (Auth Gaps)** : Identification des points d'entrée (Routes API) totalement déconnectés de vos modules de sécurité.

Chaque alerte d'audit inclut désormais un **conseil de remédiation** concret.

---

## 🧪 Analyse de Flux de Données (Taint Analysis)

Tracez la propagation d'une variable à travers les fichiers et les langages (Elixir, Rust, Python, TS, etc.) :
```bash
axon trace <nom_fonction> <nom_variable>
```
Axon vous affichera un arbre visuel indiquant exactement comment votre donnée transite dans l'architecture.

---

## Pourquoi Axon est différent ?

*   **Intelligence Polyglotte Experte :** Analyse profonde de 12 langages (Python, TS, Rust, Elixir, Go, SQL, HTML, CSS, Java, YAML, Markdown).
*   **Transparence Inter-langages :** Capacité unique à traverser les frontières (ex: un appel Elixir vers un NIF Rust).
*   **Zéro Cloud :** Tout tourne localement. Vos données ne quittent jamais votre machine.
*   **Pleine Conscience du Projet :** Indexe systématiquement les documents stratégiques (`GEMINI.md`, `.paul/`, etc.) pour relier l'intention au code.

---

## Licence

Propriétaire — Tous droits réservés.
Bâti avec passion par l'équipe Axon.
