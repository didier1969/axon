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
//! `DEC-AXO-901673` (qui supersède `DEC-AXO-901649`) redéfinit la frontière au
//! lieu de la supprimer : une règle sélectionne ses extrémités par *kind* et par
//! *statut*, contraint un ensemble de relations, lit une métadonnée, ou compare
//! les sujets entre eux (unicité, agrégat, atteignabilité). Restent interdits :
//! variables liées, jointures arbitraires, récursion définie par l'utilisateur,
//! et la combinaison de deux prédicats dans une même règle.
//!
//! `duplicate_titles` a migré ici (`GUI-PRO-121`) et son SQL a été RETIRÉ.
//! `uncovered_requirements` reste en code : c'est une CONJONCTION — ni preuve ni
//! critère d'acceptation — que `parse_soll_rule` refuse par construction.

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
    /// L'arête compte de QUELQUE côté qu'elle se trouve. Nécessaire pour les
    /// invariants de rattachement — « ce nœud est-il relié au graphe » — qu'un
    /// seul sens ne peut pas exprimer : deux règles `outgoing` + `incoming`
    /// produiraient DEUX violations pour un nœud isolé, et une violation à tort
    /// pour un nœud rattaché d'un seul côté.
    Either,
}

impl EdgeDirection {
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "outgoing" | "out" | "from" => Some(Self::Outgoing),
            "incoming" | "in" | "to" => Some(Self::Incoming),
            "either" | "any" | "both" => Some(Self::Either),
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
    /// Axe 4 — deux sujets ne partagent pas la même valeur de `field`.
    ///
    /// PREMIER prédicat qui compare des nœuds ENTRE EUX au lieu de juger chacun
    /// isolément : c'est lui qui révise `DEC-AXO-901649`. La frontière se
    /// déplace, elle ne disparaît pas — voir la Décision qui la supersède.
    /// Forme de `duplicate_titles`, jusqu'ici en dur.
    UniqueBy { field: RuleField },
    /// Axe 5 — au plus `max` sujets par groupe. `group_by_relation` absent = un
    /// seul groupe (tous les sujets du projet).
    AtMost {
        max: usize,
        group_by_relation: Option<String>,
        group_direction: EdgeDirection,
    },
    /// Axe 6 — tout sujet doit ATTEINDRE un nœud correspondant à `other`, en ne
    /// suivant que `relations`. Rend vérifiable la cohérence de filiation :
    /// « toute exigence remonte à une Vision ».
    Reaches {
        other: EndpointMatcher,
        relations: Vec<String>,
    },
    /// Axe 7 — le CORPS du sujet doit contenir au moins un de ces fragments,
    /// comparés sans casse. C'est la forme de `GUI-PRO-110` : le statut seul est
    /// invisible au scan d'un LLM, qui lit le texte.
    ///
    /// Fermé volontairement à une liste de fragments : accepter une expression
    /// régulière de l'utilisateur serait le langage de requête que
    /// `DEC-AXO-901673` continue d'interdire.
    BodyContainsAny { fragments: Vec<String> },
    /// Axe 8 — le sous-graphe formé par `relations` ne contient aucun cycle.
    /// Forme de `DEC-AXO-098`, dont le validateur n'a jamais pu être activé :
    /// `soll_acyclic_audit` mesure 3 cycles sur AXO et dit lui-même qu'il
    /// « requires these to be 0 ».
    Acyclic { relations: Vec<String> },
}

/// Le champ sur lequel porte une contrainte d'unicité. Fermé volontairement :
/// ouvrir sur une expression arbitraire serait le langage de requête que la
/// Décision continue d'interdire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleField {
    Title,
    Id,
}

impl RuleField {
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "title" => Some(Self::Title),
            "id" => Some(Self::Id),
            _ => None,
        }
    }

    fn of<'a>(&self, node: &'a super::snapshot::SnapshotNode) -> &'a str {
        match self {
            Self::Title => node.title.as_str(),
            Self::Id => node.id.as_str(),
        }
    }
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

/// Quel prédicat a produit une violation. Permet à un consommateur de filtrer
/// par NATURE de règle sans coder en dur l'id d'une Guideline — le couplage
/// code→règle que le passage aux règles-données supprime précisément.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredicateKind {
    Edge,
    EvidenceStatus,
    Metadata,
    Uniqueness,
    Aggregate,
    Reachability,
    BodyContent,
    Acyclicity,
}

impl RulePredicate {
    pub fn kind(&self) -> PredicateKind {
        match self {
            Self::Edge { .. } => PredicateKind::Edge,
            Self::ForbiddenEvidenceStatus { .. } => PredicateKind::EvidenceStatus,
            Self::RequiredMetadata { .. } => PredicateKind::Metadata,
            Self::UniqueBy { .. } => PredicateKind::Uniqueness,
            Self::AtMost { .. } => PredicateKind::Aggregate,
            Self::Reaches { .. } => PredicateKind::Reachability,
            Self::BodyContainsAny { .. } => PredicateKind::BodyContent,
            Self::Acyclic { .. } => PredicateKind::Acyclicity,
        }
    }
}

/// Une violation détectée, portant l'id de règle pour la jointure SOLL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SollRuleViolation {
    pub rule_id: String,
    /// La nature du prédicat violé — voir [`PredicateKind`].
    pub predicate: PredicateKind,
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
    let unique_by = text("unique_by");
    let at_most = v.get("at_most").and_then(|x| x.as_u64());
    let reaches = v.get("reaches").and_then(|x| x.as_bool()).unwrap_or(false);
    let body_fragments = string_list("body_contains_any");
    let acyclic = v.get("acyclic").and_then(|x| x.as_bool()).unwrap_or(false);
    let declared = [
        !evidence_statuses.is_empty(),
        !metadata_keys.is_empty(),
        unique_by.is_some(),
        at_most.is_some(),
        reaches,
        !body_fragments.is_empty(),
        acyclic,
    ];
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
    } else if let Some(field) = unique_by {
        RulePredicate::UniqueBy {
            field: RuleField::from_str_ci(&field)?,
        }
    } else if let Some(max) = at_most {
        RulePredicate::AtMost {
            max: max as usize,
            group_by_relation: text("group_by_relation"),
            group_direction: v
                .get("group_direction")
                .and_then(|x| x.as_str())
                .map(EdgeDirection::from_str_ci)
                .unwrap_or(Some(EdgeDirection::Outgoing))?,
        }
    } else if !body_fragments.is_empty() {
        RulePredicate::BodyContainsAny {
            fragments: body_fragments,
        }
    } else if acyclic {
        RulePredicate::Acyclic {
            relations: string_list("relations"),
        }
    } else if reaches {
        RulePredicate::Reaches {
            other: EndpointMatcher {
                kind: text("other_kind").or_else(|| text("target_kind")),
                status: StatusMatcher {
                    any_of: first_list("other_status_in", "target_status_in"),
                    none_of: first_list("other_status_not_in", "target_status_not_in"),
                },
            },
            relations: string_list("relations"),
        }
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
        predicate: rule.predicate.kind(),
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

/// Les deux bouts d'une arête, du point de vue du SUJET : `Outgoing` le voit
/// comme la source, `Incoming` comme la cible. Partagé par les prédicats
/// d'arête et de groupement.
fn orientations<'e>(
    edge: &'e super::snapshot::SnapshotEdge,
    direction: EdgeDirection,
) -> impl Iterator<Item = (&'e str, &'e str)> {
    let (source, target) = (edge.source_id.as_str(), edge.target_id.as_str());
    let lectures = match direction {
        EdgeDirection::Outgoing => [Some((source, target)), None],
        EdgeDirection::Incoming => [Some((target, source)), None],
        EdgeDirection::Either => [Some((source, target)), Some((target, source))],
    };
    lectures.into_iter().flatten()
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
            match mode {
                RuleMode::Forbidden => {
                    for edge in &snapshot.edges {
                        if !rel_ok(&edge.relation_type) {
                            continue;
                        }
                        // `Either` lit l'arête des deux côtés, mais une arête
                        // fautive reste UNE violation : on s'arrête à la
                        // première lecture qui correspond.
                        for (subject_id, other_id) in orientations(edge, *direction) {
                            let (Some((skind, sstatus)), Some((okind, ostatus))) =
                                (facts.get(subject_id), facts.get(other_id))
                            else {
                                continue;
                            };
                            if !(rule.subject.matches(skind, sstatus)
                                && other.matches(okind, ostatus))
                            {
                                continue;
                            }
                            out.push(SollRuleViolation {
                                rule_id: rule.id.clone(),
                                predicate: rule.predicate.kind(),
                                source_id: edge.source_id.clone(),
                                target_id: Some(edge.target_id.clone()),
                                relation: Some(edge.relation_type.clone()),
                                message: rule.message.clone(),
                            });
                            break;
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
                        for (subject_id, other_id) in orientations(edge, *direction) {
                        if !satisfied.contains_key(subject_id) {
                            continue;
                        }
                        // Une extrémité hors du snapshot n'est PAS une absence
                        // d'arête. Cas réel et encouragé par le produit : une
                        // Guideline de projet retirée par la canonique `PRO`
                        // (`GUI-AXO-1032` ← `GUI-PRO-124`). Le snapshot est
                        // chargé par projet, donc le remplaçant n'y figure pas —
                        // 10 nœuds sur 5 projets au 2026-08-22.
                        //
                        // Quand la règle n'exige RIEN de l'autre bout, l'arête
                        // satisfait : une contrainte qui ne porte pas sur ce
                        // nœud ne peut pas être invalidée par son absence. Si
                        // elle exige un kind ou un statut, on ne peut pas
                        // trancher — l'arête est ignorée, comme avant.
                        let qualifies = match facts.get(other_id) {
                            Some((okind, ostatus)) => other.matches(okind, ostatus),
                            None => other.is_unconstrained(),
                        };
                        if qualifies {
                            satisfied.insert(subject_id, true);
                        }
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
                        predicate: rule.predicate.kind(),
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
        RulePredicate::UniqueBy { field } => {
            // Grouper par valeur normalisée. Une violation NOMME les nœuds en
            // cause : un compteur de doublons n'ouvre aucune action
            // (REQ-AXO-902409).
            let mut by_value: HashMap<String, Vec<&str>> = HashMap::new();
            for id in subjects(rule, facts) {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                let value = field.of(node).trim().to_lowercase();
                if value.is_empty() {
                    // Un champ vide n'est pas un doublon : c'est une absence,
                    // qui relève d'une règle de métadonnées, pas d'unicité.
                    continue;
                }
                by_value.entry(value).or_default().push(id);
            }
            let mut groups: Vec<(String, Vec<&str>)> = by_value
                .into_iter()
                .filter(|(_, ids)| ids.len() > 1)
                .collect();
            groups.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (value, mut ids) in groups {
                ids.sort_unstable();
                for id in &ids {
                    let others: Vec<&str> =
                        ids.iter().copied().filter(|other| other != id).collect();
                    out.push(SollRuleViolation {
                        rule_id: rule.id.clone(),
                        predicate: rule.predicate.kind(),
                        source_id: (*id).to_string(),
                        target_id: Some(others.join(", ")),
                        relation: Some(format!("same:{value}")),
                        message: rule.message.clone(),
                    });
                }
            }
        }
        RulePredicate::AtMost {
            max,
            group_by_relation,
            group_direction,
        } => {
            let subject_ids: Vec<&str> = subjects(rule, facts);
            if subject_ids.is_empty() {
                return out;
            }
            // Sans relation de groupement, tous les sujets forment UN groupe.
            let mut groups: HashMap<String, Vec<&str>> = HashMap::new();
            match group_by_relation {
                None => {
                    groups.insert(String::new(), subject_ids);
                }
                Some(relation) => {
                    let member: std::collections::HashSet<&str> =
                        subject_ids.iter().copied().collect();
                    for edge in &snapshot.edges {
                        if &edge.relation_type != relation {
                            continue;
                        }
                        for (subject_id, group_id) in orientations(edge, *group_direction) {
                            if !member.contains(subject_id) {
                                continue;
                            }
                            groups.entry(group_id.to_string()).or_default().push(subject_id);
                        }
                    }
                    // Un sujet sans groupe est IGNORÉ, jamais compté dans un
                    // groupe fictif : l'affecter d'office fausserait le compte.
                }
            }
            let mut over: Vec<(String, Vec<&str>)> = groups
                .into_iter()
                .filter(|(_, ids)| ids.len() > *max)
                .collect();
            over.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (group_id, mut ids) in over {
                ids.sort_unstable();
                ids.dedup();
                let count = ids.len();
                for id in ids {
                    out.push(SollRuleViolation {
                        rule_id: rule.id.clone(),
                        predicate: rule.predicate.kind(),
                        source_id: id.to_string(),
                        target_id: Some(if group_id.is_empty() {
                            format!("{count} sujets pour un maximum de {max}")
                        } else {
                            format!("{group_id} ({count} sujets pour un maximum de {max})")
                        }),
                        relation: Some("at_most".to_string()),
                        message: rule.message.clone(),
                    });
                }
            }
        }
        RulePredicate::BodyContainsAny { fragments } => {
            if rule.subject.is_unconstrained() {
                return out;
            }
            let needles: Vec<String> = fragments.iter().map(|f| f.to_lowercase()).collect();
            for id in subjects(rule, facts) {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                let body = node.description.to_lowercase();
                if !needles.iter().any(|needle| body.contains(needle.as_str())) {
                    out.push(violation(rule, id));
                }
            }
        }
        RulePredicate::Acyclic { relations } => {
            // Un cycle n'appartient à aucun nœud en particulier : chaque membre
            // est nommé, parce qu'un compteur de cycles n'ouvre aucune action
            // (REQ-AXO-902409) et que la réparation se choisit en voyant la
            // boucle entière.
            let rel_set: std::collections::HashSet<String> = if relations.is_empty() {
                snapshot
                    .edges
                    .iter()
                    .map(|e| e.relation_type.clone())
                    .collect()
            } else {
                relations.iter().cloned().collect()
            };
            let member: std::collections::HashSet<&str> =
                subjects(rule, facts).into_iter().collect();
            for cycle in snapshot.cycle_sets_via_relations(&rel_set) {
                let mut cited: Vec<&str> = cycle
                    .iter()
                    .map(String::as_str)
                    .filter(|id| member.contains(id))
                    .collect();
                cited.sort_unstable();
                let names = cited.join(", ");
                for id in cited {
                    out.push(SollRuleViolation {
                        rule_id: rule.id.clone(),
                        predicate: rule.predicate.kind(),
                        source_id: id.to_string(),
                        target_id: Some(names.clone()),
                        relation: Some("cycle".to_string()),
                        message: rule.message.clone(),
                    });
                }
            }
        }
        RulePredicate::Reaches { other, relations } => {
            if rule.subject.is_unconstrained() {
                return out;
            }
            let rel_set: std::collections::HashSet<String> = if relations.is_empty() {
                snapshot
                    .edges
                    .iter()
                    .map(|e| e.relation_type.clone())
                    .collect()
            } else {
                relations.iter().cloned().collect()
            };
            // Les cibles acceptables. En pratique peu nombreuses (les Visions
            // d'un projet se comptent sur une main), d'où la boucle directe
            // plutôt qu'un BFS instrumenté : `reaches_via_relations` est la
            // traversée petgraph déjà testée du dépôt, on ne la réécrit pas.
            let targets: Vec<&str> = facts
                .iter()
                .filter(|(_, (kind, status))| other.matches(kind, status))
                .map(|(id, _)| *id)
                .collect();
            for id in subjects(rule, facts) {
                let reached = targets
                    .iter()
                    .any(|target| snapshot.reaches_via_relations(id, target, &rel_set));
                if !reached {
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
            description: String::new(),
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
            description: String::new(),
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

    /// REQ-AXO-902458 / V2 — axe « contenu du corps ». `GUI-PRO-110` dit que le
    /// statut seul est INVISIBLE au scan d'un LLM (les LLM lisent le CORPS) :
    /// un nœud retiré doit le dire dans son texte. Cette guideline existe depuis
    /// des mois et n'a JAMAIS été mécanisée — mesuré : **122 nœuds `superseded`
    /// sur 12 projets** dont le corps ne porte aucun marqueur.
    ///
    /// Le prédicat reste FERMÉ : une liste de fragments, comparés sans casse.
    /// Pas d'expression régulière fournie par l'utilisateur — ce serait le
    /// langage de requête que `DEC-AXO-901673` continue d'interdire.
    #[test]
    fn a_body_rule_flags_a_retired_node_whose_text_does_not_say_it_is_retired() {
        let rule = parse_soll_rule(
            "GUI-TST-020",
            "Un nœud retiré le dit dans son corps",
            &json!({
                "subject_status_in": ["superseded"],
                "body_contains_any": ["supersédé par", "remplacé par"],
                "message": "statut retiré mais le corps ne le dit pas"
            }),
        )
        .unwrap();
        let mut marked = node("REQ-TST-201", "Requirement", "superseded");
        marked.description = "Corps utile. SUPERSÉDÉ PAR REQ-TST-203.".to_string();
        let mut silent = node("REQ-TST-202", "Requirement", "superseded");
        silent.description = "Corps utile, et rien qui dise qu'il est retiré.".to_string();
        let mut living = node("REQ-TST-203", "Requirement", "current");
        living.description = "Aucun marqueur, mais ce nœud est VIVANT.".to_string();

        let snap = snapshot(vec![marked, silent, living], vec![]);
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(
            found.iter().map(|v| v.source_id.as_str()).collect::<Vec<_>>(),
            vec!["REQ-TST-202"],
            "seul le nœud retiré SANS marqueur est fautif ; la comparaison ignore \
             la casse, et un nœud vivant n'est pas sujet.\n{found:?}"
        );
    }

    /// REQ-AXO-902458 / V2 — axe « acyclicité ». `DEC-AXO-098` impose un graphe
    /// de filiation strictement acyclique, et `soll_acyclic_audit` le mesure —
    /// mais en CODE, et son propre message dit que le validateur « requires
    /// these to be 0 » pour être activé. Mesuré : **3 cycles sur AXO**, donc il
    /// ne l'a jamais été.
    ///
    /// Le prédicat porte sur un JEU de relations : un cycle par `SUPERSEDES`
    /// n'est pas un cycle de filiation, et les confondre signalerait des nœuds
    /// que personne ne peut réparer.
    #[test]
    fn an_acyclic_rule_flags_only_the_cycle_formed_by_the_named_relations() {
        let rule = parse_soll_rule(
            "GUI-TST-021",
            "La filiation ne boucle pas",
            &json!({
                "subject_kind": "Requirement",
                "acyclic": true,
                "relations": ["REFINES"],
                "message": "cycle de filiation"
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("REQ-TST-301", "Requirement", "current"),
                node("REQ-TST-302", "Requirement", "current"),
                node("REQ-TST-303", "Requirement", "current"),
                node("REQ-TST-304", "Requirement", "current"),
            ],
            vec![
                // Cycle SUR les relations visées.
                edge("REQ-TST-301", "REQ-TST-302", "REFINES"),
                edge("REQ-TST-302", "REQ-TST-301", "REFINES"),
                // Cycle sur une AUTRE relation : hors sujet, ne doit rien produire.
                edge("REQ-TST-303", "REQ-TST-304", "SUPERSEDES"),
                edge("REQ-TST-304", "REQ-TST-303", "SUPERSEDES"),
            ],
        );
        let found = evaluate_rule(&snap, &rule);
        let mut cited: Vec<&str> = found.iter().map(|v| v.source_id.as_str()).collect();
        cited.sort_unstable();
        assert_eq!(
            cited,
            vec!["REQ-TST-301", "REQ-TST-302"],
            "les deux nœuds du cycle REFINES sont nommés — un compteur de cycles \
             n'ouvre aucune action (REQ-AXO-902409) — et le cycle SUPERSEDES est \
             hors du jeu de relations visé.\n{found:?}"
        );
    }

    /// REQ-AXO-902455 — le snapshot est chargé PAR PROJET, mais une supersession
    /// cross-projet est un motif que le produit ENCOURAGE : une Guideline de
    /// tenant retirée par la canonique `PRO`. Mesuré au 2026-08-22 : 10 nœuds
    /// sur 5 projets (FSF 4, AXO 2, MLD 2, NEX 1, TE2 1).
    ///
    /// Le remplaçant n'étant pas dans le snapshot du tenant, l'arête pointait
    /// vers un nœud inconnu et la règle déclarait le sujet orphelin. Ignorer
    /// une extrémité absente est prudent pour `forbidden` (ne pas inventer une
    /// violation) ; pour `required` c'est l'inverse, cela en FABRIQUE une.
    #[test]
    fn a_replacement_living_outside_the_snapshot_still_satisfies_an_incoming_rule() {
        let rule = parse_soll_rule(
            "GUI-TST-012",
            "Un nœud retiré dit ce qui le remplace",
            &json!({
                "mode": "required",
                "direction": "incoming",
                "subject_status_in": ["superseded"],
                "relations": ["SUPERSEDES"]
            }),
        )
        .unwrap();
        // `GUI-PRO-124` n'est PAS dans les nœuds : il vit dans le projet PRO.
        // L'arête, elle, est chargée — le loader prend celles dont UNE extrémité
        // est ancrée dans le projet.
        let snap = snapshot(
            vec![
                node("GUI-TST-001", "Guideline", "superseded"), // remplacé cross-projet
                node("GUI-TST-002", "Guideline", "superseded"), // vraiment orphelin
            ],
            vec![edge("GUI-PRO-124", "GUI-TST-001", "SUPERSEDES")],
        );
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(
            found.iter().map(|v| v.source_id.as_str()).collect::<Vec<_>>(),
            vec!["GUI-TST-002"],
            "seul le nœud SANS remplaçant est fautif ; celui repris par une \
             Guideline PRO a bien enregistré ce qui le remplace.\n{found:?}"
        );

        // Contrôle positif — la tolérance ne vaut QUE si la règle n'exige rien
        // de l'autre bout. Dès qu'elle contraint le remplaçant, un nœud absent
        // ne peut plus être présumé conforme : on ne sait pas ce qu'il est.
        let demanding = parse_soll_rule(
            "GUI-TST-013",
            "Le remplaçant doit être vivant",
            &json!({
                "mode": "required",
                "direction": "incoming",
                "subject_status_in": ["superseded"],
                "relations": ["SUPERSEDES"],
                "other_status_in": ["current"]
            }),
        )
        .unwrap();
        let strict = evaluate_rule(&snap, &demanding);
        assert_eq!(
            strict.len(),
            2,
            "une règle qui exige un statut du remplaçant ne peut pas le \
             présumer sur un nœud qu'elle ne voit pas.\n{strict:?}"
        );
    }

    /// REQ-AXO-902455 — troisième direction : `either`. Sans elle, trois
    /// invariants SOLL restaient en dur dans `soll_completeness_snapshot_filtered`
    /// pour une seule raison — ils demandent « une arête dans L'UN OU L'AUTRE
    /// sens » (`orphan_requirements`, `validations_without_verifies`,
    /// `decisions_without_links`), et le moteur n'en testait qu'une par règle.
    ///
    /// Deux règles `outgoing` + `incoming` ne les remplacent PAS : un nœud sans
    /// aucune arête produirait DEUX violations pour un seul défaut, et un nœud
    /// rattaché d'un seul côté en produirait une, à tort.
    #[test]
    fn an_either_direction_accepts_an_edge_on_whichever_side_it_sits() {
        let rule = parse_soll_rule(
            "GUI-TST-014",
            "Une exigence est rattachée au graphe",
            &json!({
                "mode": "required",
                "direction": "either",
                "subject_kind": "Requirement",
                "subject_status_in": ["planned"]
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("REQ-TST-101", "Requirement", "planned"), // rattaché en SORTANT
                node("REQ-TST-102", "Requirement", "planned"), // rattaché en ENTRANT
                node("REQ-TST-103", "Requirement", "planned"), // rattaché à RIEN
                node("PIL-TST-101", "Pillar", "current"),
                node("DEC-TST-101", "Decision", "current"),
            ],
            vec![
                edge("REQ-TST-101", "PIL-TST-101", "BELONGS_TO"),
                edge("DEC-TST-101", "REQ-TST-102", "SOLVES"),
            ],
        );
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(
            found.iter().map(|v| v.source_id.as_str()).collect::<Vec<_>>(),
            vec!["REQ-TST-103"],
            "seul le nœud sans AUCUNE arête est orphelin ; le sens de \
             rattachement ne doit pas décider.\n{found:?}"
        );

        // ── Contrôle positif : `either` n'est pas « toujours satisfait » ────
        // La même règle en `outgoing` seule accuse REQ-TST-102 à tort. C'est
        // ce qui prouve que la troisième direction dit quelque chose que les
        // deux autres ne peuvent pas dire.
        let outgoing_only = parse_soll_rule(
            "GUI-TST-015",
            "sortante seule",
            &json!({
                "mode": "required",
                "subject_kind": "Requirement",
                "subject_status_in": ["planned"]
            }),
        )
        .unwrap();
        let narrow = evaluate_rule(&snap, &outgoing_only);
        assert!(
            narrow.iter().any(|v| v.source_id == "REQ-TST-102"),
            "la direction sortante seule doit accuser le nœud rattaché en \
             ENTRANT — sinon `either` ne se distingue de rien.\n{narrow:?}"
        );

        // `either` vaut aussi pour `forbidden` : l'arête est interdite des deux
        // côtés, et chaque arête fautive compte UNE fois, pas deux.
        let banned = parse_soll_rule(
            "GUI-TST-016",
            "aucune liaison entre ces deux mondes",
            &json!({
                "mode": "forbidden",
                "direction": "either",
                "subject_kind": "Decision",
                "other_kind": "Requirement",
                "relations": ["SOLVES"]
            }),
        )
        .unwrap();
        let banned_hits = evaluate_rule(&snap, &banned);
        assert_eq!(
            banned_hits.len(),
            1,
            "une arête vue des deux côtés reste UNE violation.\n{banned_hits:?}"
        );
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

    /// AXE 4 — unicité. PREMIER prédicat qui compare des nœuds ENTRE EUX ; c'est
    /// lui qui révise `DEC-AXO-901649`. Forme de `duplicate_titles`, en dur
    /// jusqu'ici.
    #[test]
    fn a_uniqueness_rule_names_every_node_that_shares_a_title() {
        let rule = parse_soll_rule(
            "GUI-TST-020",
            "Deux exigences ne portent pas le même titre",
            &json!({ "subject_kind": "Requirement", "unique_by": "title" }),
        )
        .unwrap();
        let titled = |id: &str, title: &str| SnapshotNode {
            id: id.to_string(),
            entity_type: "Requirement".to_string(),
            title: title.to_string(),
            status: "current".to_string(),
            metadata_raw: "{}".to_string(),
            description: String::new(),
        };
        let snap = snapshot(
            vec![
                titled("REQ-TST-001", "Corriger le cache"),
                titled("REQ-TST-002", "  corriger le CACHE  "), // même titre, normalisé
                titled("REQ-TST-003", "Autre chose"),           // unique
                titled("REQ-TST-004", ""),                      // vide : une ABSENCE
            ],
            vec![],
        );
        let found = evaluate_rule(&snap, &rule);
        let flagged: Vec<&str> = found.iter().map(|v| v.source_id.as_str()).collect();
        assert_eq!(
            flagged,
            vec!["REQ-TST-001", "REQ-TST-002"],
            "les DEUX porteurs sont nommés — un seul ne dirait pas quoi comparer"
        );
        // Chaque ligne NOMME l'autre : un compteur de doublons n'ouvre aucune action.
        assert!(found[0].target_id.as_deref() == Some("REQ-TST-002"), "{found:?}");
        assert!(found[1].target_id.as_deref() == Some("REQ-TST-001"), "{found:?}");
        // Contrôles positifs : l'unique n'est pas signalé, et un titre VIDE n'est
        // pas un doublon — c'est une absence, qui relève d'une autre règle.
        assert!(!flagged.contains(&"REQ-TST-003"));
        assert!(!flagged.contains(&"REQ-TST-004"));
    }

    /// AXE 5 — agrégat. Capacité neuve : aucun check en dur ne la couvrait.
    #[test]
    fn an_at_most_rule_flags_the_group_that_exceeds_its_ceiling() {
        let rule = parse_soll_rule(
            "GUI-TST-021",
            "Au plus deux exigences en cours par jalon",
            &json!({
                "subject_kind": "Requirement",
                "subject_status_in": ["current"],
                "at_most": 2,
                "group_by_relation": "TARGETS",
                "group_direction": "incoming"
            }),
        )
        .unwrap();
        let mut nodes = vec![
            node("MIL-TST-001", "Milestone", "current"),
            node("MIL-TST-002", "Milestone", "current"),
        ];
        let mut edges = vec![];
        // MIL-001 porte 3 exigences (au-dessus), MIL-002 en porte exactement 2.
        for (mil, count) in [("MIL-TST-001", 3), ("MIL-TST-002", 2)] {
            for n in 1..=count {
                let id = format!("REQ-{}-{n}", &mil[4..11]);
                nodes.push(node(&id, "Requirement", "current"));
                edges.push(edge(mil, &id, "TARGETS"));
            }
        }
        let snap = snapshot(nodes, edges);
        let found = evaluate_rule(&snap, &rule);
        assert_eq!(found.len(), 3, "seul le groupe en dépassement : {found:?}");
        assert!(
            found.iter().all(|v| v.source_id.starts_with("REQ-TST-001")),
            "{found:?}"
        );
        // La ligne dit DE COMBIEN on dépasse, pas seulement qu'on dépasse.
        assert!(
            found[0].target_id.as_deref().is_some_and(|t| t.contains("3 sujets")),
            "{found:?}"
        );
        // Contrôle positif : la borne est INCLUSIVE — un groupe à exactement
        // `max` n'est pas une violation, et ce test fixe cette convention.
        assert!(!found.iter().any(|v| v.source_id.starts_with("REQ-TST-002")));
    }

    /// AXE 6 — atteignabilité. Réutilise `reaches_via_relations`, la traversée
    /// petgraph déjà testée du dépôt : ce prédicat est un appel, pas un
    /// algorithme réécrit.
    #[test]
    fn a_reachability_rule_flags_the_requirement_that_reaches_no_vision() {
        let rule = parse_soll_rule(
            "GUI-TST-022",
            "Toute exigence remonte à une Vision",
            &json!({
                "subject_kind": "Requirement",
                "reaches": true,
                "other_kind": "Vision",
                "relations": ["BELONGS_TO", "EPITOMIZES"]
            }),
        )
        .unwrap();
        let snap = snapshot(
            vec![
                node("VIS-TST-001", "Vision", "current"),
                node("PIL-TST-001", "Pillar", "current"),
                node("REQ-TST-001", "Requirement", "current"), // rattaché
                node("REQ-TST-002", "Requirement", "current"), // orphelin
                node("REQ-TST-003", "Requirement", "current"), // rattaché à un pilier ISOLÉ
                node("PIL-TST-002", "Pillar", "current"),
            ],
            vec![
                edge("PIL-TST-001", "VIS-TST-001", "EPITOMIZES"),
                edge("REQ-TST-001", "PIL-TST-001", "BELONGS_TO"),
                edge("REQ-TST-003", "PIL-TST-002", "BELONGS_TO"),
            ],
        );
        let found = evaluate_rule(&snap, &rule);
        let flagged: Vec<&str> = found.iter().map(|v| v.source_id.as_str()).collect();
        assert_eq!(
            flagged,
            vec!["REQ-TST-002", "REQ-TST-003"],
            "l'orphelin ET celui dont le pilier ne mène nulle part : c'est la \
             TRANSITIVITÉ qui est testée, pas le voisinage direct"
        );
        // Contrôle positif : celui qui atteint la Vision en deux sauts passe.
        assert!(!flagged.contains(&"REQ-TST-001"));
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
