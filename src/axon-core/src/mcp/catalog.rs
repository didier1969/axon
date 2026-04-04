use serde_json::{json, Value};

pub(crate) fn tools_catalog() -> Value {
    json!({
        "tools": [
            {
                "name": "refine_lattice",
                "description": "[SYSTEM] Lattice Refiner: Analyse le graphe post-ingestion pour lier les frontières inter-langages (ex: Elixir NIF -> Rust natif).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "fs_read",
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
                "name": "soll_manager",
                "description": "[SOLL] Centre de commande pour le graphe intentionnel. Gère la création (avec IDs auto), la mise à jour et les liaisons hiérarchiques. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
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
                "name": "soll_apply_plan",
                "description": "[SOLL] Wrapper haut niveau idempotent pour appliquer un plan SOLL (pillars, requirements, decisions, milestones) avec dry-run et rapport created/updated/skipped/errors. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_slug": { "type": "string", "description": "Slug projet (ex: AXO)." },
                        "dry_run": { "type": "boolean", "description": "Si true, ne modifie rien et produit seulement le plan d'action." },
                        "plan": {
                            "type": "object",
                            "properties": {
                                "pillars": { "type": "array", "items": { "type": "object" } },
                                "requirements": { "type": "array", "items": { "type": "object" } },
                                "decisions": { "type": "array", "items": { "type": "object" } },
                                "milestones": { "type": "array", "items": { "type": "object" } }
                            }
                        }
                    },
                    "required": ["plan"]
                }
            },
            {
                "name": "soll_apply_plan_v2",
                "description": "[SOLL] Prépare un plan révisable (dry-run par défaut), persiste un preview et fournit le diff d'opérations create/update. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_slug": { "type": "string" },
                        "author": { "type": "string" },
                        "dry_run": { "type": "boolean" },
                        "plan": { "type": "object" }
                    },
                    "required": ["plan"]
                }
            },
            {
                "name": "soll_commit_revision",
                "description": "[SOLL] Commit atomique d'un preview SOLL vers une revision journalisée. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "preview_id": { "type": "string" },
                        "author": { "type": "string" }
                    },
                    "required": ["preview_id"]
                }
            },
            {
                "name": "soll_query_context",
                "description": "[SOLL] Retourne le contexte projet (requirements, decisions, revisions) compact et prêt pour consommation LLM. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_slug": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_work_plan",
                "description": "[SOLL] Produit un plan de travail ideal read-only a partir du graphe intentionnel, avec waves paralleles, blockers, cycles et gates de validation. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_slug": { "type": "string" },
                        "limit": { "type": "integer" },
                        "top": { "type": "integer" },
                        "include_ist": { "type": "boolean" },
                        "format": { "type": "string", "enum": ["brief", "verbose", "json"] }
                    },
                    "required": ["project_slug"]
                }
            },
            {
                "name": "soll_attach_evidence",
                "description": "[SOLL] Attache des preuves (fichier/test/metric/dashboard) à une entité SOLL. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity_type": { "type": "string" },
                        "entity_id": { "type": "string" },
                        "artifacts": { "type": "array", "items": { "type": "object" } }
                    },
                    "required": ["entity_type", "entity_id", "artifacts"]
                }
            },
            {
                "name": "soll_verify_requirements",
                "description": "[SOLL] Vérifie la couverture requirements (done/partial/missing) selon critères et preuves rattachées. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_slug": { "type": "string" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_rollback_revision",
                "description": "[SOLL] Rollback best-effort d'une révision SOLL via le journal RevisionChange. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "revision_id": { "type": "string" }
                    },
                    "required": ["revision_id"]
                }
            },
            {
                "name": "export_soll",
                "description": "[SOLL] Exporte l'intégralité du graphe intentionnel (Vision, Pillars, Milestones, Requirements, Decisions, Concepts) dans un document Markdown horodaté. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "restore_soll",
                "description": "[SOLL] Restaure les entites conceptuelles depuis un export Markdown officiel SOLL. Fonctionne en mode merge, sans purge destructive implicite. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Chemin optionnel vers un export SOLL. Par defaut: dernier fichier docs/vision/SOLL_EXPORT_*.md." }
                    },
                    "required": []
                }
            },
            {
                "name": "validate_soll",
                "description": "[SOLL] Exécute des garde-fous minimaux de cohérence sur le graphe intentionnel. Validation en lecture seule: détecte les états orphelins évidents sans modifier SOLL. Guide opérateur: docs/skills/axon-soll-operator/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "query",
                "description": "[DX] Recherche de symboles à forte valeur développeur. Utilise la recherche structurelle immédiatement, et ajoute la similarité sémantique seulement si l'embedding temps réel est disponible.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "inspect",
                "description": "[DX] Vue 360° d'un symbole (code source, appelants/appelés, statistiques).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "diagnose_indexing",
                "description": "[SYSTEM] Diagnostic Day-1 d'indexation par projet: causes probables, raisons dominantes, erreurs parser/runtime et remédiations.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Slug projet ou '*' pour global." }
                    },
                    "required": []
                }
            },
            {
                "name": "audit",
                "description": "[GOVERNANCE] Vérification de conformité (Sécurité OWASP, Qualité, Anti-patterns, Dette Technique).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "impact",
                "description": "[RISK] Analyse prédictive (Rayon d'impact et chemins critiques).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "depth": { "type": "integer" },
                        "project": { "type": "string" },
                        "symbol": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "health",
                "description": "[GOVERNANCE] Rapport de santé global (Code mort, lacunes de tests, points d'entrée).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "diff",
                "description": "[RISK] Analyse sémantique des changements (Git Diff -> Symboles touchés).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_content": { "type": "string" },
                        "limit": { "type": "integer", "description": "Maximum symboles par fichier (default 120, borné 10..500)" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["diff_content"]
                }
            },
            {
                "name": "batch",
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
                "name": "semantic_clones",
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
                "name": "architectural_drift",
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
                "name": "bidi_trace",
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
                "name": "api_break_check",
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
                "name": "simulate_mutation",
                "description": "[RISK] Dry-run : calcule le volume de l'impact d'une modification avant de coder.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "symbol": { "type": "string" },
                        "depth": { "type": "integer", "description": "Profondeur d'impact (optionnel)" }
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "schema_overview",
                "description": "[SYSTEM] Vue d'ensemble du schéma SQL Axon (tables main/soll, volumétrie colonnes).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "list_labels_tables",
                "description": "[SYSTEM] Inventaire des tables/labels principales et colonnes clés pour démarrer des requêtes sans connaissance interne.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "query_examples",
                "description": "[SYSTEM] Exemples de requêtes prêtes à l'emploi pour exploration, backlog, erreurs et bridges inter-langages.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "cypher",
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
                "name": "debug",
                "description": "[SYSTEM] Diagnostic système bas niveau : Affiche l'état interne du moteur Axon V2 (RAM, DB, architecture, statut d'indexation) pour éviter les hallucinations sur l'infrastructure.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            }),
            json!({
                "name": "truth_check",
                "description": "[SYSTEM] Contrôle de cohérence reader-path vs canonical writer sur les compteurs critiques (File/Symbol/CALLS...).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            })
        ]
    })
}
