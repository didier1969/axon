//! REQ-AXO-219 — supporting-chunk evidence classification helpers extracted from
//! the `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! functions on `McpServer`; behavior-preserving move, `Self::…` call sites
//! unchanged. They tag supporting chunks + structural neighbours with their
//! authority class / provenance / link mode for the evidence packet.

use super::super::McpServer;
use serde_json::Value;

impl McpServer {
    pub(super) fn classify_supporting_chunks_by_provenance(
        chunks: &[Value],
        provenance: &str,
        authority_class: &str,
    ) -> Vec<Value> {
        chunks
            .iter()
            .filter_map(|row| {
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let row_provenance = Self::evidence_provenance_for_uri(uri);
                (row_provenance == provenance).then(|| {
                    let mut enriched = row.clone();
                    let link_mode = match row
                        .get("anchored_to_entry")
                        .and_then(|value| value.as_bool())
                    {
                        Some(true) => "direct",
                        _ => "inferred",
                    };
                    if let Some(object) = enriched.as_object_mut() {
                        object.insert(
                            "authority_class".to_string(),
                            Value::String(authority_class.to_string()),
                        );
                        object.insert(
                            "evidence_provenance".to_string(),
                            Value::String(provenance.to_string()),
                        );
                        object.insert(
                            "link_mode".to_string(),
                            Value::String(link_mode.to_string()),
                        );
                        object.insert(
                            "inclusion_reason".to_string(),
                            Value::String(
                                row.get("match_reason")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("supporting_chunk")
                                    .to_string(),
                            ),
                        );
                    }
                    enriched
                })
            })
            .collect()
    }

    pub(super) fn classify_supporting_code_context(chunks: &[Value], neighbors: &[Value]) -> Vec<Value> {
        let mut items = chunks
            .iter()
            .filter_map(|row| {
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let provenance = Self::evidence_provenance_for_uri(uri);
                (provenance != "doc").then(|| {
                    let mut enriched = row.clone();
                    let link_mode = if matches!(provenance, "benchmark" | "test" | "script") {
                        "weak_correlation"
                    } else if row
                        .get("anchored_to_entry")
                        .and_then(|value| value.as_bool())
                        == Some(true)
                    {
                        "direct"
                    } else {
                        "inferred"
                    };
                    let authority_class = if link_mode == "weak_correlation" {
                        "correlated"
                    } else {
                        "supporting"
                    };
                    if let Some(object) = enriched.as_object_mut() {
                        object.insert(
                            "authority_class".to_string(),
                            Value::String(authority_class.to_string()),
                        );
                        object.insert(
                            "evidence_provenance".to_string(),
                            Value::String(provenance.to_string()),
                        );
                        object.insert(
                            "link_mode".to_string(),
                            Value::String(link_mode.to_string()),
                        );
                        object.insert(
                            "inclusion_reason".to_string(),
                            Value::String(
                                row.get("match_reason")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("supporting_chunk")
                                    .to_string(),
                            ),
                        );
                    }
                    enriched
                })
            })
            .collect::<Vec<_>>();

        for neighbor in neighbors {
            let mut enriched = neighbor.clone();
            if let Some(object) = enriched.as_object_mut() {
                object.insert(
                    "authority_class".to_string(),
                    Value::String("supporting".to_string()),
                );
                object.insert(
                    "evidence_provenance".to_string(),
                    Value::String("code_chunk".to_string()),
                );
                object.insert(
                    "link_mode".to_string(),
                    Value::String("inferred".to_string()),
                );
                object.insert(
                    "inclusion_reason".to_string(),
                    Value::String(
                        neighbor
                            .get("edge_kind")
                            .and_then(|value| value.as_str())
                            .unwrap_or("structural_neighbor")
                            .to_string(),
                    ),
                );
            }
            items.push(enriched);
        }

        items
    }

    pub(super) fn classify_governing_entities(
        entities: &[Value],
        expected_type: &str,
        provenance: &str,
    ) -> Vec<Value> {
        entities
            .iter()
            .filter(|row| row.get("type").and_then(|value| value.as_str()) == Some(expected_type))
            .map(|row| {
                let evidence_class = row
                    .get("evidence_class")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let ranking_reason = row
                    .get("ranking_reasons")
                    .and_then(|value| value.as_array())
                    .and_then(|items| items.first())
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let link_mode = if evidence_class == "soll_traceability"
                    && (ranking_reason.starts_with("direct_")
                        || ranking_reason.starts_with("requirement_"))
                {
                    "direct"
                } else if evidence_class == "soll_traceability"
                    || evidence_class == "soll_concept_bridge"
                {
                    "inferred"
                } else {
                    "weak_correlation"
                };
                let authority_class = if link_mode == "weak_correlation" {
                    "correlated"
                } else if expected_type == "Guideline" {
                    "supporting"
                } else {
                    "governing"
                };
                let mut enriched = row.clone();
                if let Some(object) = enriched.as_object_mut() {
                    object.insert(
                        "authority_class".to_string(),
                        Value::String(authority_class.to_string()),
                    );
                    object.insert(
                        "evidence_provenance".to_string(),
                        Value::String(provenance.to_string()),
                    );
                    object.insert(
                        "link_mode".to_string(),
                        Value::String(link_mode.to_string()),
                    );
                    object.insert(
                        "inclusion_reason".to_string(),
                        Value::String(ranking_reason.to_string()),
                    );
                }
                enriched
            })
            .collect()
    }

    pub(super) fn evidence_provenance_for_uri(uri: &str) -> &'static str {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("benchmark") {
            "benchmark"
        } else if matches!(Self::uri_penalty_reason(uri), Some("test_file_penalty")) {
            "test"
        } else if lower.contains("/scripts/") || lower.starts_with("scripts/") {
            "script"
        } else if matches!(Self::uri_penalty_reason(uri), Some("docs_file_penalty")) {
            "doc"
        } else {
            "code_chunk"
        }
    }

    pub(super) fn classify_direct_code_evidence(direct_evidence: &[Value]) -> Vec<Value> {
        direct_evidence
            .iter()
            .map(|row| {
                let mut enriched = row.clone();
                let kind = row
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let uri = row
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let evidence_class = row
                    .get("evidence_class")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown");
                let uri_provenance = Self::evidence_provenance_for_uri(uri);
                let provenance = if evidence_class == "repo_literal_file" {
                    uri_provenance
                } else if matches!(uri_provenance, "benchmark" | "test" | "script" | "doc") {
                    uri_provenance
                } else if kind == "file" {
                    "code_file"
                } else {
                    "code_symbol"
                };
                let authority_class = match provenance {
                    "benchmark" | "test" | "script" | "doc" => "correlated",
                    _ => "supporting",
                };
                let link_mode = if evidence_class == "repo_literal_file" {
                    "weak_correlation"
                } else {
                    "direct"
                };
                if let Some(object) = enriched.as_object_mut() {
                    object.insert(
                        "authority_class".to_string(),
                        Value::String(authority_class.to_string()),
                    );
                    object.insert(
                        "evidence_provenance".to_string(),
                        Value::String(provenance.to_string()),
                    );
                    object.insert(
                        "link_mode".to_string(),
                        Value::String(link_mode.to_string()),
                    );
                    object.insert(
                        "inclusion_reason".to_string(),
                        Value::String(
                            row.get("evidence_class")
                                .and_then(|value| value.as_str())
                                .unwrap_or("direct_evidence")
                                .to_string(),
                        ),
                    );
                }
                enriched
            })
            .collect()
    }
}
