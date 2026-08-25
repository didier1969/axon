use super::*;
use std::collections::BTreeMap;

impl McpServer {
    /// REQ-AXO-902443 — the Blockers section, folded.
    ///
    /// Measured on AXO (llm_feedback #215): 26 identical lines republished on
    /// EVERY call, and the canon makes an LLM call this tool after every batch
    /// (GUI-PRO-114 §4). Nine of them read `status_deferred` — `deferred` is a
    /// DELIBERATE decision, someone chose not to do them; filing them under a
    /// word that demands action is the opposite of what they are. The other
    /// sixteen were all blocked by the same three milestones, which is three
    /// lines of information printed as sixteen.
    ///
    /// The word "Blockers" is what costs: it calls for an action there is none
    /// to take. So: `deferred` gets a COUNT and a way to list them; the rest is
    /// folded per blocking node, which additionally answers the question you
    /// previously had to reconstruct by hand — *which milestone unblocks the
    /// most if I take it?*. Nothing is hidden: `format=verbose` still
    /// enumerates, and `data.blockers` was always complete.
    fn render_blocker_section(blockers: &[WorkPlanBlocker], verbose: bool) -> String {
        const BLOCKED_BY: &str = "blocked_by:";
        if verbose {
            let mut out = String::from("Blockers:\n");
            for blocker in blockers {
                out.push_str(&format!(
                    "- {} ({}) : {}\n",
                    blocker.id, blocker.entity_type, blocker.reason
                ));
            }
            out.push('\n');
            return out;
        }
        let mut deferred = 0usize;
        let mut by_blocking_node: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut other: Vec<&WorkPlanBlocker> = Vec::new();
        for blocker in blockers {
            if blocker.reason == "status_deferred" {
                deferred += 1;
            } else if let Some(target) = blocker.reason.strip_prefix(BLOCKED_BY) {
                by_blocking_node
                    .entry(target.trim())
                    .or_default()
                    .push(blocker.id.as_str());
            } else {
                other.push(blocker);
            }
        }

        let mut out = String::from("Blockers:\n");
        // Heaviest blocking node first — that is the actionable ordering.
        let mut folded: Vec<(&str, Vec<&str>)> = by_blocking_node.into_iter().collect();
        folded.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (target, blocked) in &folded {
            // Folding only pays when there is something to fold. A milestone
            // blocking ONE node loses information if its name is replaced by
            // "1 node(s)" — so name it. The fold exists to kill repetition,
            // not to hide identity.
            match blocked.as_slice() {
                [single] => out.push_str(&format!("- {target} blocks {single}\n")),
                many => out.push_str(&format!("- {target} blocks {} node(s)\n", many.len())),
            }
        }
        for blocker in other {
            out.push_str(&format!(
                "- {} ({}) : {}\n",
                blocker.id, blocker.entity_type, blocker.reason
            ));
        }
        if deferred > 0 {
            out.push_str(&format!(
                "- {deferred} node(s) `deferred` — a DECISION, not an obstacle; \
                 not listed (`soll_query_context` with status=deferred to see them)\n"
            ));
        }
        out.push('\n');
        out
    }

    pub(super) fn render_work_plan_text(
        &self,
        project_code: &str,
        waves: &[WorkPlanWave],
        blockers: &[WorkPlanBlocker],
        cycles: &[WorkPlanCycle],
        top_recommendations: &[Value],
        truncated: bool,
        verbose: bool,
        // REQ-AXO-902503 — statut terminal -> ids écartés, déjà triés.
        ecartes_par_statut: &std::collections::BTreeMap<String, Vec<String>>,
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
            evidence.push_str(&Self::render_blocker_section(blockers, verbose));
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
        // REQ-AXO-902503 — dire le PÉRIMÈTRE, pas seulement le résultat. Un plan vide
        // parce que tout est livré et un plan vide parce que rien n'existe sont deux
        // mondes différents ; sans cette ligne ils rendent le même texte.
        evidence.push_str(&render_ecartes_section(ecartes_par_statut, verbose));
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

/// REQ-AXO-902503 — le périmètre écarté, rendu à part pour être falsifiable.
///
/// Extraite du corps de `render_work_plan_text` pour une raison précise : le défaut
/// que SWT rapporte n'est pas « la ligne manque », c'est que **deux mondes rendent le
/// même texte**. Un plan vide parce que tout est livré et un plan vide parce que rien
/// n'existe étaient indiscernables. Falsifier ça demande de comparer DEUX sorties —
/// impossible à travers un `McpServer` sans monter deux projets entiers.
pub(super) fn render_ecartes_section(
    ecartes_par_statut: &std::collections::BTreeMap<String, Vec<String>>,
    verbose: bool,
) -> String {
    let mut out = String::new();
    let ecartes_total: usize = ecartes_par_statut.values().map(|v| v.len()).sum();
    if ecartes_total > 0 {
        // Le COMPTE est toujours rendu — c'est lui qui lève l'ambiguïté. Les IDS
        // ne le sont qu'en `verbose`, exactement comme REQ-AXO-902443 a plié la
        // section blockers : sur AXO il y a 128+ nœuds `delivered`, et les lister
        // à chaque appel de la boucle per-batch rajouterait du volume dans la
        // session même où la Vague 3 le combat. Un plan doit dire son périmètre,
        // pas le réciter.
        out.push_str(&format!(
            "Écartés du plan — {} nœud(s) en état terminal (ce n'est PAS de l'absence, c'est du travail fini) :\n",
            ecartes_total
        ));
        // Budget global, pas par statut : 6 statuts × 6 ids feraient 36 lignes
        // pour une information qui en vaut une.
        let mut budget = if verbose { 24usize } else { 0 };
        for (statut, ids) in ecartes_par_statut {
            if budget == 0 {
                out.push_str(&format!("- {} : {}\n", statut, ids.len()));
                continue;
            }
            let montres = ids.len().min(budget);
            // `Compte::borne` dit lequel des deux nombres on lit : le total est
            // exact, la liste est tronquée, et la sortie ne les confond pas.
            let compte = crate::mcp::format::Compte::borne(ids.len(), montres);
            out.push_str(&format!(
                "- {} : {} — {}{}\n",
                statut,
                compte.rendre(),
                ids.iter().take(montres).cloned().collect::<Vec<_>>().join(", "),
                if ids.len() > montres { ", …" } else { "" }
            ));
            budget -= montres;
        }
        if !verbose {
            out.push_str(
                "  _(ids en `format=verbose` — le compte suffit à distinguer « fini » de « inexistant »)_\n",
            );
        }
        out.push('\n');
    }
    out
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
                // REQ-AXO-902295 — the hygiene axis travels beside the
                // execution score on every surface, never folded into it.
                "proof_gap_score": item.proof_gap_score,
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

#[cfg(test)]
mod blocker_section_tests {
    use super::*;

    fn blocker(id: &str, reason: &str) -> WorkPlanBlocker {
        WorkPlanBlocker {
            id: id.to_string(),
            entity_type: "Requirement".to_string(),
            reason: reason.to_string(),
        }
    }

    /// REQ-AXO-902443 — the AXO measurement (llm_feedback #215): 26 lines
    /// republished identically on every call, 9 of them `status_deferred` and
    /// 16 pointing at the same three milestones. The canon makes an LLM call
    /// this tool after EVERY batch, so the section was re-read three times in
    /// one session for nothing.
    #[test]
    fn deferred_is_counted_and_blocked_by_is_folded_per_blocking_node() {
        let mut blockers: Vec<WorkPlanBlocker> = Vec::new();
        for i in 0..9 {
            blockers.push(blocker(&format!("REQ-AXO-9020{i:02}"), "status_deferred"));
        }
        for i in 0..8 {
            blockers.push(blocker(
                &format!("REQ-AXO-9021{i:02}"),
                "blocked_by:MIL-AXO-054",
            ));
        }
        for i in 0..4 {
            blockers.push(blocker(
                &format!("REQ-AXO-9022{i:02}"),
                "blocked_by:MIL-AXO-053",
            ));
        }
        assert_eq!(blockers.len(), 21);

        let folded = McpServer::render_blocker_section(&blockers, false);
        let lines = folded.lines().filter(|l| l.starts_with("- ")).count();
        assert!(
            lines <= 3,
            "21 blockers must fold to at most 3 lines, got {lines}:\n{folded}"
        );
        // The heaviest blocking node comes FIRST — that is the actionable
        // ordering the reader previously had to reconstruct by hand.
        let first = folded
            .lines()
            .find(|l| l.starts_with("- "))
            .unwrap_or_default();
        assert!(
            first.contains("MIL-AXO-054") && first.contains("8"),
            "heaviest blocker first with its count, got: {first}"
        );
        // Folding only pays when there is something to fold: a milestone
        // blocking ONE node is NAMED, not reduced to "1 node(s)". Losing the
        // identity of a lone blocked requirement would trade one defect for
        // another (caught by
        // `test_work_plan_separates_belonging_to_a_live_milestone_from_being_blocked`).
        let lone = vec![blocker("REQ-AXO-902368", "blocked_by:MIL-AXO-054")];
        let rendered = McpServer::render_blocker_section(&lone, false);
        assert!(
            rendered.contains("REQ-AXO-902368"),
            "a single blocked node keeps its id: {rendered}"
        );
        assert!(
            folded.contains("9 node(s) `deferred`") && folded.contains("DECISION"),
            "deferred is counted and named as a decision, not an obstacle:\n{folded}"
        );
        // No deferred id is enumerated — the count carries the information.
        assert!(
            !folded.contains("REQ-AXO-902000"),
            "deferred nodes are not listed one by one:\n{folded}"
        );
    }

    /// POSITIVE CONTROL — the audit surface keeps every line. Without this,
    /// the test above would also pass against a renderer that dropped blockers
    /// outright, which is the opposite of the contract.
    #[test]
    fn verbose_still_enumerates_every_blocker() {
        let blockers = vec![
            blocker("REQ-AXO-902070", "status_deferred"),
            blocker("REQ-AXO-902368", "blocked_by:MIL-AXO-054"),
        ];
        let verbose = McpServer::render_blocker_section(&blockers, true);
        assert!(verbose.contains("REQ-AXO-902070"), "{verbose}");
        assert!(verbose.contains("REQ-AXO-902368"), "{verbose}");
        assert!(
            verbose.contains("status_deferred"),
            "the raw reason survives in the audit surface: {verbose}"
        );
    }
}

#[cfg(test)]
mod ecartes_section_tests {
    use super::render_ecartes_section;
    use std::collections::BTreeMap;

    fn livres(n: usize) -> BTreeMap<String, Vec<String>> {
        let mut m = BTreeMap::new();
        m.insert(
            "delivered".to_string(),
            (0..n).map(|i| format!("REQ-TST-{i:03}")).collect(),
        );
        m
    }

    /// REQ-AXO-902503 — LA garde : les deux mondes doivent RENDRE DIFFÉREMMENT.
    ///
    /// C'est l'assertion que le comportement d'avant ne peut pas satisfaire : avant,
    /// les écartés n'étaient rendus nulle part, donc « projet vide » et « projet
    /// entièrement livré » produisaient la même chaîne — vide. SWT a failli déposer
    /// un faux rapport de bug sur cette confusion, et c'est le coût réel du défaut.
    ///
    /// Un test qui vérifierait seulement « la ligne apparaît quand il y a des nœuds
    /// terminaux » passerait sans rien prouver de l'ambiguïté.
    #[test]
    fn un_projet_vide_et_un_projet_tout_livre_ne_rendent_pas_le_meme_texte() {
        let vide = render_ecartes_section(&BTreeMap::new(), false);
        let tout_livre = render_ecartes_section(&livres(4), false);

        assert_ne!(
            vide, tout_livre,
            "« rien n'existe » et « tout est fini » DOIVENT se distinguer — c'est tout le défaut"
        );
        assert!(
            vide.is_empty(),
            "un projet sans écarté n'ajoute rien : le coût est nul quand il n'y a rien à dire, got: {vide:?}"
        );
        assert!(
            tout_livre.contains('4') && tout_livre.contains("delivered"),
            "l'écart doit être CHIFFRÉ et son motif NOMMÉ, got: {tout_livre}"
        );
    }

    /// REQ-AXO-902503 + précédent REQ-AXO-902443 — le compte survit toujours, les ids
    /// non. Le compte est ce qui lève l'ambiguïté ; les ids sont du volume, et cette
    /// session combat le volume par ailleurs (Vague 3).
    #[test]
    fn le_compte_est_toujours_rendu_les_ids_seulement_en_verbose() {
        let brief = render_ecartes_section(&livres(40), false);
        let verbeux = render_ecartes_section(&livres(40), true);

        assert!(
            brief.contains("40"),
            "le compte doit survivre au mode brief, got: {brief}"
        );
        assert!(
            !brief.contains("REQ-TST-000"),
            "le mode brief ne doit PAS lister les ids, got: {brief}"
        );
        assert!(
            verbeux.contains("REQ-TST-000"),
            "le mode verbose doit les lister, got: {verbeux}"
        );
        let rendus = verbeux.matches("REQ-TST-").count();
        assert!(
            rendus <= 24,
            "la liste doit rester bornée même en verbose, {rendus} ids rendus"
        );
        assert!(
            verbeux.contains("sur 40"),
            "et le TOTAL doit rester exact à côté de la liste tronquée, got: {verbeux}"
        );
    }
}
