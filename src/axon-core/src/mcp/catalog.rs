use serde_json::{json, Value};

pub(crate) fn tools_catalog() -> Value {
    json!({
        "tools": [
            {
                "name": "axon_refine_lattice",
                "description": "[SYSTEM] Lattice Refiner: Analyse le graphe post-ingestion pour lier les frontières inter-langages (ex: Elixir NIF -> Rust natif).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "axon_fs_read",
                "description": "[DX] Agent DX L2 (Detail) : Lit le contenu physique complet d'un fichier source. À n'utiliser qu'après avoir identifié une URI (chemin) précise via axon_query ou axon_inspect.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Le chemin complet vers le fichier (ex: 'src/main.rs')" },
                        "start_line": { "type": "integer", "description": "Ligne de début optionnelle" },
                        "end_line": { "type": "integer", "description": "Ligne de fin optionnelle" }
                    },
                    "required": ["uri"]
                }
            },
            {
                "name": "axon_soll_manager",
                "description": "[SOLL] Centre de commande pour le graphe intentionnel. Gère la création (avec IDs auto), la mise à jour et les liaisons hiérarchiques.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["create", "update", "link"], "description": "L'opération à effectuer." },
                        "entity": { "type": "string", "enum": ["pillar", "requirement", "concept", "milestone", "decision", "stakeholder", "validation"], "description": "Le type d'objet concerné." },
                        "data": {
                            "type": "object",
                            "description": "Données JSON. \n- create (pillar: title, desc; requirement: title, desc, priority; concept: name, explanation, rationale; decision: title, context, rationale, status; milestone: title, status; stakeholder: name, role; validation: method, result).\n- update (id, status/desc/etc).\n- link (source_id, target_id)."
                        }
                    },
                    "required": ["action", "entity", "data"]
                }
            },
            {
                "name": "axon_export_soll",
                "description": "[SOLL] Exporte l'intégralité du graphe intentionnel (Vision, Pillars, Milestones, Requirements, Decisions, Concepts) dans un document Markdown horodaté.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "axon_restore_soll",
                "description": "[SOLL] Restaure les entites conceptuelles depuis un export Markdown officiel SOLL. Fonctionne en mode merge, sans purge destructive implicite.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Chemin optionnel vers un export SOLL. Par defaut: dernier fichier docs/vision/SOLL_EXPORT_*.md." }
                    },
                    "required": []
                }
            },
            {
                "name": "axon_validate_soll",
                "description": "[SOLL] Exécute des garde-fous minimaux de cohérence sur le graphe intentionnel. Validation en lecture seule: détecte les états orphelins évidents sans modifier SOLL.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "axon_query",
                "description": "[DX] Recherche de symboles à forte valeur développeur. Utilise la recherche structurelle immédiatement, et ajoute la similarité sémantique seulement si l'embedding temps réel est disponible.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "project": { "type": "string" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "axon_inspect",
                "description": "[DX] Vue 360° d'un symbole (code source, appelants/appelés, statistiques).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" },
                        "project": { "type": "string" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_audit",
                "description": "[GOVERNANCE] Vérification de conformité (Sécurité OWASP, Qualité, Anti-patterns, Dette Technique).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" }
                    },
                    "required": []
                }
            },
            {
                "name": "axon_impact",
                "description": "[RISK] Analyse prédictive (Rayon d'impact et chemins critiques).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "depth": { "type": "integer" },
                        "symbol": { "type": "string" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_health",
                "description": "[GOVERNANCE] Rapport de santé global (Code mort, lacunes de tests, points d'entrée).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" }
                    },
                    "required": []
                }
            },
            {
                "name": "axon_diff",
                "description": "[RISK] Analyse sémantique des changements (Git Diff -> Symboles touchés).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_content": { "type": "string" }
                    },
                    "required": ["diff_content"]
                }
            },
            {
                "name": "axon_batch",
                "description": "[SYSTEM] Orchestration d'appels multiples pour optimiser la performance.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "calls": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "tool": { "type": "string" },
                                    "args": { "type": "object", "additionalProperties": true }
                                },
                                "required": ["tool", "args"]
                            }
                        }
                    },
                    "required": ["calls"]
                }
            },
            {
                "name": "axon_semantic_clones",
                "description": "[GOVERNANCE] Trouve des fonctions sémantiquement similaires (clones de logique) dans le projet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Nom du symbole source" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_architectural_drift",
                "description": "[GOVERNANCE] Vérifie les violations d'architecture entre deux couches (ex: 'ui' appelant directement 'db').",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_layer": { "type": "string", "description": "Couche source (ex: 'ui', 'frontend')" },
                        "target_layer": { "type": "string", "description": "Couche interdite (ex: 'db', 'repository')" }
                    },
                    "required": ["source_layer", "target_layer"]
                }
            },
            {
                "name": "axon_bidi_trace",
                "description": "[DX] Trace bidirectionnelle: remonte aux Entry Points (haut) et liste les appels profonds (bas).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Symbole de départ" },
                        "depth": { "type": "integer", "description": "Profondeur maximale (défaut: sans limite pour être exhaustif, mais cappé par le moteur)" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_api_break_check",
                "description": "[RISK] Vérifie si la modification d'un symbole public impacte des composants externes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_simulate_mutation",
                "description": "[RISK] Dry-run : calcule le volume de l'impact d'une modification avant de coder.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" },
                        "depth": { "type": "integer", "description": "Profondeur d'impact (optionnel)" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "axon_cypher",
                "description": "[SYSTEM] Interface de bas niveau pour requêtes graphe brutes. Reservee au diagnostic et aux usages experts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cypher": { "type": "string" }
                    },
                    "required": ["cypher"]
                }
            },
            json!({
                "name": "axon_debug",
                "description": "[SYSTEM] Diagnostic système bas niveau : Affiche l'état interne du moteur Axon V2 (RAM, DB, architecture, statut d'indexation) pour éviter les hallucinations sur l'infrastructure.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            })
        ]
    })
}
