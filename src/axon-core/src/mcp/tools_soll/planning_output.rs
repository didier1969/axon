use super::*;

impl McpServer {
    pub(super) fn render_work_plan_text(
        &self,
        project_code: &str,
        waves: &[WorkPlanWave],
        blockers: &[WorkPlanBlocker],
        cycles: &[WorkPlanCycle],
        top_recommendations: &[Value],
        truncated: bool,
    ) -> String {
        let mut evidence = String::new();
        if !top_recommendations.is_empty() {
            evidence.push_str("Immediate actions:\n");
            for rec in top_recommendations {
                let id = rec.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
                let kind = rec.get("kind").and_then(|v| v.as_str()).unwrap_or("task");
                let reason = rec
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("action immediate");
                evidence.push_str(&format!("- {} [{}] : {}\n", id, kind, reason));
            }
            evidence.push('\n');
        }
        if !blockers.is_empty() {
            evidence.push_str("Blockers:\n");
            for blocker in blockers {
                evidence.push_str(&format!(
                    "- {} ({}) : {}\n",
                    blocker.id, blocker.entity_type, blocker.reason
                ));
            }
            evidence.push('\n');
        }
        if !cycles.is_empty() {
            evidence.push_str("Cycles:\n");
            for cycle in cycles {
                evidence.push_str(&format!("- {}\n", cycle.node_ids.join(" -> ")));
            }
            evidence.push('\n');
        }
        for wave in waves {
            evidence.push_str(&format!("Wave {}:\n", wave.wave_index));
            for item in &wave.items {
                evidence.push_str(&format!(
                    "- {} [{}] score={} :: {}\n",
                    item.id,
                    item.entity_type.label(),
                    item.score,
                    item.reasons.join(", ")
                ));
            }
            evidence.push('\n');
        }
        if truncated {
            evidence.push_str("[truncated=true]\n");
        }
        format!(
            "### 🗺️ SOLL Work Plan: {}\n\n{}",
            project_code,
            format_standard_contract(
                "ok",
                "work plan computed from SOLL",
                &format!("project:{}", project_code),
                &evidence,
                &[
                    "review blockers before execution",
                    "use `format=json` for machine consumption"
                ],
                "medium",
            )
        )
    }
}

pub(super) fn build_top_recommendations(waves: &[WorkPlanWave], top: usize) -> Vec<Value> {
    let mut recommendations = Vec::new();
    for wave in waves {
        for item in &wave.items {
            recommendations.push(json!({
                "id": item.id,
                "entity_type": item.entity_type.label(),
                "title": item.title,
                "score": item.score,
                "wave_index": wave.wave_index,
                "kind": recommendation_kind(item),
                "reason": recommendation_reason(item),
                "validation_gates": item.validation_gates
            }));
            if recommendations.len() >= top {
                return recommendations;
            }
        }
    }
    recommendations
}
