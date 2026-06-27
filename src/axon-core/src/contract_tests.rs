//! Tests TDD de la tranche 1 (REQ-AXO-902088/89/90). Le test central reproduit
//! en Rust le test négatif validé par le prototype VAL-AXO-149 : un `proves` faible
//! laisse SURVIVRE des mutants réels de `parse_seq_buckets_from_env` et doit donc
//! FAIRE ÉCHOUER le sceau. Contrat ancre : REQ-AXO-262 (embedder/gpu_backend.rs).

use super::*;
use crate::contract::adequacy::{assess, AdequacyThresholds};
use crate::contract::binding::{bind, BindingSource};
use crate::contract::certification::certify;
use crate::contract::expand::{
    derive_postconditions, next_pull, propose_from_observed, should_expand, ExpansionStatus,
};
use crate::contract::seal::{seal_node, structural_seal, EmpiricalAttestation, Verdict};

type Impl = fn(Option<&str>) -> Vec<usize>;

const DEFAULT: [usize; 4] = [128, 256, 384, 512];

// --- La fonction réelle, portée fidèlement (référence). ---------------------
fn reference(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT.to_vec(),
    };
    if raw.is_empty()
        || raw == "0"
        || raw.eq_ignore_ascii_case("off")
        || raw.eq_ignore_ascii_case("none")
    {
        return Vec::new();
    }
    let mut out: Vec<usize> = raw
        .split(',')
        .filter_map(|t| t.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        DEFAULT.to_vec()
    } else {
        out
    }
}

// --- Cinq mutants réels (un point de mutation chacun). ----------------------
fn mut_no_sort(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT.to_vec(),
    };
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut out: Vec<usize> =
        raw.split(',').filter_map(|t| t.trim().parse::<usize>().ok()).filter(|v| *v > 0).collect();
    out.dedup(); // SDL: sort retiré
    if out.is_empty() { DEFAULT.to_vec() } else { out }
}

fn mut_no_dedup(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT.to_vec(),
    };
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut out: Vec<usize> =
        raw.split(',').filter_map(|t| t.trim().parse::<usize>().ok()).filter(|v| *v > 0).collect();
    out.sort_unstable(); // SDL: dedup retiré
    if out.is_empty() { DEFAULT.to_vec() } else { out }
}

// La mutation EST `> -> >=` ; sur usize `>= 0` est toujours vrai (c'est le bug
// injecté qui laisse passer 0). Le warning `unused_comparisons` est donc attendu
// et fait partie du mutant — silencé localement (GUI-PRO-003 zéro-warning).
#[allow(unused_comparisons)]
fn mut_ge_zero(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT.to_vec(),
    };
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut out: Vec<usize> = raw
        .split(',')
        .filter_map(|t| t.trim().parse::<usize>().ok())
        .filter(|v| *v >= 0) // ROR: > -> >= (laisse passer 0)
        .collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() { DEFAULT.to_vec() } else { out }
}

fn mut_disable_default(raw: Option<&str>) -> Vec<usize> {
    let raw = match raw {
        Some(v) => v.trim(),
        None => return DEFAULT.to_vec(),
    };
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("none") {
        return DEFAULT.to_vec(); // BRANCH: disable renvoie DEFAULT au lieu de []
    }
    let mut out: Vec<usize> =
        raw.split(',').filter_map(|t| t.trim().parse::<usize>().ok()).filter(|v| *v > 0).collect();
    out.sort_unstable();
    out.dedup();
    if out.is_empty() { DEFAULT.to_vec() } else { out }
}

fn mut_always_empty(_raw: Option<&str>) -> Vec<usize> {
    Vec::new() // SDL: corps réduit à un stub
}

fn mutants() -> Vec<Impl> {
    vec![mut_no_sort, mut_no_dedup, mut_ge_zero, mut_disable_default, mut_always_empty]
}

// --- Deux bundles `proves`. -------------------------------------------------
fn proves_weak(f: Impl) -> bool {
    // Happy-path unique : trié, sans doublon, positif. Passe sur beaucoup de bugs.
    f(Some("128,256")) == vec![128, 256]
}

fn proves_strong(f: Impl) -> bool {
    f(Some("256,128")) == vec![128, 256]                 // sorted
        && f(Some("128,128,256")) == vec![128, 256]       // dedup
        && f(Some("0,128")) == vec![128]                  // positive (0 dropped)
        && f(Some("off")) == Vec::<usize>::new()          // disable -> empty
        && f(None) == DEFAULT.to_vec()                    // none -> default
        && f(Some("abc")) == DEFAULT.to_vec()             // all-invalid -> default
}

/// `true` = mutant TUÉ (le bundle échoue dessus). La référence DOIT passer.
fn outcomes(proves: fn(Impl) -> bool, mutants: &[Impl]) -> Vec<bool> {
    assert!(proves(reference), "le bundle doit passer sur la référence");
    mutants.iter().map(|m| !proves(*m)).collect()
}

fn anchor_contract() -> ContractNode {
    ContractNode {
        kind: ContractKind::Function,
        signature: "parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<usize>".to_string(),
        why: "SOLVES REQ-AXO-262".to_string(),
        post_conditions: vec![
            PostCondition("sorted".into()),
            PostCondition("dedup".into()),
            PostCondition("positive".into()),
            PostCondition("disable->empty".into()),
            PostCondition("none->default".into()),
            PostCondition("allinvalid->default".into()),
        ],
        proves_ref: "proves:anchor".to_string(),
        realized_by: None,
    }
}

// ===========================================================================
// TEST NÉGATIF (le make-or-break) : proves faible -> sceau REFUSÉ.
// ===========================================================================
#[test]
fn weak_proves_fails_the_seal() {
    let node = anchor_contract();
    let killed = outcomes(proves_weak, &mutants());
    // coverage modélisée (S2-complet la calculera) : faible ne discrimine pas
    // disable/none/allinvalid -> 3/6.
    let report = assess(&killed, 3.0 / 6.0, &AdequacyThresholds::default());

    assert_eq!(report.total, 5);
    assert!(report.kill_rate < 0.80, "kill_rate faible attendu, eu {}", report.kill_rate);
    assert!(!report.passed, "un proves faible NE doit PAS passer l'adéquation");

    let seal = structural_seal(&node.shape_hash(), &node.proves_ref, report.passed, &[]);
    assert!(seal.is_none(), "proves faible -> AUCUN sceau (anti théâtre du sceau)");
}

// ===========================================================================
// TEST POSITIF : proves fort -> sceau accordé.
// ===========================================================================
#[test]
fn strong_proves_grants_the_seal() {
    let node = anchor_contract();
    let killed = outcomes(proves_strong, &mutants());
    let report = assess(&killed, 1.0, &AdequacyThresholds::default());

    assert_eq!((report.killed, report.total), (5, 5));
    assert!((report.kill_rate - 1.0).abs() < f64::EPSILON);
    assert!(report.passed);

    let seal = structural_seal(&node.shape_hash(), &node.proves_ref, report.passed, &[]);
    assert!(seal.is_some(), "proves fort + adéquat -> sceau accordé");
}

#[test]
fn empirical_attestation_is_out_of_the_structural_hash() {
    let node = anchor_contract();
    let seal_green = structural_seal(&node.shape_hash(), "p", true, &[]);
    let seal_red = structural_seal(&node.shape_hash(), "p", true, &[]);
    assert_eq!(seal_green, seal_red, "le sceau structurel ne dépend pas du run empirique");

    let green = EmpiricalAttestation::new(Verdict::Green, 1_000, 3_600);
    let red = EmpiricalAttestation::new(Verdict::Red, 1_000, 3_600);
    assert_ne!(green.result, red.result);
    assert!(green.is_fresh(1_000));
    assert!(!green.is_fresh(1_000 + 3_601 * 1_000));
}

#[test]
fn parent_unsealed_when_required_child_unsealed() {
    let parent_shape = "parent_shape";
    let child_sealed = structural_seal("child_shape", "p", true, &[]);
    assert!(child_sealed.is_some());

    // enfant requis non scellé (None) -> parent non scellable
    let blocked = seal_node(parent_shape, "p", true, &[None]);
    assert!(blocked.is_none(), "parent non scellable si un enfant requis n'est pas scellé");

    // enfant requis scellé -> parent scellable
    let ok = seal_node(parent_shape, "p", true, &[child_sealed]);
    assert!(ok.is_some());
}

#[test]
fn validate_rejects_malformed_contracts() {
    let mut node = anchor_contract();
    assert_eq!(node.validate(), Ok(()));

    node.signature = "parse_seq_buckets(raw)".to_string(); // pas de '->'
    assert_eq!(node.validate(), Err(ContractError::UntypedSignature));

    let mut node2 = anchor_contract();
    node2.post_conditions.clear();
    assert_eq!(node2.validate(), Err(ContractError::NoPostConditions));
}

// ===========================================================================
// S4 (REQ-AXO-902091) — binding + certification (le keeper DEC-AXO-901656).
// ===========================================================================
#[test]
fn binding_prefers_proof_witness_over_anchor() {
    // Couverture ET ancre présentes -> le témoignage par la preuve prime.
    let b = bind("CON-1", Some("sym::witnessed"), Some("sym::anchor")).unwrap();
    assert_eq!(b.symbol_id, "sym::witnessed");
    assert_eq!(b.source, BindingSource::ProofWitnessed);
}

#[test]
fn binding_falls_back_to_anchor_then_none() {
    let anchored = bind("CON-1", None, Some("sym::anchor")).unwrap();
    assert_eq!(anchored.source, BindingSource::IdentityAnchor);
    assert_eq!(anchored.symbol_id, "sym::anchor");

    assert!(bind("CON-1", None, None).is_none(), "ni couverture ni ancre -> pas de binding");
}

#[test]
fn certification_is_derived_never_declared() {
    // Aucun chemin pour certifier sans (vert ET adéquat) : pas de preuve verte
    // ou pas d'adéquation -> aucune Certification, quelle que soit la "déclaration".
    assert!(certify(false, true, "code_v1", "evidence").is_none(), "non-vert -> pas de certif");
    assert!(certify(true, false, "code_v1", "evidence").is_none(), "non-adéquat -> pas de certif");
    assert!(certify(true, true, "code_v1", "evidence").is_some(), "vert + adéquat -> certif dérivée");
}

#[test]
fn certification_is_invalidated_by_code_change() {
    let cert = certify(true, true, "code_v1", "evidence").unwrap();
    assert!(cert.is_valid_for("code_v1"), "valide pour l'état de code prouvé");
    assert!(!cert.is_valid_for("code_v2"), "vert-périmé : code changé -> certif invalide");
}

// ===========================================================================
// S5 (REQ-AXO-902092) — design_expand pull + auto-amorçage legacy.
// ===========================================================================
#[test]
fn legacy_bootstrap_proposes_observed_contract_not_abduced() {
    // Données réellement observées dans l'IST pour parse_seq_buckets (REQ-262).
    let tests = [
        "parse_seq_buckets_normalizes_input",
        "parse_seq_buckets_skips_non_numeric_and_zero",
        "parse_seq_buckets_defaults_to_canonical_list",
        "parse_seq_buckets_explicit_disable_returns_empty",
    ];
    let node = propose_from_observed(
        ContractKind::Function,
        "parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<usize>",
        "SOLVES REQ-AXO-262",
        "embedder::gpu_backend::parse_seq_buckets_from_env",
        &tests,
    );
    // realized_by déjà rempli = OBSERVÉ (le code existe), pas abduit ex nihilo.
    assert!(node.realized_by.is_some(), "contrat proposé depuis l'IST observé");
    assert!(!node.post_conditions.is_empty(), "post-conditions dérivées des tests");
    assert_eq!(node.validate(), Ok(()), "un contrat auto-amorcé est bien formé");
}

#[test]
fn derive_postconditions_maps_test_names_and_dedups() {
    let conds = derive_postconditions(&["x_normalizes_input", "y_sorts_then_normalizes"]);
    // les deux mappent vers "normalized" -> dédupliqué
    assert_eq!(conds, vec![PostCondition("normalized".into())]);
}

#[test]
fn expansion_is_bounded_to_seams_pull_driven() {
    use ContractKind::*;
    use ExpansionStatus::*;
    // coutures expansables
    assert!(should_expand(Module, LeafPending));
    assert!(should_expand(Interface, LeafPending));
    // feuilles / déjà fait -> jamais (compilateur+tests = oracle des feuilles)
    assert!(!should_expand(Function, LeafPending));
    assert!(!should_expand(Module, Frozen));
    assert!(!should_expand(Interface, Expanded));

    // pull : prend la première couture expansable, None si tout figé/feuille
    let frontier = [(Function, LeafPending), (Interface, LeafPending), (Module, LeafPending)];
    assert_eq!(next_pull(&frontier), Some(1));
    assert_eq!(next_pull(&[(Function, LeafPending), (Type, Frozen)]), None);
}

#[test]
fn shape_hash_is_deterministic_and_signature_sensitive() {
    let a = anchor_contract();
    let b = anchor_contract();
    assert_eq!(a.shape_hash(), b.shape_hash());

    let mut c = anchor_contract();
    c.signature = "parse_seq_buckets_from_env(raw: Option<&str>) -> Vec<u32>".to_string();
    assert_ne!(a.shape_hash(), c.shape_hash(), "un changement de signature change le shape_hash");
}
