//! REQ-AXO-902455 — declarative rule engine for the SOLL contract.
//!
//! RÉUTILISE : `soll_snapshot::snapshot::SollSnapshot` (nodes + edges + status,
//! déjà chargés en RAM) pour les données ; le PATRON de
//! `ist_snapshot::structural_invariants` (mode forbidden/required, matcher
//! d'endpoint, évaluateur pur, join règle→intention) ; `tools_governance::
//! axon_structural_invariants` pour le chargement depuis les Guideline.
//! Le code de ce dernier n'est PAS partageable : il évalue un `IstGraph` (CSR
//! compact, `NodeKind`/`RelationType` en enums) là où le SOLL est un
//! `HashMap<String, SnapshotNode>` à types et relations textuels — vérifié via
//! `axon query "évaluer une règle déclarative sur le graphe SOLL"` et
//! `grep "fn evaluate_"` : aucun évaluateur ne couvre le graphe SOLL.
//!
//! `DEC-AXO-901652` prescrit que l'enforcement du contrat LLM-SOLL soit un
//! **ruleset déclaratif évalué côté serveur**, le ruleset vivant « comme
//! DONNÉE, versionnable, auditable ». Elle n'avait jamais été exécutée : chaque
//! invariant SOLL était une branche Rust en dur, donc chaque règle demandée par
//! un tenant coûtait une modification du cœur et un promote. `REQ-AXO-902453`
//! — la règle réclamée par TE2 — est le cas qui l'a rendu concret.
//!
//! `DEC-AXO-901649` interdit le DSL de requête général, et la frontière est
//! tenue : une règle sélectionne ses extrémités par *kind* et par *statut*, et
//! contraint un ensemble de relations. Rien d'autre. Les prédicats qui lisent
//! les métadonnées d'un nœud ou comparent des nœuds deux à deux
//! (`uncovered_requirements`, `duplicate_titles`) restent en code, exprès.

use std::collections::HashMap;

use super::snapshot::SollSnapshot;

/// Whether the rule forbids the edge pattern or requires it to exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleMode {
    /// No matching source may carry a qualifying edge to a matching target.
    /// Each offending edge is one violation.
    Forbidden,
    /// Every matching source MUST carry at least one qualifying edge to a
    /// matching target. A source with none is one violation.
    Required,
}

impl RuleMode {
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "forbidden" | "forbid" | "deny" => Some(Self::Forbidden),
            "required" | "require" | "must" => Some(Self::Required),
            _ => None,
        }
    }
}

/// Sélectionne une extrémité par son statut de cycle de vie.
///
/// Ces deux listes sont ce qui permet à une règle de parler d'un ATTRIBUT et
/// non seulement d'une arête — l'extension minimale par rapport à
/// `REQ-AXO-157`, et elle est nécessaire : la seule règle qu'un tenant ait
/// réellement demandée (« une arête `SUPERSEDES` dont la cible est encore
/// ouverte ») est inexprimable sans elle, et un moteur qui n'exprime aucune
/// règle existante est livré vide. C'est exactement ce qui est arrivé à son
/// jumeau IST : **0 Guideline sur 258** en portait une au moment d'écrire ceci.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusMatcher {
    /// Le statut doit être l'un de ceux-ci. Vide = aucune contrainte.
    pub any_of: Vec<String>,
    /// Le statut ne doit être aucun de ceux-ci. Vide = aucune contrainte.
    pub none_of: Vec<String>,
}

impl StatusMatcher {
    fn matches(&self, status: &str) -> bool {
        if !self.any_of.is_empty() && !self.any_of.iter().any(|s| s == status) {
            return false;
        }
        if self.none_of.iter().any(|s| s == status) {
            return false;
        }
        true
    }

    fn is_unconstrained(&self) -> bool {
        self.any_of.is_empty() && self.none_of.is_empty()
    }
}

/// Sélectionne une extrémité de l'arête : type d'entité SOLL + statut.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointMatcher {
    /// Type d'entité SOLL (« Requirement », « Milestone »…). `None` = tout type.
    pub kind: Option<String>,
    pub status: StatusMatcher,
}

impl EndpointMatcher {
    fn matches(&self, entity_type: &str, status: &str) -> bool {
        if let Some(kind) = &self.kind {
            if !kind.eq_ignore_ascii_case(entity_type) {
                return false;
            }
        }
        self.status.matches(status)
    }

    fn is_unconstrained(&self) -> bool {
        self.kind.is_none() && self.status.is_unconstrained()
    }
}

/// Une règle SOLL déclarative. `id`/`title` portent la Guideline qui la
/// gouverne, pour qu'une violation se rattache à son intention.
#[derive(Clone, Debug)]
pub struct SollRule {
    pub id: String,
    pub title: String,
    pub mode: RuleMode,
    pub source: EndpointMatcher,
    pub target: EndpointMatcher,
    /// Types de relation contraints. Vide = toute relation.
    pub relations: Vec<String>,
    /// Phrase rendue avec chaque violation. Sans elle le lecteur reçoit une
    /// correspondance de motif et aucune idée de quoi faire — c'est le « un
    /// compteur n'est pas un rapport » que ce dépôt corrige en boucle
    /// (REQ-AXO-902409).
    pub message: Option<String>,
}

/// Une violation détectée, portant l'id de règle pour la jointure SOLL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SollRuleViolation {
    pub rule_id: String,
    pub source_id: String,
    /// La cible fautive pour `Forbidden`. `None` pour `Required` (la source ne
    /// porte AUCUNE arête qualifiante).
    pub target_id: Option<String>,
    pub relation: Option<String>,
    pub message: Option<String>,
}

impl SollRuleViolation {
    /// Une ligne pour le lecteur. L'id de règle y est toujours : c'est lui qui
    /// transforme « c'est interdit » en « c'est interdit PARCE QUE <intention> ».
    pub fn render(&self) -> String {
        let edge = match (&self.target_id, &self.relation) {
            (Some(target), Some(rel)) => format!("{} -{}-> {}", self.source_id, rel, target),
            (Some(target), None) => format!("{} -> {}", self.source_id, target),
            (None, _) => format!("{} (aucune arête qualifiante)", self.source_id),
        };
        match &self.message {
            Some(msg) => format!("{edge} — {msg} [{}]", self.rule_id),
            None => format!("{edge} [{}]", self.rule_id),
        }
    }
}

/// Parse un objet de règle (metadata de Guideline ou entrée `rules[]` inline).
/// `None` quand `mode` manque ou n'est pas reconnu : une règle que personne ne
/// peut évaluer est ÉCARTÉE, pas traitée comme permissive.
pub fn parse_soll_rule(id: &str, title: &str, v: &serde_json::Value) -> Option<SollRule> {
    let mode = RuleMode::from_str_ci(v.get("mode")?.as_str()?)?;
    let string_list = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let kind_field = |key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(SollRule {
        id: id.to_string(),
        title: title.to_string(),
        mode,
        source: EndpointMatcher {
            kind: kind_field("source_kind"),
            status: StatusMatcher {
                any_of: string_list("source_status_in"),
                none_of: string_list("source_status_not_in"),
            },
        },
        target: EndpointMatcher {
            kind: kind_field("target_kind"),
            status: StatusMatcher {
                any_of: string_list("target_status_in"),
                none_of: string_list("target_status_not_in"),
            },
        },
        relations: string_list("relations"),
        message: v
            .get("message")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

/// `(entity_type, status)` de chaque nœud, par id. Construit une fois par balayage.
type NodeFacts<'a> = HashMap<&'a str, (&'a str, &'a str)>;

fn node_facts(snapshot: &SollSnapshot) -> NodeFacts<'_> {
    snapshot
        .nodes
        .iter()
        .map(|(id, node)| {
            (
                id.as_str(),
                (node.entity_type.as_str(), node.status.as_str()),
            )
        })
        .collect()
}

/// Évalue une règle. O(arêtes) pour `Forbidden`, O(nœuds + arêtes) pour
/// `Required`.
///
/// Un nœud absent du snapshot est IGNORÉ, jamais supposé : son extrémité est
/// hors du périmètre chargé, et lui prêter un statut serait en inventer un.
pub fn evaluate_rule(snapshot: &SollSnapshot, rule: &SollRule) -> Vec<SollRuleViolation> {
    let facts = node_facts(snapshot);
    evaluate_rule_with_facts(snapshot, rule, &facts)
}

fn evaluate_rule_with_facts(
    snapshot: &SollSnapshot,
    rule: &SollRule,
    facts: &NodeFacts<'_>,
) -> Vec<SollRuleViolation> {
    let rel_ok = |rel: &str| rule.relations.is_empty() || rule.relations.iter().any(|r| r == rel);
    let mut out = Vec::new();

    match rule.mode {
        RuleMode::Forbidden => {
            for edge in &snapshot.edges {
                if !rel_ok(&edge.relation_type) {
                    continue;
                }
                let (Some((skind, sstatus)), Some((tkind, tstatus))) = (
                    facts.get(edge.source_id.as_str()),
                    facts.get(edge.target_id.as_str()),
                ) else {
                    continue;
                };
                if rule.source.matches(skind, sstatus) && rule.target.matches(tkind, tstatus) {
                    out.push(SollRuleViolation {
                        rule_id: rule.id.clone(),
                        source_id: edge.source_id.clone(),
                        target_id: Some(edge.target_id.clone()),
                        relation: Some(edge.relation_type.clone()),
                        message: rule.message.clone(),
                    });
                }
            }
        }
        RuleMode::Required => {
            // Une règle `required` sans sélecteur de source exigerait l'arête de
            // CHAQUE nœud du graphe. Ce n'est jamais l'intention de l'auteur, et
            // ça noierait le rapport sous des milliers de lignes.
            if rule.source.is_unconstrained() {
                return out;
            }
            let mut satisfied: HashMap<&str, bool> = HashMap::new();
            for (id, (kind, status)) in facts.iter() {
                if rule.source.matches(kind, status) {
                    satisfied.insert(*id, false);
                }
            }
            for edge in &snapshot.edges {
                if !rel_ok(&edge.relation_type) {
                    continue;
                }
                if !satisfied.contains_key(edge.source_id.as_str()) {
                    continue;
                }
                let Some((tkind, tstatus)) = facts.get(edge.target_id.as_str()) else {
                    continue;
                };
                if rule.target.matches(tkind, tstatus) {
                    satisfied.insert(edge.source_id.as_str(), true);
                }
            }
            let mut offenders: Vec<&str> = satisfied
                .into_iter()
                .filter(|(_, ok)| !*ok)
                .map(|(id, _)| id)
                .collect();
            // Trié : un rapport qui change d'ordre d'un appel à l'autre ne se
            // compare pas (même raison que REQ-AXO-902452).
            offenders.sort_unstable();
            for id in offenders {
                out.push(SollRuleViolation {
                    rule_id: rule.id.clone(),
                    source_id: id.to_string(),
                    target_id: None,
                    relation: None,
                    message: rule.message.clone(),
                });
            }
        }
    }
    out
}

/// Évalue un lot en partageant un seul index de faits entre toutes les règles.
pub fn evaluate_all(snapshot: &SollSnapshot, rules: &[SollRule]) -> Vec<SollRuleViolation> {
    let facts = node_facts(snapshot);
    rules
        .iter()
        .flat_map(|rule| evaluate_rule_with_facts(snapshot, rule, &facts))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soll_snapshot::snapshot::{SnapshotEdge, SnapshotNode};
    use serde_json::json;

    fn node(id: &str, entity_type: &str, status: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: entity_type.to_string(),
            title: id.to_string(),
            status: status.to_string(),
            metadata_raw: "{}".to_string(),
        }
    }

    fn edge(source: &str, target: &str, relation: &str) -> SnapshotEdge {
        SnapshotEdge {
            source_id: source.to_string(),
            target_id: target.to_string(),
            relation_type: relation.to_string(),
        }
    }

    fn snapshot(nodes: Vec<SnapshotNode>, edges: Vec<SnapshotEdge>) -> SollSnapshot {
        let map: HashMap<String, SnapshotNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        SollSnapshot::build("TST", 1, map, edges, Vec::new())
    }

    /// La règle demandée par TE2, exprimée en DONNÉE et non en branche Rust.
    fn supersedes_rule() -> SollRule {
        parse_soll_rule(
            "GUI-TST-001",
            "Une supersession retire sa cible",
            &json!({
                "mode": "forbidden",
                "relations": ["SUPERSEDES"],
                "target_status_not_in": ["superseded"],
                "message": "la cible est encore ouverte"
            }),
        )
        .expect("la règle doit se parser")
    }

    #[test]
    fn a_supersedes_edge_whose_target_is_still_open_is_a_violation() {
        let snap = snapshot(
            vec![
                node("MIL-TST-001", "Milestone", "current"),
                node("MIL-TST-002", "Milestone", "current"),
            ],
            vec![edge("MIL-TST-001", "MIL-TST-002", "SUPERSEDES")],
        );
        let found = evaluate_rule(&snap, &supersedes_rule());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule_id, "GUI-TST-001");
        assert_eq!(found[0].target_id.as_deref(), Some("MIL-TST-002"));
        // Le rendu porte l'id de règle : c'est lui qui rend l'intention
        // atteignable via `soll_get`, et c'est la valeur que DEC-AXO-901649 nomme.
        let line = found[0].render();
        assert!(line.contains("GUI-TST-001"), "{line}");
        assert!(line.contains("la cible est encore ouverte"), "{line}");
    }

    /// Contrôle positif : sans lui, une règle qui signalerait TOUTE arête
    /// `SUPERSEDES` passerait au vert.
    #[test]
    fn a_coherent_supersession_is_not_a_violation() {
        let snap = snapshot(
            vec![
                node("MIL-TST-001", "Milestone", "current"),
                node("MIL-TST-002", "Milestone", "superseded"),
            ],
            vec![edge("MIL-TST-001", "MIL-TST-002", "SUPERSEDES")],
        );
        assert!(evaluate_rule(&snap, &supersedes_rule()).is_empty());
    }

    /// Les 7 cas sur 10 mesurés sur AXO : la SOURCE est retirée, la cible est
    /// vivante. Deux règles séparent ce qu'une branche `if` distinguait — et
    /// chacune pointe alors SA Guideline, donc SA réparation.
    #[test]
    fn source_status_separates_an_inverted_edge_from_a_forgotten_target() {
        let inverted = parse_soll_rule(
            "GUI-TST-002",
            "Arête SUPERSEDES inversée",
            &json!({
                "mode": "forbidden",
                "relations": ["SUPERSEDES"],
                "source_status_in": ["superseded", "rejected"],
                "target_status_not_in": ["superseded"],
                "message": "la source est retirée : l'arête part du mauvais bout"
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("PIL-TST-006", "Pillar", "superseded"),
                node("PIL-TST-004", "Pillar", "current"),
                node("MIL-TST-001", "Milestone", "current"),
                node("MIL-TST-002", "Milestone", "current"),
            ],
            vec![
                edge("PIL-TST-006", "PIL-TST-004", "SUPERSEDES"),
                edge("MIL-TST-001", "MIL-TST-002", "SUPERSEDES"),
            ],
        );
        let found = evaluate_rule(&snap, &inverted);
        assert_eq!(found.len(), 1, "seule l'arête à source retirée : {found:?}");
        assert_eq!(found[0].source_id, "PIL-TST-006");
    }

    #[test]
    fn a_required_rule_flags_the_source_that_carries_no_qualifying_edge() {
        let rule = parse_soll_rule(
            "GUI-TST-003",
            "Une Validation VERIFIE quelque chose",
            &json!({
                "mode": "required",
                "source_kind": "Validation",
                "relations": ["VERIFIES"]
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("VAL-TST-001", "Validation", "current"),
                node("VAL-TST-002", "Validation", "current"),
                node("REQ-TST-001", "Requirement", "current"),
            ],
            vec![edge("VAL-TST-001", "REQ-TST-001", "VERIFIES")],
        );
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source_id, "VAL-TST-002");
        assert!(found[0].target_id.is_none());
    }

    /// Une règle `required` sans sélecteur de source exigerait l'arête de CHAQUE
    /// nœud. On ne l'évalue pas, plutôt que de rendre des milliers de lignes
    /// qu'aucun lecteur ne peut utiliser.
    #[test]
    fn an_unconstrained_required_rule_is_not_evaluated_rather_than_flooding() {
        let rule =
            parse_soll_rule("GUI-TST-004", "trop large", &json!({ "mode": "required" })).unwrap();
        let snap = snapshot(
            vec![
                node("REQ-TST-001", "Requirement", "current"),
                node("REQ-TST-002", "Requirement", "current"),
            ],
            vec![],
        );
        assert!(evaluate_rule(&snap, &rule).is_empty());
    }

    /// Un nœud hors du périmètre chargé n'a PAS de statut connu. L'ignorer est
    /// la seule lecture honnête — lui en prêter un fabriquerait une violation,
    /// ou en masquerait une.
    #[test]
    fn an_endpoint_outside_the_snapshot_is_skipped_not_assumed() {
        let snap = snapshot(
            vec![node("MIL-TST-001", "Milestone", "current")],
            vec![edge("MIL-TST-001", "MIL-OTHER-999", "SUPERSEDES")],
        );
        assert!(evaluate_rule(&snap, &supersedes_rule()).is_empty());
    }

    #[test]
    fn a_rule_without_a_recognised_mode_is_dropped_not_treated_as_permissive() {
        assert!(
            parse_soll_rule("GUI-TST-005", "x", &json!({ "relations": ["SUPERSEDES"] })).is_none()
        );
        assert!(parse_soll_rule("GUI-TST-006", "x", &json!({ "mode": "peut-être" })).is_none());
    }
}
