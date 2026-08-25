pub(crate) fn format_table_from_json(json_res: &str, headers: &[&str]) -> String {
    let rows: Vec<Vec<serde_json::Value>> = match serde_json::from_str(json_res) {
        Ok(r) => r,
        Err(_) => return format!("Formatting error: {}", json_res),
    };

    if rows.is_empty() {
        return "No results found.".to_string();
    }

    let mut output = String::new();

    output.push('|');
    for h in headers {
        output.push_str(&format!(" {} |", h));
    }
    output.push('\n');

    output.push('|');
    for _ in headers {
        output.push_str(" --- |");
    }
    output.push('\n');

    for row in rows {
        output.push('|');
        for val in row {
            let clean_val = match val {
                serde_json::Value::Null => "null".to_string(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::String(v) => v,
                serde_json::Value::Array(v) => {
                    serde_json::to_string(&v).unwrap_or_else(|_| "[]".to_string())
                }
                serde_json::Value::Object(v) => {
                    serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_string())
                }
            };
            output.push_str(&format!(" {} |", clean_val));
        }
        output.push('\n');
    }

    output
}

pub(crate) fn format_standard_contract(
    status: &str,
    summary: &str,
    scope: &str,
    evidence: &str,
    next_actions: &[&str],
    confidence: &str,
) -> String {
    let actions = if next_actions.is_empty() {
        "- none".to_string()
    } else {
        next_actions
            .iter()
            .map(|item| format!("- {}", item))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "**Status:** {}\n\
         **Summary:** {}\n\
         **Scope:** {}\n\
         **Confidence:** {}\n\n\
         ### Evidence\n{}\n\n\
         ### Next actions\n{}\n",
        status, summary, scope, confidence, evidence, actions
    )
}

pub(crate) fn evidence_by_mode(evidence: &str, mode: Option<&str>) -> String {
    let normalized = mode.unwrap_or("brief").to_ascii_lowercase();
    if normalized == "verbose" {
        return evidence.to_string();
    }
    let max_chars = 4000usize;
    if evidence.chars().count() <= max_chars {
        return evidence.to_string();
    }
    let mut end = evidence.len();
    for (count, (idx, _)) in evidence.char_indices().enumerate() {
        if count == max_chars {
            end = idx;
            break;
        }
    }
    let mut clipped = evidence[..end].to_string();
    clipped.push_str("\n\n[truncated=true, mode=brief, max_chars=4000]");
    clipped
}


/// REQ-AXO-902409 tranche 3 — un compte qui dit COMMENT il a été obtenu.
///
/// Invariant KKI (doléance #204, `blocking`) : *aucun outil ne rend une valeur
/// numérique ou nommée pour une grandeur qu'il n'a pas calculée. L'état « non
/// calculé » est un état de premier rang, distinct de zéro et distinct de vide.*
///
/// Trois états, parce que le dépôt en confondait trois en un :
///
/// | état | rendu | ce que ça évitait |
/// |---|---|---|
/// | mesuré, complet | `"7"` | — |
/// | mesuré, tronqué | `"20 sur 137"` | `anomalies` publiait `20` pour un plafond de collecte de 20 : « au moins 20 » se lisait « exactement 20 » |
/// | non calculé | `"non calculé (raison)"` | `anomalies project="*"` rendait DIX zéros fabriqués et un vrai chiffre, visuellement identiques |
///
/// ⚠️ **Ne PAS décorer un compte complet.** `Detours: 7` était sous le plafond,
/// donc vrai : lui ajouter un plancher fallacieux serait une régression. C'est
/// `exact` qui l'exprime, et c'est pour ça que les deux cas sont deux
/// constructeurs distincts plutôt qu'un booléen à interpréter.
///
/// Cette forme n'est pas inventée ici : elle GÉNÉRALISE celle qui opère déjà dans
/// la section `dry` de `debt_digest` (« 2320 au total, 3 affiché(s) — 177 pair(s)
/// withheld as unopenable ») et dans `sample_identities` (« showing N of M »,
/// REQ-AXO-902279). Ce que ces deux-là ne savent pas dire, c'est « je n'ai pas
/// calculé » — et `sample_identities` ne peut pas voir une troncature survenue
/// AVANT elle, ce qui est exactement le cas d'`anomalies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Compte {
    /// La grandeur a été mesurée. `rendus <= total` ; l'égalité dit « complet ».
    Mesure { total: usize, rendus: usize },
    /// La grandeur n'a PAS été calculée, et la raison est publiée avec.
    NonCalcule(&'static str),
    /// La RECHERCHE elle-même était bornée : `au_moins` trouvés avant la borne, et
    /// le total reste **inconnu**.
    ///
    /// Distinct de `Mesure { tronqué }`, où le total EST connu et où seul
    /// l'affichage est coupé. Mesuré sur `structural_health_worklist` :
    /// `scan_cap = top × 3` fait dépendre la PROFONDEUR DE RECHERCHE du paramètre
    /// d'AFFICHAGE — `coupling` valait 4 à `top=200` et 3 à `top=1`. Aucun de ces
    /// deux nombres n'est un total ; publier l'un ou l'autre comme tel serait faux
    /// quel que soit l'endroit où on compte.
    Plancher {
        au_moins: usize,
        raison: &'static str,
    },
}

impl Compte {
    /// Mesuré et complet — rien n'a été écarté.
    pub(crate) fn exact(total: usize) -> Self {
        Compte::Mesure {
            total,
            rendus: total,
        }
    }

    /// Mesuré puis borné : `total` trouvés, `rendus` publiés.
    ///
    /// `rendus >= total` retombe sur `exact` : un « 7 sur 7 » n'apprend rien et
    /// entraînerait le lecteur à chercher une troncature qui n'existe pas.
    pub(crate) fn borne(total: usize, rendus: usize) -> Self {
        if rendus >= total {
            Compte::exact(total)
        } else {
            Compte::Mesure { total, rendus }
        }
    }

    /// Pas de mesure — et pourquoi. La raison est `&'static str` À DESSEIN : elle
    /// décrit une branche du code, pas une donnée d'exécution, donc elle ne peut
    /// pas être fabriquée à partir d'une valeur observée.
    pub(crate) fn non_calcule(raison: &'static str) -> Self {
        Compte::NonCalcule(raison)
    }

    /// Plancher SI la recherche a saturé sa borne, compte exact sinon.
    ///
    /// C'est le constructeur à utiliser quand une borne de scan existe : il évite
    /// d'apposer « ≥ » sur un compte qui, se trouvant sous la borne, est complet —
    /// le faux positif qui pousse à désaffuter la règle.
    pub(crate) fn plancher_si_sature(trouves: usize, borne: usize, raison: &'static str) -> Self {
        if trouves >= borne {
            Compte::Plancher {
                au_moins: trouves,
                raison,
            }
        } else {
            Compte::exact(trouves)
        }
    }

    pub(crate) fn rendre(&self) -> String {
        match self {
            Compte::Mesure { total, rendus } if rendus >= &total => total.to_string(),
            Compte::Mesure { total, rendus } => format!("{rendus} sur {total}"),
            Compte::NonCalcule(raison) => format!("non calculé ({raison})"),
            Compte::Plancher { au_moins, raison } => format!("≥ {au_moins} ({raison})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-AXO-902190 — format_standard_contract is the shared MCP output contract (24 callers,
    // a top uncovered hub). Cover its two branches: full render + the empty-actions "- none".
    #[test]
    fn format_standard_contract_renders_all_sections() {
        let out = format_standard_contract(
            "ok",
            "did the thing",
            "project:AXO",
            "commit abc123",
            &["run tests", "ship it"],
            "high",
        );
        assert!(out.contains("**Status:** ok"));
        assert!(out.contains("**Summary:** did the thing"));
        assert!(out.contains("**Scope:** project:AXO"));
        assert!(out.contains("**Confidence:** high"));
        assert!(out.contains("### Evidence\ncommit abc123"));
        assert!(out.contains("- run tests\n- ship it"));
    }

    #[test]
    fn format_standard_contract_empty_actions_renders_none() {
        let out = format_standard_contract("ok", "s", "sc", "e", &[], "low");
        assert!(out.contains("### Next actions\n- none"));
    }

    /// REQ-AXO-902409 tranche 3 — les trois états, et surtout celui du MILIEU.
    ///
    /// Le piège de cette primitive n'est pas de savoir dire « non calculé » : c'est
    /// de ne PAS décorer un compte qui est complet. `Detours: 7` était sous le
    /// plafond de collecte, donc juste ; lui coller « 7 sur 7 » enverrait le lecteur
    /// chercher une troncature qui n'existe pas, et c'est le genre de faux positif
    /// qui pousse à désaffuter une règle (deux formulations de ce nœud sont déjà
    /// mortes ainsi).
    #[test]
    fn a_count_says_how_it_was_obtained_and_never_decorates_a_complete_one() {
        assert_eq!(Compte::exact(7).rendre(), "7");
        assert_eq!(Compte::exact(0).rendre(), "0", "un vrai zéro reste un zéro");
        assert_eq!(Compte::borne(137, 20).rendre(), "20 sur 137");
        // Complet malgré un plafond : aucune décoration.
        assert_eq!(Compte::borne(7, 20).rendre(), "7");
        assert_eq!(Compte::borne(20, 20).rendre(), "20");
        assert_eq!(
            Compte::non_calcule("instantané froid").rendre(),
            "non calculé (instantané froid)"
        );
        // Recherche bornée : le total est INCONNU, pas seulement l'affichage coupé.
        assert_eq!(
            Compte::plancher_si_sature(45, 45, "recherche bornée").rendre(),
            "≥ 45 (recherche bornée)"
        );
        // Sous la borne : la recherche est allée au bout, aucune décoration.
        assert_eq!(Compte::plancher_si_sature(3, 45, "recherche bornée").rendre(), "3");
    }

    /// REQ-AXO-902409 — un zéro MESURÉ et un zéro NON CALCULÉ ne se rendent pas
    /// pareil. C'est l'invariant KKI #204 en une assertion : c'est exactement cette
    /// confusion qui faisait lire « aucune anomalie » là où rien n'avait été cherché.
    #[test]
    fn a_measured_zero_and_an_uncomputed_one_never_render_alike() {
        let mesure = Compte::exact(0).rendre();
        let inconnu = Compte::non_calcule("instantané IST indisponible").rendre();
        assert_ne!(
            mesure, inconnu,
            "un zéro mesuré et une absence de mesure doivent être distinguables \
             DANS LE TEXTE — c'est le seul canal que le client expose"
        );
        assert!(
            inconnu.parse::<u64>().is_err(),
            "l'état « non calculé » ne doit pas pouvoir se lire comme un nombre : {inconnu}"
        );
    }
}
