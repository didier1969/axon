//! S4 (REQ-AXO-902091) — binding contrat↔code (DEC-AXO-901656).
//!
//! Deux sources, asymétriques : le *témoignage par la preuve* (le run du bundle
//! `proves` a exercé le symbole sous trace de couverture) prime ; l'*ancre
//! d'identité* `realizes:<id>` n'est qu'un repli pour la stabilité au rename —
//! une revendication, pas une preuve. Le binding établit QUEL code réalise le
//! contrat ; il ne dit RIEN de la certification (cf. [`super::certification`]).

/// Comment le binding a été établi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingSource {
    /// Le run `proves` a exercé le symbole (trace de couverture). Prioritaire.
    ProofWitnessed,
    /// Ancre `realizes:<id>` déclarée — repli pour la stabilité au rename.
    IdentityAnchor,
}

/// Lien établi entre un contrat et le symbole qui le réalise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub contract_id: String,
    pub symbol_id: String,
    pub source: BindingSource,
}

/// Lie un contrat à son symbole. `coverage_symbol` = symbole touché par le run
/// `proves` (témoignage) ; `anchor_symbol` = symbole déclaré par l'ancre. Le
/// témoignage prime ; l'ancre est le repli ; aucun des deux ⇒ `None`.
pub fn bind(
    contract_id: &str,
    coverage_symbol: Option<&str>,
    anchor_symbol: Option<&str>,
) -> Option<Binding> {
    let (symbol_id, source) = match (coverage_symbol, anchor_symbol) {
        (Some(sym), _) => (sym, BindingSource::ProofWitnessed),
        (None, Some(sym)) => (sym, BindingSource::IdentityAnchor),
        (None, None) => return None,
    };
    Some(Binding {
        contract_id: contract_id.to_string(),
        symbol_id: symbol_id.to_string(),
        source,
    })
}
