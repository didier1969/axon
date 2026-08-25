//! REQ-AXO-902433 — l'heure murale, en un seul endroit et avec UNE politique.
//!
//! RÉUTILISE : néant — vérifié via `axon query "now_ms"` et `axon query "now_unix_ms"`
//! (aucun module d'horloge partagé n'existe ; ce fichier EST la brique manquante).
//! Ce module REMPLACE 11 définitions locales, recensées et migrées ici :
//! `optimizer.rs` · `runtime_readiness.rs` · `runtime_watchdog.rs` · `service_guard.rs` ·
//! `contract/store.rs` · `mcp.rs::now_unix_ms` · `mcp/tools_soll/storage.rs` ·
//! `soll_snapshot/snapshot.rs` · `watchman_source.rs` · `embedder/lifecycle_machine.rs` ·
//! `bin/axonctl.rs`.
//!
//! Le dépôt portait **douze** définitions d'horloge, réparties sur trois types de
//! retour (`i64`, `u64`, `u128`) et **deux politiques de débordement
//! incompatibles dans le même binaire** :
//!
//! | site | au-delà de `i64::MAX` |
//! |---|---|
//! | `watchman_source.rs`, `embedder/lifecycle_machine.rs` | `.min(i64::MAX)` — **saturation explicite** |
//! | `contract/store.rs`, `mcp.rs`, `soll_snapshot`, `tools_soll/storage.rs`, … | `as i64` — **troncature silencieuse**, résultat arbitraire et possiblement NÉGATIF |
//!
//! Aucune des deux n'est fausse. C'est de ne pas savoir laquelle on obtient qui
//! coûte — et la session 121 avait déjà payé ce prix sur un autre couple de
//! constantes (`SAFE_CHARS_PER_TOKEN`=4 face à `FALLBACK_CHARS_PER_TOKEN`=3,
//! 33 % d'écart par construction).
//!
//! ## La politique, choisie et écrite
//!
//! 1. **Un seul type : `i64`.** C'est celui qu'attend PostgreSQL (`bigint`), donc
//!    celui qui traverse la persistance sans conversion.
//! 2. **Antérieur à l'époque → `0`.** Une horloge mal réglée ne doit pas produire
//!    un instant négatif qui se propagerait dans des soustractions.
//! 3. **Au-delà de `i64::MAX` → saturation**, jamais de troncature. Un `as i64`
//!    sur un `u128` trop grand rend une valeur arbitraire ; la saturation rend
//!    une valeur fausse mais **monotone et positive**, qui casse visiblement au
//!    lieu de mentir discrètement.
//!
//! La valeur est donc toujours dans `[0, i64::MAX]` : un appelant qui veut un
//! `u64` ou un `u128` convertit sans risque (`as u64`, `as u128`).
//!
//! ## Ce que cette dette n'était pas
//!
//! Aucun défaut observé : `as_millis()` ne dépasse `i64::MAX` qu'après ~292
//! millions d'années. C'est une **dette nommée**, traitée pour elle-même — et
//! trouvée par `debt_digest`, pas par une relecture. À noter pour le triage :
//! `semantic_clones` ne voyait rien sur cette famille (plancher cosinus 0,10).

use std::time::{SystemTime, UNIX_EPOCH};

/// Millisecondes depuis l'époque Unix — **la** source d'heure murale du dépôt.
///
/// Voir la politique en tête de module. Garantie : le résultat est dans
/// `[0, i64::MAX]`.
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_stays_inside_its_declared_range() {
        let t = now_unix_ms();
        assert!(t > 0, "l'heure murale doit etre positive, got {t}");
        // 2020-01-01 — un plancher qui ne bougera plus, et qui attrape une
        // horloge revenue a l'epoque (le cas que la politique borne a 0).
        assert!(t > 1_577_836_800_000, "horloge anterieure a 2020 : {t}");
    }

    /// REQ-AXO-902433 — la garde qui est le vrai livrable.
    ///
    /// Fusionner les douze exemplaires ne vaut que si un treizième ne peut pas
    /// réapparaître. Chaque copie était écrite de bonne foi, localement, par
    /// quelqu'un qui ne savait pas que onze autres existaient — c'est
    /// exactement ce que `GUI-PRO-013` vise, et une fusion sans garde le
    /// laisserait recommencer.
    ///
    /// ⚠️ Portée DÉLIBÉRÉMENT étroite : la garde interdit une nouvelle
    /// **définition de fonction** d'horloge, pas tout appel en ligne à
    /// `duration_since(UNIX_EPOCH)`. `REQ-AXO-902326` a déjà payé le prix d'une
    /// garde statique sur-approximante (370 contrevenants pour un défaut réel
    /// ×40 plus petit), et il reste des appels en ligne dans le dépôt : les
    /// interdire d'un bloc ferait rougir la porte sans dire quoi corriger.
    /// Ce périmètre est mesuré et assumé, pas ignoré.
    #[test]
    fn no_second_wall_clock_definition_creeps_back() {
        let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut coupables: Vec<String> = Vec::new();

        fn parcourir(dir: &std::path::Path, racine: &std::path::Path, coupables: &mut Vec<String>) {
            let Ok(entrees) = std::fs::read_dir(dir) else {
                return;
            };
            for entree in entrees.flatten() {
                let chemin = entree.path();
                if chemin.is_dir() {
                    parcourir(&chemin, racine, coupables);
                    continue;
                }
                if chemin.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // Ce module EST la définition canonique.
                if chemin.file_name().and_then(|f| f.to_str()) == Some("clock.rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&chemin) else {
                    continue;
                };
                for (n, ligne) in source.lines().enumerate() {
                    let l = ligne.trim_start();
                    // Une DÉFINITION, pas un appel : `fn now_ms(` / `fn now_unix_ms(`.
                    // Les helpers de test explicitement nommés `*_for_tests` sont
                    // hors sujet : ils fabriquent une heure, ils ne la lisent pas.
                    let est_definition = (l.contains("fn now_ms(") || l.contains("fn now_unix_ms("))
                        && !l.contains("_for_tests");
                    if est_definition {
                        coupables.push(format!(
                            "{}:{}",
                            chemin
                                .strip_prefix(racine)
                                .unwrap_or(&chemin)
                                .to_string_lossy(),
                            n + 1
                        ));
                    }
                }
            }
        }
        parcourir(&racine, &racine, &mut coupables);
        coupables.sort();

        assert!(
            coupables.is_empty(),
            "{} definition(s) d'horloge murale hors de `clock.rs` :\n  {}\n\
             Chaque copie porte SA politique de debordement — c'est de ne pas savoir \
             laquelle on obtient qui coute. Utiliser `crate::clock::now_unix_ms()` \
             (converti en `as u64` / `as u128` si besoin : la valeur est garantie \
             dans [0, i64::MAX]).",
            coupables.len(),
            coupables.join("\n  ")
        );
    }
}
