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

/// REQ-AXO-902455 axe 1 — de quel côté du sujet l'arête est cherchée.
///
/// `Outgoing` (défaut) répond à « ce nœud pointe-t-il vers … ». `Incoming`
/// répond à « quelque chose pointe-t-il vers ce nœud », que la forme sortante ne
/// peut PAS exprimer : OPV (`llm_feedback` #96) demande « 105 nœuds retirés,
/// rien n'exige d'enregistrer le remplaçant » — c'est-à-dire *un nœud
/// `superseded` doit recevoir une arête `SUPERSEDES`*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EdgeDirection {
    #[default]
    Outgoing,
    Incoming,
}

impl EdgeDirection {
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "outgoing" | "out" | "from" => Some(Self::Outgoing),
            "incoming" | "in" | "to" => Some(Self::Incoming),
            _ => None,
        }
    }
}

/// Ce que la règle exige ou interdit du sujet.
///
/// UN prédicat par règle, jamais deux : une règle qui en combinerait plusieurs
/// n'aurait pas de sens de violation univoque, et son message ne pourrait pas
/// dire lequel a échoué. `parse_soll_rule` refuse la combinaison plutôt que de
/// choisir en silence.
#[derive(Clone, Debug)]
pub enum RulePredicate {
    /// Axe historique — une arête (dans `direction`) vers/depuis un nœud
    /// correspondant à `other` est interdite (`Forbidden`) ou requise
    /// (`Required`).
    Edge {
        mode: RuleMode,
        direction: EdgeDirection,
        other: EndpointMatcher,
        relations: Vec<String>,
    },
    /// Axe 2 — aucune preuve du sujet ne doit porter l'un de ces
    /// `artifact_status`. Répond à VPC : « empêcher `delivered` quand une preuve
    /// est passée `missing` » — 485 preuves sur 636 n'avaient jamais été
    /// vérifiées. La donnée était déjà dans le snapshot ; elle n'était pas
    /// atteignable depuis le schéma.
    ForbiddenEvidenceStatus {
        statuses: Vec<String>,
        /// Restreint aux preuves de ces types. Vide = tous.
        artifact_types: Vec<String>,
    },
    /// Axe 3 — ces clés de métadonnées doivent être présentes ET non vides.
    /// C'est la forme de `uncovered_requirements`, jusqu'ici en dur.
    RequiredMetadata { keys: Vec<String> },
}

/// Une règle SOLL déclarative. `id`/`title` portent la Guideline qui la
/// gouverne, pour qu'une violation se rattache à son intention.
#[derive(Clone, Debug)]
pub struct SollRule {
    pub id: String,
    pub title: String,
    /// Les nœuds auxquels la règle s'applique.
    pub subject: EndpointMatcher,
    pub predicate: RulePredicate,
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
///
/// `None` quand la règle est inévaluable : `mode` manquant/inconnu, ou DEUX
/// prédicats combinés. Une règle que personne ne peut évaluer est ÉCARTÉE,
/// jamais traitée comme permissive.
///
/// Le sujet se déclare par `subject_kind` / `subject_status_in` /
/// `subject_status_not_in`. Les noms historiques `source_*` restent acceptés :
/// `GUI-PRO-119` et `GUI-PRO-120` sont livrées sous cette forme et un tenant a
/// pu en écrire d'autres.
pub fn parse_soll_rule(id: &str, title: &str, v: &serde_json::Value) -> Option<SollRule> {
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
    let text = |key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let first_list = |a: &str, b: &str| -> Vec<String> {
        let primary = string_list(a);
        if primary.is_empty() {
            string_list(b)
        } else {
            primary
        }
    };

    let subject = EndpointMatcher {
        kind: text("subject_kind").or_else(|| text("source_kind")),
        status: StatusMatcher {
            any_of: first_list("subject_status_in", "source_status_in"),
            none_of: first_list("subject_status_not_in", "source_status_not_in"),
        },
    };

    // UN prédicat, jamais deux : sinon la violation n'a pas de sens univoque.
    let evidence_statuses = string_list("evidence_status_in");
    let metadata_keys = string_list("metadata_required");
    let declared = [!evidence_statuses.is_empty(), !metadata_keys.is_empty()];
    if declared.iter().filter(|d| **d).count() > 1 {
        return None;
    }

    let predicate = if !evidence_statuses.is_empty() {
        RulePredicate::ForbiddenEvidenceStatus {
            statuses: evidence_statuses,
            artifact_types: string_list("evidence_artifact_types"),
        }
    } else if !metadata_keys.is_empty() {
        RulePredicate::RequiredMetadata { keys: metadata_keys }
    } else {
        // Forme historique : une contrainte d'arête. `mode` y est obligatoire.
        let mode = RuleMode::from_str_ci(v.get("mode")?.as_str()?)?;
        let direction = v
            .get("direction")
            .and_then(|x| x.as_str())
            .map(EdgeDirection::from_str_ci)
            .unwrap_or(Some(EdgeDirection::Outgoing))?;
        RulePredicate::Edge {
            mode,
            direction,
            other: EndpointMatcher {
                kind: text("other_kind").or_else(|| text("target_kind")),
                status: StatusMatcher {
                    any_of: first_list("other_status_in", "target_status_in"),
                    none_of: first_list("other_status_not_in", "target_status_not_in"),
                },
            },
            relations: string_list("relations"),
        }
    };

    Some(SollRule {
        id: id.to_string(),
        title: title.to_string(),
        subject,
        predicate,
        message: text("message"),
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

/// Évalue une règle. O(arêtes) pour une contrainte d'arête `Forbidden`,
/// O(nœuds + arêtes) sinon.
///
/// Un nœud absent du snapshot est IGNORÉ, jamais supposé : son extrémité est
/// hors du périmètre chargé, et lui prêter un statut serait en inventer un.
pub fn evaluate_rule(snapshot: &SollSnapshot, rule: &SollRule) -> Vec<SollRuleViolation> {
    let facts = node_facts(snapshot);
    evaluate_rule_with_facts(snapshot, rule, &facts)
}

fn violation(rule: &SollRule, subject: &str) -> SollRuleViolation {
    SollRuleViolation {
        rule_id: rule.id.clone(),
        source_id: subject.to_string(),
        target_id: None,
        relation: None,
        message: rule.message.clone(),
    }
}

/// Les sujets de la règle, triés — un rapport dont l'ordre change d'un appel à
/// l'autre ne se compare pas (même raison que REQ-AXO-902452).
fn subjects<'a>(rule: &SollRule, facts: &NodeFacts<'a>) -> Vec<&'a str> {
    let mut out: Vec<&str> = facts
        .iter()
        .filter(|(_, (kind, status))| rule.subject.matches(kind, status))
        .map(|(id, _)| *id)
        .collect();
    out.sort_unstable();
    out
}

fn evaluate_rule_with_facts(
    snapshot: &SollSnapshot,
    rule: &SollRule,
    facts: &NodeFacts<'_>,
) -> Vec<SollRuleViolation> {
    let mut out = Vec::new();
    match &rule.predicate {
        RulePredicate::Edge {
            mode,
            direction,
            other,
            relations,
        } => {
            let rel_ok = |rel: &str| relations.is_empty() || relations.iter().any(|r| r == rel);
            // Selon la direction, le SUJET est l'une ou l'autre extrémité.
            fn ends<'e>(
                edge: &'e super::snapshot::SnapshotEdge,
                direction: EdgeDirection,
            ) -> (&'e str, &'e str) {
                match direction {
                    EdgeDirection::Outgoing => (edge.source_id.as_str(), edge.target_id.as_str()),
                    EdgeDirection::Incoming => (edge.target_id.as_str(), edge.source_id.as_str()),
                }
            }
            match mode {
                RuleMode::Forbidden => {
                    for edge in &snapshot.edges {
                        if !rel_ok(&edge.relation_type) {
                            continue;
                        }
                        let (subject_id, other_id) = ends(edge, *direction);
                        let (Some((skind, sstatus)), Some((okind, ostatus))) =
                            (facts.get(subject_id), facts.get(other_id))
                        else {
                            continue;
                        };
                        if rule.subject.matches(skind, sstatus) && other.matches(okind, ostatus) {
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
                    // Une règle `required` sans sélecteur de sujet exigerait
                    // l'arête de CHAQUE nœud du graphe. Ce n'est jamais
                    // l'intention de l'auteur, et ça noierait le rapport.
                    if rule.subject.is_unconstrained() {
                        return out;
                    }
                    let mut satisfied: HashMap<&str, bool> =
                        subjects(rule, facts).into_iter().map(|id| (id, false)).collect();
                    for edge in &snapshot.edges {
                        if !rel_ok(&edge.relation_type) {
                            continue;
                        }
                        let (subject_id, other_id) = ends(edge, *direction);
                        if !satisfied.contains_key(subject_id) {
                            continue;
                        }
                        let Some((okind, ostatus)) = facts.get(other_id) else {
                            continue;
                        };
                        if other.matches(okind, ostatus) {
                            satisfied.insert(subject_id, true);
                        }
                    }
                    let mut offenders: Vec<&str> = satisfied
                        .into_iter()
                        .filter(|(_, ok)| !*ok)
                        .map(|(id, _)| id)
                        .collect();
                    offenders.sort_unstable();
                    out.extend(offenders.into_iter().map(|id| violation(rule, id)));
                }
            }
        }
        RulePredicate::ForbiddenEvidenceStatus {
            statuses,
            artifact_types,
        } => {
            let subjects: std::collections::HashSet<&str> =
                subjects(rule, facts).into_iter().collect();
            for trace in &snapshot.traceability {
                if !subjects.contains(trace.soll_entity_id.as_str()) {
                    continue;
                }
                if !artifact_types.is_empty()
                    && !artifact_types
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(&trace.artifact_type))
                {
                    continue;
                }
                if statuses.iter().any(|s| s == &trace.artifact_status) {
                    out.push(SollRuleViolation {
                        rule_id: rule.id.clone(),
                        source_id: trace.soll_entity_id.clone(),
                        target_id: Some(trace.artifact_ref.clone()),
                        relation: Some(format!("evidence:{}", trace.artifact_status)),
                        message: rule.message.clone(),
                    });
                }
            }
        }
        RulePredicate::RequiredMetadata { keys } => {
            if rule.subject.is_unconstrained() {
                return out;
            }
            for id in subjects(rule, facts) {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                let parsed: serde_json::Value =
                    serde_json::from_str(&node.metadata_raw).unwrap_or(serde_json::Value::Null);
                let missing = keys.iter().any(|key| {
                    match parsed.get(key) {
                        None | Some(serde_json::Value::Null) => true,
                        Some(serde_json::Value::String(text)) => text.trim().is_empty(),
                        Some(serde_json::Value::Array(items)) => items.is_empty(),
                        Some(_) => false,
                    }
                });
                if missing {
                    out.push(violation(rule, id));
                }
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
        snapshot_with_evidence(nodes, edges, Vec::new())
    }

    fn snapshot_with_evidence(
        nodes: Vec<SnapshotNode>,
        edges: Vec<SnapshotEdge>,
        traceability: Vec<crate::soll_snapshot::SnapshotTraceability>,
    ) -> SollSnapshot {
        let map: HashMap<String, SnapshotNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        SollSnapshot::build("TST", 1, map, edges, traceability)
    }

    fn node_with_metadata(id: &str, entity_type: &str, status: &str, meta: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: entity_type.to_string(),
            title: id.to_string(),
            status: status.to_string(),
            metadata_raw: meta.to_string(),
        }
    }

    fn evidence(entity_id: &str, artifact_type: &str, r#ref: &str, status: &str)
        -> crate::soll_snapshot::SnapshotTraceability
    {
        crate::soll_snapshot::SnapshotTraceability {
            id: format!("{entity_id}:{ref_}", ref_ = r#ref),
            soll_entity_type: "requirement".to_string(),
            soll_entity_id: entity_id.to_string(),
            artifact_type: artifact_type.to_string(),
            artifact_ref: r#ref.to_string(),
            artifact_status: status.to_string(),
        }
    }

    /// AXE 1 — REQ-AXO-902455, demandé par OPV (`llm_feedback` #96) : « 105 nœuds
    /// retirés, rien n'exige d'enregistrer ce qui les remplace ». C'est une règle
    /// sur les arêtes ENTRANTES ; la forme sortante ne peut pas l'exprimer.
    #[test]
    fn an_incoming_rule_flags_a_retired_node_that_nothing_replaces() {
        let rule = parse_soll_rule(
            "GUI-TST-010",
            "Un nœud retiré dit ce qui le remplace",
            &json!({
                "mode": "required",
                "direction": "incoming",
                "subject_status_in": ["superseded"],
                "relations": ["SUPERSEDES"],
                "message": "retiré sans remplaçant enregistré"
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("REQ-TST-001", "Requirement", "current"),
                node("REQ-TST-002", "Requirement", "superseded"), // remplacé : OK
                node("REQ-TST-003", "Requirement", "superseded"), // orphelin : violation
            ],
            vec![edge("REQ-TST-001", "REQ-TST-002", "SUPERSEDES")],
        );
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source_id, "REQ-TST-003");
        // Contrôle positif : celui QUI a un remplaçant n'est pas signalé, et le
        // nœud `current` n'est pas sujet du tout.
        assert!(!found.iter().any(|v| v.source_id == "REQ-TST-002"));
        assert!(!found.iter().any(|v| v.source_id == "REQ-TST-001"));
    }

    /// La MÊME règle en sortante ne dit pas la même chose — c'est ce qui prouve
    /// que `direction` porte du sens et n'est pas un champ décoratif.
    #[test]
    fn the_same_rule_outgoing_does_not_mean_the_same_thing() {
        let outgoing = parse_soll_rule(
            "GUI-TST-011",
            "sortante",
            &json!({
                "mode": "required",
                "subject_status_in": ["superseded"],
                "relations": ["SUPERSEDES"]
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("REQ-TST-001", "Requirement", "current"),
                node("REQ-TST-002", "Requirement", "superseded"),
            ],
            vec![edge("REQ-TST-001", "REQ-TST-002", "SUPERSEDES")],
        );
        // En sortante, REQ-002 est signalé (il ne supersède rien) — l'INVERSE du
        // verdict entrant, où il est le seul à être correctement remplacé.
        let found = evaluate_rule(&snap, &outgoing);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_id, "REQ-TST-002");
    }

    /// AXE 2 — VPC : « empêcher `delivered` quand une preuve est passée
    /// `missing` ». 485 preuves sur 636 n'avaient jamais été vérifiées ; la
    /// donnée était dans le snapshot, inatteignable depuis le schéma.
    #[test]
    fn an_evidence_rule_flags_a_delivered_requirement_whose_proof_went_missing() {
        let rule = parse_soll_rule(
            "GUI-TST-012",
            "Une exigence livrée n'est pas prouvée par un fichier disparu",
            &json!({
                "subject_kind": "Requirement",
                "subject_status_in": ["delivered"],
                "evidence_status_in": ["broken"],
                "message": "preuve absente sous une exigence livrée"
            }),
        )
        .unwrap();
        let snap = snapshot_with_evidence(
            vec![
                node("REQ-TST-001", "Requirement", "delivered"),
                node("REQ-TST-002", "Requirement", "delivered"),
                node("REQ-TST-003", "Requirement", "current"),
            ],
            vec![],
            vec![
                evidence("REQ-TST-001", "file", "src/parti.rs", "broken"),
                evidence("REQ-TST-002", "file", "src/present.rs", "present"),
                // `current` avec une preuve cassée : hors sujet, la règle vise
                // les LIVRÉES.
                evidence("REQ-TST-003", "file", "src/autre.rs", "broken"),
            ],
        );
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].source_id, "REQ-TST-001");
        assert_eq!(found[0].target_id.as_deref(), Some("src/parti.rs"));
        assert!(found[0].render().contains("GUI-TST-012"));
    }

    /// Le filtre par type de preuve : une règle qui ne vise que les fichiers ne
    /// doit pas rougir sur une métrique cassée.
    #[test]
    fn an_evidence_rule_can_be_restricted_to_one_artifact_type() {
        let rule = parse_soll_rule(
            "GUI-TST-013",
            "fichiers seulement",
            &json!({
                "subject_kind": "Requirement",
                "evidence_status_in": ["broken"],
                "evidence_artifact_types": ["file"]
            }),
        )
        .unwrap();
        let snap = snapshot_with_evidence(
            vec![node("REQ-TST-001", "Requirement", "delivered")],
            vec![],
            vec![evidence("REQ-TST-001", "metric", "p99_latency", "broken")],
        );
        assert!(
            evaluate_rule(&snap, &rule).is_empty(),
            "une métrique n'est pas un fichier"
        );
    }

    /// AXE 3 — la forme de `uncovered_requirements`, jusqu'ici en dur.
    #[test]
    fn a_metadata_rule_flags_a_requirement_without_acceptance_criteria() {
        let rule = parse_soll_rule(
            "GUI-TST-014",
            "Une exigence porte ses critères d'acceptation",
            &json!({
                "subject_kind": "Requirement",
                "metadata_required": ["acceptance_criteria"]
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node_with_metadata("REQ-TST-001", "Requirement", "current", "{}"),
                node_with_metadata(
                    "REQ-TST-002",
                    "Requirement",
                    "current",
                    r#"{"acceptance_criteria": []}"#,
                ),
                node_with_metadata(
                    "REQ-TST-003",
                    "Requirement",
                    "current",
                    r#"{"acceptance_criteria": ["le test passe"]}"#,
                ),
            ],
            vec![],
        );
        let found = evaluate_rule(&snap, &rule);
        let flagged: Vec<&str> = found.iter().map(|v| v.source_id.as_str()).collect();
        assert_eq!(
            flagged,
            vec!["REQ-TST-001", "REQ-TST-002"],
            "absent ET vide comptent tous deux comme manquants ; renseigné ne compte pas"
        );
    }

    /// Une règle qui combine DEUX prédicats n'a pas de sens de violation
    /// univoque : son message ne pourrait pas dire lequel a échoué. Refusée au
    /// parse plutôt que tranchée en silence.
    #[test]
    fn a_rule_combining_two_predicates_is_refused_rather_than_silently_resolved() {
        assert!(parse_soll_rule(
            "GUI-TST-015",
            "x",
            &json!({
                "subject_kind": "Requirement",
                "evidence_status_in": ["missing"],
                "metadata_required": ["acceptance_criteria"]
            }),
        )
        .is_none());
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
