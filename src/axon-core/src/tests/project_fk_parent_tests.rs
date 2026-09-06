// REQ-AXO-902626 — le parent FK de toute table IST.
//
// `ist.IndexedFile.project_code` est une FK NOT NULL vers `axon.Project(code)`.
// Le seul écrivain de production était l'UPSERT de REQ-AXO-901860 dans
// `bulk_writer` ; `c72cd227` (2026-08-28) l'a retiré au nom de REQ-AXO-902541
// — « ProjectCodeRegistry owns tenant creation » — sans jamais donner cette
// charge au registre. Résultat mesuré le 2026-09-06 : dernier enrôlement réussi
// le 2026-08-26, 20 codes au registre sans parent, dont 7 tenants vivants dont
// A3 refusait 100 % des lots sur `indexedfile_project_code_fkey`.
//
// Ces gardes tiennent les deux moitiés du correctif — le pont (l'enrôlement
// écrit le parent) et la réconciliation (le passé est soldé) — et la
// réconciliation en a DEUX, symétriques : sans celle qui vérifie qu'on enrôle,
// une boucle vide passerait celle qui vérifie qu'on n'enrôle pas trop.

#[cfg(test)]
mod tests {
    use crate::project_meta::CanonicalProjectIdentity;
    use crate::tests::test_helpers::{create_test_db, unique_test_scope};

    /// Code projet à 3 caractères, dérivé du scope pour que deux tests
    /// parallèles ne se disputent pas la même ligne (même motif que
    /// `rescan_project_tests`).
    fn three_char_code_from_scope(scope: &str) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut hash: u64 = 1469598103934665603;
        for b in scope.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(1099511628211);
        }
        let mut out = String::with_capacity(3);
        for i in 0..3 {
            let idx = ((hash >> (i * 12)) as usize) % ALPHABET.len();
            out.push(ALPHABET[idx] as char);
        }
        out
    }

    fn parent_rows(store: &crate::graph::GraphStore, code: &str) -> i64 {
        let raw = store
            .query_json_param(
                "SELECT COUNT(*) FROM axon.Project WHERE code = ?",
                &serde_json::json!([code]),
            )
            .expect("compter les lignes parentes");
        let rows: Vec<Vec<serde_json::Value>> =
            serde_json::from_str(&raw).unwrap_or_default();
        rows.first()
            .and_then(|row| row.first())
            .map(|cell| match cell {
                serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
                serde_json::Value::String(s) => s.parse::<i64>().unwrap_or(0),
                _ => 0,
            })
            .unwrap_or(0)
    }

    /// Le pont : enrôler au registre DOIT écrire la ligne parente.
    /// Mutant : retirer l'appel à `ensure_project_fk_parent` dans
    /// `sync_project_registry_entry`.
    #[test]
    fn un_tenant_enrole_recoit_son_parent_fk() {
        let store = create_test_db().expect("create test db");
        let scope = unique_test_scope("fkp");
        let code = three_char_code_from_scope(&scope);
        let root = std::env::temp_dir().join(format!("fkparent-{scope}"));
        std::fs::create_dir_all(&root).expect("create project root");

        assert_eq!(
            parent_rows(&store, &code),
            0,
            "le code doit être vierge avant l'enrôlement, sinon la garde ne prouve rien"
        );

        store
            .sync_project_registry_entry(
                &code,
                Some("fk-parent-fixture"),
                Some(root.to_string_lossy().as_ref()),
            )
            .expect("enrôler le projet");

        assert_eq!(
            parent_rows(&store, &code),
            1,
            "l'enrôlement doit écrire axon.Project — sans ce parent, A3 refuse 100 % \
             des lots du tenant sur indexedfile_project_code_fkey (REQ-AXO-902626)"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// L'effet observable : un lot A3 d'un tenant fraîchement enrôlé passe la FK.
    /// C'est la garde qui parle le langage de la panne signalée par MRG, et non
    /// celui de la table.
    #[test]
    fn un_lot_a3_d_un_tenant_frais_est_accepte() {
        let store = create_test_db().expect("create test db");
        let scope = unique_test_scope("a3f");
        let code = three_char_code_from_scope(&scope);
        let root = std::env::temp_dir().join(format!("a3fresh-{scope}"));
        std::fs::create_dir_all(&root).expect("create project root");
        let fichier = root.join("lib.rs");
        std::fs::write(&fichier, "pub fn main() {}\n").expect("write fixture");

        // REQ-AXO-902626 — NEUTRALISER la béquille du harnais avant de mesurer.
        //
        // `test_db.rs:781` installe `trg_test_autoseed_indexedfile`, un BEFORE
        // INSERT qui fabrique lui-même `axon.Project` à chaque insertion dans
        // `ist.IndexedFile` — « the root-cause fix for the whole class of
        // `Writer Error: INSERT INTO ist.* ... FK` test failures », dit son
        // commentaire. Il fait exactement ce que la PRODUCTION a cessé de faire
        // quand `c72cd227` a retiré l'UPSERT d'A3 : c'est pourquoi ce retrait a
        // laissé la suite verte, et pourquoi 7 tenants ont perdu leur IST sans
        // qu'aucune garde ne bouge.
        //
        // Le contrôle mutant l'a prouvé : sans ce DISABLE, la garde passe AVEC
        // ET SANS le pont (pratique 2169). Une garde qui ne peut pas rougir ne
        // prouve rien.
        // REQ-AXO-902630 — opt-out NOMMÉ, les sept triggers d'un coup. La forme
        // recopiée (`ALTER TABLE ist.IndexedFile DISABLE TRIGGER ...`) ne
        // couvrait qu'une table : une garde qui écrit dans `Symbol` serait
        // restée aveugle pour la même raison, sans que rien ne le dise.
        crate::test_support::test_db::neutraliser_autoseed_des_parents_fk(&store)
            .expect("neutraliser l'auto-seed du harnais");

        store
            .sync_project_registry_entry(
                &code,
                Some("a3-fresh-tenant"),
                Some(root.to_string_lossy().as_ref()),
            )
            .expect("enrôler le projet");

        let ecriture = store.upsert_graph(
            fichier.to_string_lossy().as_ref(),
            &code,
            "pub fn main() {}\n",
            "hash-a3-fresh",
            1,
            &[],
            &[],
        );

        assert!(
            ecriture.is_ok(),
            "un lot A3 d'un tenant enrôlé ne doit PAS être refusé par la FK : {:?}",
            ecriture.err()
        );

        // REQ-AXO-902626 — asserter l'EFFET, pas l'absence d'erreur. Le contrôle
        // mutant a montré que `is_ok()` seul passait AVEC ET SANS le pont : le
        // chemin d'écriture ne remonte pas le refus FK à l'appelant. Une garde qui
        // ne peut pas rougir ne prouve rien (pratique 2169) — c'est la LIGNE en
        // base qui atteste que la FK a été satisfaite.
        let lignes = store
            .query_json_param(
                "SELECT COUNT(*) FROM ist.IndexedFile WHERE project_code = ?",
                &serde_json::json!([code]),
            )
            .expect("compter les IndexedFile du tenant");
        let rows: Vec<Vec<serde_json::Value>> =
            serde_json::from_str(&lignes).unwrap_or_default();
        let n = rows
            .first()
            .and_then(|r| r.first())
            .map(|c| match c {
                serde_json::Value::Number(v) => v.as_i64().unwrap_or(0),
                serde_json::Value::String(v) => v.parse::<i64>().unwrap_or(0),
                _ => 0,
            })
            .unwrap_or(0);
        assert_eq!(
            n, 1,
            "le fichier du tenant doit ATTERRIR dans ist.IndexedFile — sans parent \
             axon.Project la FK le refuse et A3 perd 100 % des lots (REQ-AXO-902626)"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// La réconciliation solde le passé : un code au registre sans parent, dont
    /// le chemin existe, reçoit sa ligne. Mutant : vider la boucle.
    #[test]
    fn la_reconciliation_enrole_un_orphelin_au_chemin_vivant() {
        let store = create_test_db().expect("create test db");
        let scope = unique_test_scope("rec");
        let code = three_char_code_from_scope(&scope);
        let root = std::env::temp_dir().join(format!("reconcile-{scope}"));
        std::fs::create_dir_all(&root).expect("create project root");

        // L'orphelin est fabriqué SANS passer par le pont — c'est exactement
        // l'état laissé par c72cd227 : une ligne de registre, aucun parent.
        store
            .execute_param(
                "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) \
                 VALUES (?, ?, ?) ON CONFLICT (project_code) DO NOTHING",
                &serde_json::json!([code, "orphelin-vivant", root.to_string_lossy()]),
            )
            .expect("fabriquer l'orphelin");
        assert_eq!(
            parent_rows(&store, &code),
            0,
            "l'orphelin doit vraiment être orphelin, sinon la garde ne mesure rien"
        );

        let identities = vec![CanonicalProjectIdentity {
            name: Some("orphelin-vivant".to_string()),
            code: code.clone(),
            project_path: root.clone(),
            meta_path: root.join(".axon").join("meta.json"),
        }];
        let enroles = store.reconcile_project_fk_parents(&identities);

        assert_eq!(enroles, 1, "la réconciliation doit compter le parent qu'elle écrit");
        assert_eq!(
            parent_rows(&store, &code),
            1,
            "un orphelin au chemin vivant doit retrouver son parent FK (REQ-AXO-902626)"
        );

        // REQ-AXO-902626 — le compteur doit rendre les TROUS COMBLÉS, pas les projets
        // traités. `ensure_project_fk_parent` étant un UPSERT, il réussit aussi sur un
        // parent déjà là : compter ses succès annonçait « 63 réparations » un jour où
        // il n'y avait rien à réparer. Rejouer sur le MÊME jeu doit donc rendre 0 —
        // c'est ce second appel qui distingue les deux comptages, le premier ne le
        // pouvait pas (1 identité, 1 succès, 1 trou : les trois coïncident).
        let rejoue = store.reconcile_project_fk_parents(&identities);
        assert_eq!(
            rejoue, 0,
            "rejouée sur un parc déjà sain, la réconciliation ne doit signaler AUCUNE \
             réparation — sinon elle compte les projets traités et son alerte crie \
             au loup à chaque démarrage (REQ-AXO-902626)"
        );
        assert_eq!(
            parent_rows(&store, &code),
            1,
            "et elle reste idempotente : toujours une seule ligne parente"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Le filtre : 13 des 20 orphelins mesurés étaient des fixtures `/tmp`
    /// disparues. Les enrôler fabriquerait des locataires fantômes.
    /// Mutant : retirer le test `is_dir()`.
    #[test]
    fn la_reconciliation_n_enrole_pas_un_chemin_disparu() {
        let store = create_test_db().expect("create test db");
        let scope = unique_test_scope("dis");
        let code = three_char_code_from_scope(&scope);
        let disparu = std::env::temp_dir().join(format!("jamais-cree-{scope}"));
        assert!(
            !disparu.exists(),
            "le chemin de la garde doit être absent du disque"
        );

        let identities = vec![CanonicalProjectIdentity {
            name: Some("fixture-disparue".to_string()),
            code: code.clone(),
            project_path: disparu,
            meta_path: std::path::PathBuf::from("/nonexistent/.axon/meta.json"),
        }];
        let enroles = store.reconcile_project_fk_parents(&identities);

        assert_eq!(enroles, 0, "aucun parent ne doit être écrit pour un chemin disparu");
        assert_eq!(
            parent_rows(&store, &code),
            0,
            "une fixture disparue ne doit JAMAIS devenir un locataire (REQ-AXO-902626)"
        );
    }

    #[test]
    fn l_opt_out_retire_la_bequille_sur_les_sept_tables() {
        // REQ-AXO-902630 — la béquille du harnais couvre SEPT tables ; l'opt-out
        // doit les couvrir toutes.
        //
        // La forme recopiée d'avant ne désactivait que `trg_test_autoseed_
        // indexedfile`. Une garde qui écrit dans `Symbol`, `Edge`, `Chunk` ou
        // une table de projection restait donc aveuglée EXACTEMENT comme la
        // garde 2 l'était — et passait avec et sans le correctif, sans que
        // rien ne le dise.
        //
        // On interroge `pg_trigger` plutôt que de tenter une insertion par
        // table : le verdict ne dépend alors d'aucun schéma de colonnes, et il
        // reste juste quand une huitième table rejoint la liste.
        let store = crate::tests::test_helpers::create_test_db().expect("db de test");

        let actifs_avant = store
            .query_count(
                "SELECT count(*) FROM pg_trigger \
                 WHERE tgname LIKE 'trg_test_autoseed%' AND NOT tgisinternal AND tgenabled <> 'D'",
            )
            .expect("lire pg_trigger");
        assert_eq!(
            actifs_avant, 7,
            "le harnais installe SEPT auto-seeds ; si ce nombre change, l'opt-out \
             doit changer avec lui — c'est le but de cette garde"
        );

        crate::test_support::test_db::neutraliser_autoseed_des_parents_fk(&store)
            .expect("neutraliser l'auto-seed");

        let actifs_apres = store
            .query_count(
                "SELECT count(*) FROM pg_trigger \
                 WHERE tgname LIKE 'trg_test_autoseed%' AND NOT tgisinternal AND tgenabled <> 'D'",
            )
            .expect("lire pg_trigger");
        assert_eq!(
            actifs_apres, 0,
            "après l'opt-out, AUCUNE table ne doit encore fabriquer ses parents FK"
        );
    }
}
