use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use serde_json::{json, Value};

fn is_public_tool(name: &str) -> bool {
    !matches!(
        name,
        "refine_lattice"
            | "job_status"
            | "audit"
            | "health"
            | "batch"
            | "cypher"
            | "debug"
            | "schema_overview"
            | "list_labels_tables"
            | "query_examples"
            | "truth_check"
            | "diagnose_indexing"
            | "diff"
            | "semantic_clones"
            | "architectural_drift"
            | "bidi_trace"
            | "api_break_check"
            | "simulate_mutation"
            | "resume_vectorization"
    )
}

pub(crate) fn requires_indexed_runtime(name: &str) -> bool {
    matches!(
        name,
        "query"
            | "inspect"
            | "audit"
            | "impact"
            | "health"
            | "diagnose_indexing"
            | "diff"
            | "semantic_clones"
            | "architectural_drift"
            | "bidi_trace"
            | "api_break_check"
            | "simulate_mutation"
            | "truth_check"
            | "retrieve_context"
    )
}

fn tool_available_in_runtime(name: &str) -> bool {
    let runtime_mode = AxonRuntimeMode::from_env();
    let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
        runtime_mode.as_str(),
        std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
            .ok()
            .as_deref(),
    );

    if requires_indexed_runtime(name) {
        return matches!(
            runtime_profile,
            AxonRuntimeOperationalProfile::FullAutonomous
        );
    }

    match runtime_mode {
        AxonRuntimeMode::Full
        | AxonRuntimeMode::GraphOnly
        | AxonRuntimeMode::ReadOnly
        | AxonRuntimeMode::McpOnly => true,
    }
}

pub(crate) fn tools_catalog(include_internal: bool) -> Value {
    let mut catalog = json!({
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
                "description": "[SOLL] Centre de commande pour le graphe intentionnel. Gère la création (avec IDs auto), la mise à jour et les liaisons hiérarchiques. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["create", "update", "link"], "description": "L'opération à effectuer." },
                        "entity": { "type": "string", "enum": ["vision", "pillar", "requirement", "concept", "milestone", "decision", "stakeholder", "validation", "guideline"], "description": "Le type d'objet concerné." },
                        "data": {
                            "type": "object",
                            "description": "Données JSON. \n- create (vision/pillar/requirement/concept/decision/milestone/stakeholder/validation/guideline) avec `project_code`; le serveur retourne l'ID canonique `TYPE-CODE-NNN`.\n- update (id canonique requis, status/desc/etc).\n- link (source_id, target_id canoniques)."
                        }
                    },
                    "required": ["action", "entity", "data"]
                }
            },
            {
                "name": "axon_init_project",
                "description": "[DX/SOLL] Initialise un nouveau projet Axon. Reçoit un Document de Concept optionnel, charge les règles globales et lance le dialogue d'héritage.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_name": { "type": "string", "description": "Le nom du projet (ex: BookingSystem)." },
                        "project_code": { "type": "string", "description": "Le code canonique en 3 caractères (ex: BKS)." },
                        "project_path": { "type": "string", "description": "Le chemin absolu canonique du projet (ex: /home/dstadel/projects/BookingSystem)." },
                        "concept_document_url_or_text": { "type": "string", "description": "Optionnel: le texte ou lien vers la vision du projet." }
                    },
                    "required": ["project_name", "project_code", "project_path"]
                }
            },
            {
                "name": "axon_apply_guidelines",
                "description": "[DX/SOLL] Instancie les règles globales sélectionnées pour un projet spécifique.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Le code canonique en 3 caractères du projet cible." },
                        "accepted_global_rule_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Liste des IDs canoniques des règles globales à appliquer (ex: GUI-PRO-001)."
                        }
                    },
                    "required": ["project_code", "accepted_global_rule_ids"]
                }
            },
            {
                "name": "axon_commit_work",
                "description": "[DX/SOLL] Outil OBLIGATOIRE pour valider et commiter le travail. Évalue les fichiers modifiés contre les Guidelines SOLL. Ne JAMAIS utiliser git commit via shell.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Liste des chemins de fichiers modifiés."
                        },
                        "message": { "type": "string", "description": "Message de commit (Conventional Commits)." },
                        "dry_run": { "type": "boolean", "description": "Si true, valide uniquement sans commiter." }
                    },
                    "required": ["diff_paths", "message"]
                }
            },
            {
                "name": "axon_pre_flight_check",
                "description": "[DX/SOLL] Validation dry-run obligatoire avant commit. Vérifie les fichiers modifiés contre les Guidelines SOLL sans créer de commit.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "diff_paths": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Liste des chemins de fichiers modifiés."
                        },
                        "message": { "type": "string", "description": "Message optionnel pour journaliser la validation. Par défaut: 'pre-flight-check'." }
                    },
                    "required": ["diff_paths"]
                }
            },
            {
                "name": "soll_apply_plan",
                "description": "[SOLL] Wrapper haut niveau idempotent pour appliquer un plan SOLL (pillars, requirements, decisions, milestones) avec dry-run et rapport created/updated/skipped/errors. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet (ex: AXO)." },
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
                "name": "soll_commit_revision",
                "description": "[SOLL] Commit atomique d'un preview SOLL vers une revision journalisée. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
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
                "description": "[SOLL] Retourne le contexte projet (requirements, decisions, revisions) compact et prêt pour consommation LLM. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_work_plan",
                "description": "[SOLL] Produit un plan de travail ideal read-only a partir du graphe intentionnel, avec waves paralleles, blockers, cycles et gates de validation. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" },
                        "limit": { "type": "integer" },
                        "top": { "type": "integer" },
                        "include_ist": { "type": "boolean" },
                        "format": { "type": "string", "enum": ["brief", "verbose", "json"] }
                    },
                    "required": ["project_code"]
                }
            },
            {
                "name": "soll_attach_evidence",
                "description": "[SOLL] Attache des preuves (fichier/test/metric/dashboard) à une entité SOLL. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
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
                "description": "[SOLL] Vérifie la couverture requirements (done/partial/missing) selon critères et preuves rattachées. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string" }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_rollback_revision",
                "description": "[SOLL] Rollback best-effort d'une révision SOLL via le journal RevisionChange. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "revision_id": { "type": "string" }
                    },
                    "required": ["revision_id"]
                }
            },
            {
                "name": "soll_export",
                "description": "[SOLL] Exporte l'intégralité du graphe intentionnel (Vision, Pillars, Milestones, Requirements, Decisions, Concepts) dans un document Markdown horodaté. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filtre l'export au projet demandé." }
                    },
                    "required": []
                }
            },
            {
                "name": "restore_soll",
                "description": "[SOLL] Restaure les entites conceptuelles depuis un export Markdown officiel SOLL. Fonctionne en mode merge, sans purge destructive implicite. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Chemin optionnel vers un export SOLL. Par defaut: dernier fichier docs/vision/SOLL_EXPORT_*.md." }
                    },
                    "required": []
                }
            },
            {
                "name": "soll_validate",
                "description": "[SOLL] Exécute des garde-fous minimaux de cohérence sur le graphe intentionnel. Validation en lecture seule: détecte les états orphelins évidents sans modifier SOLL. Guide opérateur: docs/skills/axon-engineering-protocol/SKILL.md",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Filtre la validation au projet demandé." }
                    },
                    "required": []
                }
            },
            {
                "name": "job_status",
                "description": "[SYSTEM/EXPERT] Retourne l'état détaillé d'un job MCP mutateur accepté par le serveur partagé.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Identifiant du job (ex: JOB-1712851200000)." }
                    },
                    "required": ["job_id"]
                }
            },
            {
                "name": "status",
                "description": "[SYSTEM] Vue opérateur unifiée: état runtime, profil actif, disponibilité des surfaces avancées et signaux de vérité/dégradation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "project_status",
                "description": "[SYSTEM/SOLL] Etat de situation vivant du projet: vision source SOLL, état runtime, surface opérateur, diagnostics structuraux et contexte SOLL récent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet canonique (défaut: AXO)." },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "snapshot_history",
                "description": "[SYSTEM] Historique dérivé non canonique des snapshots structurels exportés par `project_status` pour un projet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet canonique (défaut: AXO)." },
                        "limit": { "type": "integer", "description": "Nombre maximum de snapshots retournés (défaut 10)." }
                    },
                    "required": []
                }
            },
            {
                "name": "snapshot_diff",
                "description": "[SYSTEM] Diff dérivé entre deux snapshots structurels non canoniques d'un projet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet canonique (défaut: AXO)." },
                        "from_snapshot_id": { "type": "string", "description": "Snapshot source optionnel; défaut: précédent." },
                        "to_snapshot_id": { "type": "string", "description": "Snapshot cible optionnel; défaut: dernier." }
                    },
                    "required": []
                }
            },
            {
                "name": "conception_view",
                "description": "[SYSTEM/DX] Vue de conception dérivée et lecture seule: modules, interfaces, contrats, flux et violations de frontières suspectées.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet canonique (défaut: AXO)." },
                        "mode": { "type": "string", "enum": ["brief", "full"] }
                    },
                    "required": []
                }
            },
            {
                "name": "change_safety",
                "description": "[SYSTEM/DX/SOLL] Résume la sûreté de changement d'une cible via tests, traceability et validation dérivée.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project_code": { "type": "string", "description": "Code projet canonique (défaut: AXO)." },
                        "target": { "type": "string", "description": "Symbole, fichier ou entité cible." },
                        "target_type": { "type": "string", "enum": ["symbol", "file", "intent"] },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["target"]
                }
            },
            {
                "name": "why",
                "description": "[DX/SOLL] Explique pourquoi un symbole, fichier ou sujet existe via liaisons code, traceability et rationale SOLL.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Symbole ou entité cible." },
                        "question": { "type": "string", "description": "Question libre si le symbole seul ne suffit pas." },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": []
                }
            },
            {
                "name": "path",
                "description": "[DX] Explique un chemin d'exécution ou de dépendance entre deux points, ou bascule en trace topologique si seul un point d'ancrage est fourni.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source": { "type": "string", "description": "Symbole source ou ancre de départ." },
                        "sink": { "type": "string", "description": "Symbole cible optionnel." },
                        "project": { "type": "string" },
                        "depth": { "type": "integer", "description": "Profondeur maximale (défaut 6)." },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] }
                    },
                    "required": ["source"]
                }
            },
            {
                "name": "anomalies",
                "description": "[GOVERNANCE] Agrège les anomalies structurelles prioritaires: cycles, god objects, wrappers et orphelins, avec sévérité, confiance et action recommandée.",
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
                "name": "retrieve_context",
                "description": "[DX] Planner-driven retrieval that assembles an evidence packet for LLM answerability from canonical truth, chunks, bounded graph context, and relevant SOLL rationale.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "project": { "type": "string" },
                        "mode": { "type": "string", "enum": ["brief", "verbose"] },
                        "token_budget": { "type": "integer" },
                        "top_k": { "type": "integer" },
                        "include_soll": { "type": "boolean" },
                        "include_graph": { "type": "boolean" }
                    },
                    "required": ["question"]
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
                "description": "[GOVERNANCE/EXPERT] Vérification de conformité approfondie (sécurité, qualité, anti-patterns, dette technique). À réserver aux diagnostics experts plutôt qu'au premier choix LLM.",
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
                "description": "[GOVERNANCE/EXPERT] Rapport de santé agrégé (code mort, lacunes de tests, points d'entrée). À réserver aux diagnostics experts plutôt qu'au premier choix LLM.",
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
                "description": "[SYSTEM/EXPERT] Orchestration experte d'appels multiples pour optimiser la performance ou piloter des outils avancés.",
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
            }),
            json!({
                "name": "resume_vectorization",
                "description": "[SYSTEM] Recrée explicitement la queue de vectorisation manquante à partir des fichiers déjà graph_indexed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            })
        ]
    });

    if let Some(tools) = catalog
        .get_mut("tools")
        .and_then(|value| value.as_array_mut())
    {
        tools.retain(|tool| {
            tool.get("name")
                .and_then(|value| value.as_str())
                .is_some_and(|name| {
                    tool_available_in_runtime(name) && (include_internal || is_public_tool(name))
                })
        });
    }

    catalog
}
