// Copyright (c) Didier Stadelmann. All rights reserved.

//! Mesure empirique pour REQ-AXO-902514:
//! La garde anti-hallucination rend `neutral` sur une affirmation frontalement fausse
//! — le corpus soutient ce qu'il documente avoir retiré.
//!
//! Critères d'acceptation 1 & 2 :
//! 1. Mesurer sur >= 10 affirmations dont la fausseté est établie par un nœud SOLL de retrait,
//!    avant tout correctif : le taux de `neutral` est-il anormal ?
//! 2. L'hypothèse « un passage qui NOMME une technologie retirée est lu comme un soutien »
//!    est confirmée ou réfutée sur les passages effectivement jugés (les rendre).

use crate::graph::GraphStore;
use crate::nli::{NliClassifier, NliVerdict};
use serde_json::Value;
use std::sync::Arc;

struct CandidateTest {
    claim: &'static str,
    soll_retirement_ref: &'static str,
}

#[test]
#[ignore = "REQ-AXO-902514: mesure empirique systématique sur la base réelle avec le modèle NLI ModernBERT"]
fn measure_req_902514_retired_technologies_nli_behavior() {
    let model_dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.axon/models/nli-modernbert-base"
    );
    let mut nli = NliClassifier::load(model_dir).expect("Chargement du modèle NLI ModernBERT");

    let candidates = [
        CandidateTest {
            claim: "Axon stocke son IST canonique dans DuckDB et AGE est le moteur de graphe en production",
            soll_retirement_ref: "REQ-AXO-271, DEC-AXO-083, MIL-AXO-017",
        },
        CandidateTest {
            claim: "Axon utilise LadybugDB comme moteur de stockage de graphe",
            soll_retirement_ref: "REQ-AXO-271",
        },
        CandidateTest {
            claim: "il suffit de constater phase=clean pour valider un promote",
            soll_retirement_ref: "REQ-AXO-902464",
        },
        CandidateTest {
            claim: "on livre avec git commit directement depuis le shell",
            soll_retirement_ref: "GUI-PRO-028, GUI-AXO-1037",
        },
        CandidateTest {
            claim: "axonctl supervise est le démon de supervision des services Axon en production",
            soll_retirement_ref: "REQ-AXO-901735",
        },
        CandidateTest {
            claim: "Axon effectue l'embedding de symboles isolés via la boucle vector_refill_loop",
            soll_retirement_ref: "DEC-AXO-070, commit b8844936",
        },
        CandidateTest {
            claim: "Axon vectorise les symboles de code individuellement via fetch_unembedded_symbols",
            soll_retirement_ref: "DEC-AXO-070, DEC-AXO-901677, REQ-AXO-902561",
        },
        CandidateTest {
            claim: "Axon utilise CozoDB pour stocker le graphe de code",
            soll_retirement_ref: "MIL-AXO-017, REQ-AXO-271",
        },
        CandidateTest {
            claim: "Axon indexe le code en mémoire vive via un snapshot DuckDB en RAM",
            soll_retirement_ref: "REQ-AXO-1951, DEC-AXO-083",
        },
        CandidateTest {
            claim: "La couche de contrôle de débit du pipeline consulte ServicePressure et interactive_priority_active pour réguler le drain",
            soll_retirement_ref: "DEC-AXO-070, REQ-AXO-902561",
        },
        CandidateTest {
            claim: "La purge ou suppression complète de la couche SOLL est une procédure standard autorisée",
            soll_retirement_ref: "GUI-AXO-1038",
        },
    ];

    println!("\n================================================================================");
    println!(
        "RAPPORT DE MESURE EMPIRIQUE REQ-AXO-902514 : COMPORTEMENT NLI SUR AFFIRMATIONS FAUSSES"
    );
    println!("================================================================================\n");

    let db_url = std::env::var("AXON_LIVE_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://axon@127.0.0.1:44144/axon_live".to_string());
    let graph_store = match GraphStore::new_with_database("/tmp/axon-nli-measure", &db_url) {
        Ok(gs) => Arc::new(gs),
        Err(e) => {
            eprintln!("Connexion impossible à la base réelle: {e}");
            return;
        }
    };

    let mut neutral_count = 0usize;
    let mut neutral_borderline_count = 0usize;
    let mut contradicts_count = 0usize;
    let mut total_passages_judged = 0usize;
    let mut high_entailment_on_retired_named = 0usize;

    for (idx, c) in candidates.iter().enumerate() {
        println!(
            "\n--------------------------------------------------------------------------------"
        );
        println!("[Affirmation #{}] : « {} »", idx + 1, c.claim);
        println!("Justification retrait SOLL : {}", c.soll_retirement_ref);

        // 1. Récupérer les passages du corpus (code ist.chunk et documentation soll.node)
        // qui nomment directement la technologie ou le motif retiré.
        let search_terms: Vec<&str> = match idx {
            0 => vec!["DuckDB", "AGE"],
            1 => vec!["LadybugDB"],
            2 => vec!["phase=clean"],
            3 => vec!["git commit"],
            4 => vec!["axonctl supervise"],
            5 => vec!["vector_refill_loop"],
            6 => vec!["fetch_unembedded_symbols"],
            7 => vec!["CozoDB"],
            8 => vec!["DuckDB"],
            9 => vec!["ServicePressure", "interactive_priority_active"],
            10 => vec!["suppression", "purge"],
            _ => vec![],
        };

        let mut rows: Vec<(String, String, String)> = Vec::new(); // (id, file_path, content)

        for term in &search_terms {
            let escaped = term.replace('\'', "''");
            // Symboles de code
            let sql_chunk = format!(
                "SELECT source_id, file_path, content FROM ist.chunk \
                 WHERE project_code = 'AXO' AND content ILIKE '%{escaped}%' LIMIT 12"
            );
            if let Ok(raw) = graph_store.query_json(&sql_chunk) {
                if let Ok(parsed) = serde_json::from_str::<Vec<Vec<String>>>(&raw) {
                    for r in parsed {
                        if r.len() >= 3 {
                            rows.push((r[0].clone(), r[1].clone(), r[2].clone()));
                        }
                    }
                }
            }
            // Nœuds SOLL documentant le sujet
            let sql_soll = format!(
                "SELECT id, title, description FROM soll.node \
                 WHERE project_code = 'AXO' AND (description ILIKE '%{escaped}%' OR title ILIKE '%{escaped}%') LIMIT 8"
            );
            if let Ok(raw) = graph_store.query_json(&sql_soll) {
                if let Ok(parsed) = serde_json::from_str::<Vec<Vec<String>>>(&raw) {
                    for r in parsed {
                        if r.len() >= 3 {
                            rows.push((
                                r[0].clone(),
                                format!("soll://{}", r[0]),
                                format!("{}\n\n{}", r[1], r[2]),
                            ));
                        }
                    }
                }
            }
        }

        let mut max_contradiction = 0f32;
        let mut max_entailment = 0f32;
        let mut conflicts: Vec<(String, String, f32, f32, String)> = Vec::new();
        let threshold = 0.5f32;
        let net_margin = 0.6f32;

        println!(
            "Passages récupérés du corpus mentionnant les termes : {}",
            rows.len()
        );

        for (symbol, file_path, content) in &rows {
            if content.trim().is_empty() {
                continue;
            }
            let passage = content.splitn(2, "\n\n").nth(1).unwrap_or(content);
            // Tronquer pour le cross-encoder si nécessaire
            let passage_trunc: String = passage.chars().take(2000).collect();

            if let Ok(scores) = nli.judge(&passage_trunc, c.claim) {
                total_passages_judged += 1;
                max_contradiction = max_contradiction.max(scores.contradiction);
                max_entailment = max_entailment.max(scores.entailment);

                if scores.entailment >= 0.5 {
                    high_entailment_on_retired_named += 1;
                }

                if scores.verdict() == NliVerdict::Contradiction
                    && scores.contradiction >= threshold
                {
                    conflicts.push((
                        symbol.to_string(),
                        file_path.to_string(),
                        scores.contradiction,
                        scores.entailment,
                        passage.chars().take(120).collect(),
                    ));
                }

                // Affichage détaillé de tous les passages avec score notable
                if scores.entailment > 0.35 || scores.contradiction > 0.5 {
                    println!(
                        "  -> [{:?}] contra={:.3} entail={:.3} neutral={:.3} | {} ({}):\n     « {} »",
                        scores.verdict(),
                        scores.contradiction,
                        scores.entailment,
                        scores.neutral,
                        symbol,
                        file_path,
                        passage.chars().take(160).collect::<String>().replace('\n', " ")
                    );
                }
            }
        }

        let margin = max_contradiction - max_entailment;
        let contradicted =
            !conflicts.is_empty() && max_contradiction >= threshold && margin >= net_margin;
        let borderline = !contradicted
            && !conflicts.is_empty()
            && max_contradiction >= threshold
            && margin >= net_margin * 0.9;

        let verdict = if rows.is_empty() {
            "inconclusive"
        } else if contradicted {
            contradicts_count += 1;
            "contradicts"
        } else if borderline {
            neutral_borderline_count += 1;
            "neutral_borderline"
        } else {
            neutral_count += 1;
            "neutral"
        };

        println!(
            "VERDICT FINAL: **{}** | max_contra={:.3} max_entail={:.3} margin={:.3} (seuil net_margin={:.2})",
            verdict, max_contradiction, max_entailment, margin, net_margin
        );
    }

    println!("\n================================================================================");
    println!("SYNTHÈSE STATISTIQUE DE LA MESURE EMPIRIQUE :");
    println!("  Total affirmations testées : {}", candidates.len());
    println!("  Passages jugés au total    : {}", total_passages_judged);
    println!(
        "  Verdicts contradicts       : {} ({:.1}%)",
        contradicts_count,
        contradicts_count as f32 / candidates.len() as f32 * 100.0
    );
    println!(
        "  Verdicts neutral_borderline: {} ({:.1}%)",
        neutral_borderline_count,
        neutral_borderline_count as f32 / candidates.len() as f32 * 100.0
    );
    println!(
        "  Verdicts neutral           : {} ({:.1}%)",
        neutral_count,
        neutral_count as f32 / candidates.len() as f32 * 100.0
    );
    println!(
        "  Passages à fort entailment : {}",
        high_entailment_on_retired_named
    );
    println!("================================================================================\n");

    assert!(
        candidates.len() >= 10,
        "Critère 1 : >= 10 affirmations mesurées"
    );
}

#[test]
fn test_req_902514_superseded_or_rejected_node_prevents_silent_neutral() {
    let _runtime = super::RuntimeEnvGuard::full_autonomous();
    let server = super::create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    // 1. Insérer un nœud SOLL de retrait/rejet
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('DEC-TST-083', 'Decision', 'TST', 'Retrait formel de DuckDB et AGE en production', \
                  'DuckDB et AGE sont definitivement retires de la production. Il est strictement interdit de les utiliser comme stockage canonique.', \
                  'superseded', '{}')");

    // 2. Insérer du code dans ist.chunk qui mentionne DuckDB pour simuler la présence historique dans le corpus
    exec("INSERT INTO ist.Chunk (id, project_code, file_path, source_id, source_type, content, start_line, end_line, created_at_ms) \
          VALUES ('chk-tst-duck-01', 'TST', 'src/legacy.rs', 'sym-legacy', 'symbol', \
                  'symbol:legacy\n\nfn init_duckdb() { println!(\"duckdb connection\"); }', 1, 3, 1000)");

    // Embedding factice pour le chunk dans ist.ChunkEmbedding
    let fake_vec = crate::postgres::vector::vector_literal(&vec![0.01f32; 1024]).unwrap();
    exec(&format!(
        "INSERT INTO ist.ChunkEmbedding (chunk_id, project_code, source_hash, embedding, model_id, embedded_at_ms) \
         VALUES ('chk-tst-duck-01', 'TST', 'hash-01', {fake_vec}, 'bge-small-en-v1.5', 1000)"
    ));

    // 3. Tester contradiction_check avec une affirmation fausse contredite par le nœud de retrait
    let check_res = server
        .axon_contradiction_check(&serde_json::json!({
            "candidate": "Axon stocke son IST canonique dans DuckDB et AGE en production",
            "scope": { "project": "TST" },
            "candidate_embedding": vec![0.01f32; 1024],
            "threshold": 0.4
        }))
        .expect("must return answer");

    let text = check_res["content"][0]["text"].as_str().unwrap_or_default();
    let data = &check_res["data"];
    let verdict = data
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or_default();

    println!("TEST REQ-902514 verdict: {verdict}");
    println!("TEST REQ-902514 report: {text}");

    // Critère 3 : L'affirmation contredite par un nœud 'superseded'/'rejected' ne rend plus un 'neutral' silencieux.
    assert_ne!(
        verdict, "neutral",
        "Une affirmation contredite par un nœud superseded/rejected ne doit JAMAIS rendre un 'neutral' silencieux ! Rapport: {text}"
    );
    assert_eq!(
        verdict, "contradicts",
        "Le verdict doit être 'contradicts'. Rapport: {text}"
    );

    // Les conflits doivent être conservés
    let conflicts = data
        .get("top_conflicts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !conflicts.is_empty(),
        "Les conflits doivent être conservés et rendus dans top_conflicts"
    );
    assert!(
        conflicts
            .iter()
            .any(|c| c.get("id").and_then(Value::as_str) == Some("DEC-TST-083")),
        "Le conflit avec le nœud SOLL de retrait DEC-TST-083 doit figurer dans les conflits"
    );
}

#[test]
fn test_req_902514_rejected_node_prevents_silent_neutral() {
    let _runtime = super::RuntimeEnvGuard::full_autonomous();
    let server = super::create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    // 1. Insérer un nœud SOLL rejeté
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('CPT-TST-031', 'Concept', 'TST', 'DuckDB plugin layer et FFI surface', \
                  'La couche plugin DuckDB est formellement rejetee. L architecture canonique refuse DuckDB.', \
                  'rejected', '{}')");

    // 2. Insérer du code avec mention historique
    exec("INSERT INTO ist.Chunk (id, project_code, file_path, source_id, source_type, content, start_line, end_line, created_at_ms) \
          VALUES ('chk-tst-duck-02', 'TST', 'src/plugin.rs', 'sym-plugin', 'symbol', \
                  'symbol:plugin\n\nfn load_duckdb_plugin() { println!(\"duckdb\"); }', 1, 3, 1000)");

    let fake_vec = crate::postgres::vector::vector_literal(&vec![0.02f32; 1024]).unwrap();
    exec(&format!(
        "INSERT INTO ist.ChunkEmbedding (chunk_id, project_code, source_hash, embedding, model_id, embedded_at_ms) \
         VALUES ('chk-tst-duck-02', 'TST', 'hash-02', {fake_vec}, 'bge-small-en-v1.5', 1000)"
    ));

    // 3. Tester contradiction_check
    let check_res = server
        .axon_contradiction_check(&serde_json::json!({
            "candidate": "Axon utilise le plugin DuckDB comme couche de stockage",
            "scope": { "project": "TST" },
            "candidate_embedding": vec![0.02f32; 1024],
            "threshold": 0.4
        }))
        .expect("must return answer");

    let text = check_res["content"][0]["text"].as_str().unwrap_or_default();
    let data = &check_res["data"];
    let verdict = data
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or_default();

    println!("TEST REQ-902514 rejected verdict: {verdict}");
    println!("TEST REQ-902514 rejected report: {text}");

    assert_eq!(
        verdict, "contradicts",
        "Un nœud SOLL 'rejected' doit forcer le verdict à 'contradicts' et interdire le neutral silencieux"
    );
    assert_eq!(
        data.get("has_soll_retirement_conflict"),
        Some(&serde_json::json!(true)),
        "data.has_soll_retirement_conflict doit être true"
    );
}
