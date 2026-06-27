//! S1 (REQ-AXO-902088) — store canonique du ContractNode + S6 (REQ-AXO-902093)
//! réconciliation IST↔contrat. Modèle A : tables `soll.Contract` / `soll.ContractEdge`
//! (DDL `db/ddl/21_contract.sql`), ADOSSÉES à la machinerie SOLL via [`GraphStore`].
//!
//! Deux responsabilités, séparées du cœur pur ([`super::ContractNode`]) :
//!
//! - **Persistance (S1)** : [`persist_contract`] (UPSERT) / [`load_contract`] —
//!   la forme désirée durable ; [`persist_seal`] / [`load_seal`] — le sceau
//!   structurel Merkle versionné (`H(shape_hash, proves_ref, adequacy_verdict,
//!   [enfants])`, l'empirique RESTE hors-hash, DEC-AXO-901657).
//!
//! - **Réconciliation (S6)** : [`reconcile_contract`] compare la forme IST-OBSERVÉE
//!   du `realized_by` (ré-calculée live depuis `ist.Symbol`) contre la baseline
//!   stockée (`observed_shape_hash`), et type le drift ([`DriftVerdict`]).
//!   Binding *witnessed-first* : l'ancre `realized_by` est BORNÉE À L'IDENTITÉ
//!   (rename-tracking, DEC-AXO-901656), JAMAIS à la certification. La
//!   réconciliation lit le `realized_by` déjà stocké sur le contrat — elle
//!   n'EXIGE aucun marqueur `realizes:<CON-id>` omniprésent dans le code (ce qui
//!   serait le retour au MDA, le RED FLAG du panel). Aucune dépendance à un
//!   verdict de certification.
//!
//! Toutes les méthodes sont SYNCHRONES (idiome `execute_param` / `query_json_param`)
//! : le fixture de test (`create_test_db`) porte son propre runtime, les tests
//! `#[test]` sync ne doivent pas en imbriquer un second (cf. pipeline/stage_b1.rs).

use anyhow::{anyhow, Result};
use serde_json::Value;

use super::seal::StructuralSeal;
use super::{sha256_hex, ContractKind, ContractNode, PostCondition};
use crate::graph::GraphStore;

/// Sceau persisté : le hash structurel + son contexte d'adéquation + sa version
/// monotone. L'attestation empirique n'y figure PAS (canal séparé, hors-hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSeal {
    pub seal: StructuralSeal,
    /// `true` = le gate d'adéquation est passé au moment du sceau (un sceau
    /// n'existe que si adéquat — invariant de [`super::seal::structural_seal`]).
    pub adequate: bool,
    /// Version monotone (millis epoch) — ordonne les re-sceaux sans contaminer
    /// le journal d'intention soll.Revision.
    pub revision: i64,
}

/// Verdict de réconciliation IST↔contrat (S6). Drift TYPÉ : chaque variante porte
/// l'évidence-sol nécessaire à la décision en aval (re-anchor, re-sceau, alerte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftVerdict {
    /// `realized_by` absent : contrat planifié, binding S4 pas encore établi —
    /// rien à réconcilier (ce n'est PAS un drift).
    Unbound,
    /// L'ancre d'identité `realized_by` pointe un symbole absent de l'IST :
    /// candidat rename / re-anchor (l'identité a bougé, DEC-AXO-901656).
    SymbolMissing { symbol_id: String },
    /// Le `kind` IST-observé est incompatible avec le `ContractKind` désiré — la
    /// forme réalisée a divergé de la promesse structurelle.
    KindMismatch { expected: ContractKind, observed_kind: String },
    /// La forme IST-observée a dérivé depuis la baseline stockée (même symbole,
    /// même kind compatible, mais hash observé ≠ baseline).
    ShapeDrift { baseline: String, observed: String },
    /// Première réconciliation (aucune baseline stockée) : rien à comparer, la
    /// forme observée est retournée pour qu'un appelant la fige via
    /// [`capture_observed_baseline`].
    NoBaseline { observed: String },
    /// Forme observée == baseline et kind compatible — aligné.
    Aligned { observed: String },
}

// ════════════════════════════════════════════════════════════════════════
// S1 — persistance
// ════════════════════════════════════════════════════════════════════════

/// UPSERT d'un ContractNode. Le contrat est validé ([`ContractNode::validate`])
/// AVANT écriture — un contrat malformé n'a pas de `shape_hash` intègre, donc
/// rien à persister (le gain B2). `project_code` est dérivé de l'id canonique
/// quand possible, sinon `'AXO'`.
///
/// L'UPSERT préserve `observed_shape_hash`, `seal_*` et `created_at` sur conflit
/// (ce sont des dimensions gérées par [`persist_seal`] / [`capture_observed_baseline`],
/// pas par la forme désirée).
pub fn persist_contract(store: &GraphStore, id: &str, node: &ContractNode) -> Result<()> {
    node.validate()
        .map_err(|e| anyhow!("contrat malformé, non persisté: {e:?}"))?;

    let post: Vec<&str> = node.post_conditions.iter().map(|p| p.0.as_str()).collect();
    let post_json = serde_json::to_string(&post)?;
    let project_code = project_code_from_id(id);

    store.execute_param(
        "INSERT INTO soll.Contract
            (id, project_code, kind, signature, why, post_conditions, proves_ref,
             realized_by, shape_hash, status, updated_at)
         VALUES
            ($id, $project_code, $kind, $signature, $why, $post_conditions::jsonb,
             $proves_ref, $realized_by, $shape_hash, $status,
             (extract(epoch from now()) * 1000)::BIGINT)
         ON CONFLICT (id) DO UPDATE SET
            project_code    = EXCLUDED.project_code,
            kind            = EXCLUDED.kind,
            signature       = EXCLUDED.signature,
            why             = EXCLUDED.why,
            post_conditions = EXCLUDED.post_conditions,
            proves_ref      = EXCLUDED.proves_ref,
            realized_by     = EXCLUDED.realized_by,
            shape_hash      = EXCLUDED.shape_hash,
            status          = EXCLUDED.status,
            updated_at      = EXCLUDED.updated_at",
        &serde_json::json!({
            "id": id,
            "project_code": project_code,
            "kind": node.kind.tag(),
            "signature": node.signature,
            "why": node.why,
            "post_conditions": post_json,
            "proves_ref": node.proves_ref,
            "realized_by": node.realized_by,
            "shape_hash": node.shape_hash(),
            "status": if node.realized_by.is_some() { "bound" } else { "planned" },
        }),
    )?;

    // Arête d'identité REALIZED_BY (cross-ref → ist.Symbol) — bornée à l'identité,
    // jamais à la certification (DEC-AXO-901656).
    if let Some(sym) = &node.realized_by {
        upsert_edge(store, id, sym, "REALIZED_BY", &project_code)?;
    }
    // Cross-ref de gouvernance dérivé du `why` (« SOLVES <SOLL-id> » /
    // « EXPLAINS <SOLL-id> ») → arête vers soll.Node : le pourquoi gouvernant est
    // rendu navigable, pas enfermé dans une chaîne libre.
    for (relation, target) in governance_refs(&node.why) {
        upsert_edge(store, id, &target, relation, &project_code)?;
    }
    Ok(())
}

/// Extrait les cross-refs de gouvernance d'un `why` (« SOLVES <id> », « EXPLAINS
/// <id> »). Tolérant à la casse du verbe ; l'id suivant est pris tel quel (format
/// canonique `TYPE-PROJ-N`, DEC-AXO-085).
fn governance_refs(why: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let tokens: Vec<&str> = why.split_whitespace().collect();
    for pair in tokens.windows(2) {
        let relation = match pair[0].to_ascii_uppercase().as_str() {
            "SOLVES" => "SOLVES",
            "EXPLAINS" => "EXPLAINS",
            _ => continue,
        };
        let target = pair[1].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if target.contains('-') {
            out.push((relation, target.to_string()));
        }
    }
    out
}

/// Charge un ContractNode par id. `None` si absent. Le `shape_hash` n'est PAS
/// reconstruit depuis la ligne : il est ré-dérivable de la forme chargée
/// ([`ContractNode::shape_hash`]) — l'appelant qui veut l'intégrité compare.
pub fn load_contract(store: &GraphStore, id: &str) -> Result<Option<ContractNode>> {
    let raw = store.query_json_param(
        "SELECT kind, signature, why, post_conditions, proves_ref, realized_by
           FROM soll.Contract WHERE id = $id",
        &serde_json::json!({ "id": id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };

    let kind_tag = col_str(&row, 0).ok_or_else(|| anyhow!("kind manquant"))?;
    let kind = ContractKind::from_tag(&kind_tag)
        .ok_or_else(|| anyhow!("kind persisté inconnu: {kind_tag}"))?;
    let signature = col_str(&row, 1).ok_or_else(|| anyhow!("signature manquante"))?;
    let why = col_str(&row, 2).unwrap_or_default();
    let post_conditions = parse_post_conditions(row.get(3));
    let proves_ref = col_str(&row, 4).unwrap_or_default();
    let realized_by = col_str(&row, 5);

    Ok(Some(ContractNode {
        kind,
        signature,
        why,
        post_conditions,
        proves_ref,
        realized_by,
    }))
}

/// Persiste le sceau structurel d'un contrat + sa version monotone. Le sceau a
/// déjà été calculé (et donc gaté par l'adéquation) par
/// [`super::seal::structural_seal`] côté appelant — ici on ne fait que le DURCIR.
/// `adequate` reflète le gate franchi (un sceau présent ⇒ adéquat par invariant).
pub fn persist_seal(
    store: &GraphStore,
    id: &str,
    seal: &StructuralSeal,
    adequate: bool,
) -> Result<i64> {
    let revision = now_ms();
    store.execute_param(
        "UPDATE soll.Contract SET
            seal_hash        = $seal_hash,
            adequacy_verdict = $adequacy_verdict,
            seal_revision    = $seal_revision,
            status           = 'sealed',
            updated_at       = (extract(epoch from now()) * 1000)::BIGINT
         WHERE id = $id",
        &serde_json::json!({
            "id": id,
            "seal_hash": seal.0,
            "adequacy_verdict": if adequate { "adequate" } else { "inadequate" },
            "seal_revision": revision,
        }),
    )?;
    Ok(revision)
}

/// Charge le sceau persisté d'un contrat. `None` si le contrat n'est pas scellé
/// (`seal_hash` NULL) — l'absence de sceau est un état légitime (sealing partiel,
/// DEC-AXO-901659), pas une erreur.
pub fn load_seal(store: &GraphStore, id: &str) -> Result<Option<StoredSeal>> {
    let raw = store.query_json_param(
        "SELECT seal_hash, adequacy_verdict, seal_revision
           FROM soll.Contract WHERE id = $id",
        &serde_json::json!({ "id": id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let Some(seal_hash) = col_str(&row, 0) else {
        return Ok(None); // contrat présent mais non scellé
    };
    let adequate = col_str(&row, 1).as_deref() == Some("adequate");
    let revision = col_i64(&row, 2).unwrap_or(0);
    Ok(Some(StoredSeal {
        seal: StructuralSeal(seal_hash),
        adequate,
        revision,
    }))
}

// ════════════════════════════════════════════════════════════════════════
// S6 — réconciliation IST↔contrat
// ════════════════════════════════════════════════════════════════════════

/// Hash de la forme IST-OBSERVÉE d'un symbole : `H(kind_ist, name)`. Déterministe,
/// content-addressed comme le `shape_hash` désiré. C'est le « ré-calcul du
/// shape_hash depuis ist.Symbol » : on ne dispose en IST que de (kind, name) — pas
/// de la signature typée complète ni des post-conditions — donc l'observé borne sa
/// promesse aux dimensions réellement témoignées par l'IST (honnêteté du signal :
/// pas de hash sur des champs qu'on n'observe pas).
fn observed_shape_hash(ist_kind: &str, name: &str) -> String {
    sha256_hex(&format!("{}\u{1f}{}", ist_kind.trim(), name.trim()))
}

/// Fige la forme IST-OBSERVÉE courante du `realized_by` comme baseline du contrat
/// (S6). Retourne le hash figé, ou `None` si le contrat n'a pas d'ancre / le
/// symbole est absent de l'IST. C'est l'établissement explicite du point de
/// référence contre lequel [`reconcile_contract`] détectera le drift ultérieur.
pub fn capture_observed_baseline(store: &GraphStore, id: &str) -> Result<Option<String>> {
    let Some(realized_by) = contract_realized_by(store, id)? else {
        return Ok(None);
    };
    let Some((ist_kind, name)) = ist_symbol(store, &realized_by)? else {
        return Ok(None);
    };
    let observed = observed_shape_hash(&ist_kind, &name);
    store.execute_param(
        "UPDATE soll.Contract SET
            observed_shape_hash = $observed,
            updated_at          = (extract(epoch from now()) * 1000)::BIGINT
         WHERE id = $id",
        &serde_json::json!({ "id": id, "observed": observed }),
    )?;
    Ok(Some(observed))
}

/// Réconcilie un contrat avec l'IST (S6). LECTURE SEULE : compare la forme
/// IST-observée live du `realized_by` (ancre d'identité, rename-tracking) contre la
/// baseline stockée, et contre le `ContractKind` désiré. Le drift est typé
/// ([`DriftVerdict`]).
///
/// La réconciliation s'appuie sur le `realized_by` DÉJÀ stocké — elle n'exige
/// aucun marqueur `realizes:` omniprésent dans le code source (ce qui ramènerait au
/// MDA, le RED FLAG du panel) et n'interroge AUCUN verdict de certification.
pub fn reconcile_contract(store: &GraphStore, id: &str) -> Result<DriftVerdict> {
    let row = load_reconcile_row(store, id)?
        .ok_or_else(|| anyhow!("contrat introuvable: {id}"))?;
    let ReconcileRow { kind, realized_by, baseline } = row;

    let Some(realized_by) = realized_by else {
        return Ok(DriftVerdict::Unbound);
    };
    let Some((ist_kind, name)) = ist_symbol(store, &realized_by)? else {
        return Ok(DriftVerdict::SymbolMissing { symbol_id: realized_by });
    };

    if !ist_kind_matches(kind, &ist_kind) {
        return Ok(DriftVerdict::KindMismatch {
            expected: kind,
            observed_kind: ist_kind,
        });
    }

    let observed = observed_shape_hash(&ist_kind, &name);
    match baseline {
        None => Ok(DriftVerdict::NoBaseline { observed }),
        Some(b) if b == observed => Ok(DriftVerdict::Aligned { observed }),
        Some(b) => Ok(DriftVerdict::ShapeDrift { baseline: b, observed }),
    }
}

/// Compatibilité `ContractKind` désiré ↔ `kind` IST-observé. Borné (un drift de
/// kind est un vrai signal structurel) mais tolérant aux synonymes par langage.
fn ist_kind_matches(kind: ContractKind, ist_kind: &str) -> bool {
    let k = ist_kind.trim().to_ascii_lowercase();
    match kind {
        ContractKind::Function => matches!(k.as_str(), "function" | "method" | "fn"),
        ContractKind::Type => {
            matches!(k.as_str(), "struct" | "enum" | "type" | "typealias" | "union" | "class")
        }
        ContractKind::Interface => matches!(k.as_str(), "trait" | "interface" | "protocol"),
        ContractKind::Module => {
            matches!(k.as_str(), "module" | "mod" | "file_context" | "namespace" | "package")
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// S7 — surface de consommation (REQ-AXO-902094)
// ════════════════════════════════════════════════════════════════════════

/// Résumé dénormalisé d'un contrat pour la LISTE S7 — lu en un seul SELECT (pas de
/// N+1) : forme désirée minimale + cycle de vie + état du sceau. `sealed`/`adequate`
/// sont dérivés des colonnes `seal_hash`/`adequacy_verdict` (le sceau réel est
/// chargé à l'inspection via [`load_seal`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractSummary {
    pub id: String,
    pub kind: ContractKind,
    pub signature: String,
    /// Cycle de vie stocké : `planned` | `bound` | `sealed`.
    pub status: String,
    pub realized_by: Option<String>,
    /// `true` si `seal_hash` est présent (contrat scellé).
    pub sealed: bool,
    /// `Some(true/false)` = verdict d'adéquation du sceau ; `None` = non scellé.
    pub adequate: Option<bool>,
    pub seal_revision: Option<i64>,
}

/// Une arête du graphe de contrats (`soll.ContractEdge`) : gouvernance
/// (`SOLVES`/`EXPLAINS` → soll.Node) ou identité (`REALIZED_BY` → ist.Symbol).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractEdgeRow {
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
}

/// Liste paginée des contrats d'un projet (S7). Ordre stable `id ASC` pour une
/// pagination cognitive déterministe (GUI-AXO-1004). Une ligne au `kind` corrompu
/// (tag inconnu) est SAUTÉE plutôt que de faire échouer toute la liste.
pub fn list_contracts(
    store: &GraphStore,
    project_code: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<ContractSummary>> {
    let raw = store.query_json_param(
        "SELECT id, kind, signature, status, realized_by, seal_hash, adequacy_verdict, seal_revision
           FROM soll.Contract
          WHERE project_code = $project_code
          ORDER BY id ASC
          LIMIT $limit OFFSET $offset",
        &serde_json::json!({
            "project_code": project_code,
            "limit": limit,
            "offset": offset,
        }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(id) = col_str(&row, 0) else { continue };
        let Some(kind) = col_str(&row, 1).and_then(|t| ContractKind::from_tag(&t)) else {
            continue; // kind corrompu : on saute, pas d'échec global
        };
        let seal_hash = col_str(&row, 5);
        let sealed = seal_hash.is_some();
        out.push(ContractSummary {
            id,
            kind,
            signature: col_str(&row, 2).unwrap_or_default(),
            status: col_str(&row, 3).unwrap_or_else(|| "planned".to_string()),
            realized_by: col_str(&row, 4),
            sealed,
            adequate: if sealed {
                Some(col_str(&row, 6).as_deref() == Some("adequate"))
            } else {
                None
            },
            seal_revision: col_i64(&row, 7),
        });
    }
    Ok(out)
}

/// Total de contrats d'un projet (pour la pagination de [`list_contracts`]).
pub fn count_contracts(store: &GraphStore, project_code: &str) -> Result<i64> {
    store.query_count_param(
        "SELECT count(*) FROM soll.Contract WHERE project_code = $project_code",
        &serde_json::json!({ "project_code": project_code }),
    )
}

/// Arêtes incidentes d'un contrat, partitionnées par direction relative à `id` :
/// `(sortantes, entrantes)`. Sortantes = gouvernance + identité que CE contrat
/// porte (`impact` aval, le squelette navigable) ; entrantes = qui pointe VERS lui
/// (rare aujourd'hui — contrat→contrat non modélisé — mais surfacé pour l'`impact`
/// amont, symétrique de l'IST).
pub fn contract_edges(
    store: &GraphStore,
    id: &str,
) -> Result<(Vec<ContractEdgeRow>, Vec<ContractEdgeRow>)> {
    let raw = store.query_json_param(
        "SELECT source_id, target_id, relation_type
           FROM soll.ContractEdge
          WHERE source_id = $id OR target_id = $id
          ORDER BY relation_type, target_id",
        &serde_json::json!({ "id": id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let (mut outgoing, mut incoming) = (Vec::new(), Vec::new());
    for row in rows {
        let edge = ContractEdgeRow {
            source_id: col_str(&row, 0).unwrap_or_default(),
            target_id: col_str(&row, 1).unwrap_or_default(),
            relation_type: col_str(&row, 2).unwrap_or_default(),
        };
        if edge.source_id == id {
            outgoing.push(edge);
        } else {
            incoming.push(edge);
        }
    }
    Ok((outgoing, incoming))
}

// ════════════════════════════════════════════════════════════════════════
// helpers internes
// ════════════════════════════════════════════════════════════════════════

struct ReconcileRow {
    kind: ContractKind,
    realized_by: Option<String>,
    baseline: Option<String>,
}

fn load_reconcile_row(store: &GraphStore, id: &str) -> Result<Option<ReconcileRow>> {
    let raw = store.query_json_param(
        "SELECT kind, realized_by, observed_shape_hash
           FROM soll.Contract WHERE id = $id",
        &serde_json::json!({ "id": id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let kind_tag = col_str(&row, 0).ok_or_else(|| anyhow!("kind manquant"))?;
    let kind = ContractKind::from_tag(&kind_tag)
        .ok_or_else(|| anyhow!("kind persisté inconnu: {kind_tag}"))?;
    Ok(Some(ReconcileRow {
        kind,
        realized_by: col_str(&row, 1),
        baseline: col_str(&row, 2),
    }))
}

fn contract_realized_by(store: &GraphStore, id: &str) -> Result<Option<String>> {
    let raw = store.query_json_param(
        "SELECT realized_by FROM soll.Contract WHERE id = $id",
        &serde_json::json!({ "id": id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(rows.into_iter().next().and_then(|r| col_str(&r, 0)))
}

/// Lookup (kind, name) d'un symbole IST par id. `None` = symbole absent (l'ancre
/// d'identité a bougé → candidat rename).
fn ist_symbol(store: &GraphStore, symbol_id: &str) -> Result<Option<(String, String)>> {
    let raw = store.query_json_param(
        "SELECT kind, name FROM ist.Symbol WHERE id = $id",
        &serde_json::json!({ "id": symbol_id }),
    )?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let kind = col_str(&row, 0).unwrap_or_default();
    let name = col_str(&row, 1).unwrap_or_default();
    Ok(Some((kind, name)))
}

fn upsert_edge(
    store: &GraphStore,
    source_id: &str,
    target_id: &str,
    relation_type: &str,
    project_code: &str,
) -> Result<()> {
    store.execute_param(
        "INSERT INTO soll.ContractEdge (source_id, target_id, relation_type, project_code)
         VALUES ($source_id, $target_id, $relation_type, $project_code)
         ON CONFLICT (source_id, target_id, relation_type) DO NOTHING",
        &serde_json::json!({
            "source_id": source_id,
            "target_id": target_id,
            "relation_type": relation_type,
            "project_code": project_code,
        }),
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `TYPE-PROJ-N` → `PROJ` (DEC-AXO-085) ; défaut `AXO` pour les ids hors-format.
fn project_code_from_id(id: &str) -> String {
    id.split('-')
        .nth(1)
        .filter(|p| p.len() == 3 && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        .map(|p| p.to_string())
        .unwrap_or_else(|| "AXO".to_string())
}

/// Lit une colonne en `Option<String>`. Le render writer
/// (`postgres::native::render_pg_value`) sérialise CHAQUE cellule en chaîne et un
/// SQL `NULL` devient le littéral `"null"` — on le replie donc sur `None` (idiome
/// connu de ce chemin de rendu `Vec<Vec<String>>`).
fn col_str(row: &[Value], idx: usize) -> Option<String> {
    let s = match row.get(idx) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => return None,
        Some(other) => other.to_string(),
    };
    if s == "null" {
        None
    } else {
        Some(s)
    }
}

fn col_i64(row: &[Value], idx: usize) -> Option<i64> {
    col_str(row, idx).and_then(|s| s.parse().ok())
}

/// post_conditions est un JSONB array. Le render writer le renvoie comme une
/// `Value::String` (JSON sérialisé, p.ex. `["sorted","dedup"]`) ; on tolère aussi
/// une `Value::Array` au cas où un chemin de rendu le renverrait nested.
fn parse_post_conditions(v: Option<&Value>) -> Vec<PostCondition> {
    let arr: Vec<String> = match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        _ => Vec::new(),
    };
    arr.into_iter().map(PostCondition).collect()
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
