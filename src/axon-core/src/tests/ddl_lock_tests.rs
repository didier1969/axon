// REQ-AXO-902339 — le DDL canonique ne doit prendre AUCUN verrou bloquant
// quand il n'a rien à faire.
//
// Pourquoi ce fichier existe
// --------------------------
// PostgreSQL prend le verrou de table AVANT d'évaluer `IF NOT EXISTS`.
// `ADD COLUMN IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, `DROP TRIGGER IF
// EXISTS`, `CREATE OR REPLACE TRIGGER` sont donc idempotents dans leur EFFET
// mais pas dans leur COÛT DE VERROUILLAGE. Le step 5b du promote rejoue le DDL
// juste après le cutover, c'est-à-dire contre un indexeur qui vient de
// redémarrer et écrit en continu. Ce n'est pas une course qu'on peut reperdre
// puis regagner : c'est une famine. Le promote du 2026-08-15 a échoué DEUX fois
// (11:33 puis 20:33) sur un schéma pourtant déjà correct — annoncé FAILED alors
// que binaires, manifeste et schéma étaient justes.
//
// Ce que ce fichier garantit, et comment il est falsifiable
// --------------------------------------------------------
// Le test principal rejoue le DDL canonique ENTIER pendant qu'une seconde
// session tient les verrous d'écrivain sur les tables chaudes, avec un
// `lock_timeout` court. Zéro échec exigé.
//
// Un tel test passerait tout aussi bien si le harnais ne mesurait rien — si le
// verrou n'était pas réellement tenu, si `lock_timeout` n'était pas appliqué,
// si `55P03` n'était jamais levable. C'est pourquoi le CONTRÔLE NÉGATIF est
// obligatoire ici : il rejoue la forme BRUTE (`ALTER TABLE ... ADD COLUMN IF
// NOT EXISTS` sur une colonne qui existe déjà) dans le MÊME harnais et EXIGE
// qu'elle échoue en `55P03`. Si le contrôle négatif cesse d'échouer, le test
// positif ne prouve plus rien et la suite rougit — ce qui est le comportement
// voulu. (Leçon session 113 : une garde dont l'entrée n'est pas substituable ne
// peut pas être falsifiée.)

#[cfg(test)]
mod tests {
    use crate::test_support::test_db::TestDb;
    use tokio_postgres::error::SqlState;
    use tokio_postgres::{Client, NoTls};

    /// Tables que l'indexeur écrit en continu (pipeline A puis B). Ce sont
    /// celles dont le ROW EXCLUSIVE bloque tout ACCESS EXCLUSIVE / SHARE.
    const HOT_TABLES: &[&str] = &[
        "ist.Chunk",
        "ist.Symbol",
        "ist.IndexedFile",
        "ist.ChunkEmbedding",
        "ist.Edge",
    ];

    /// Court, mais très au-dessus du temps d'acquisition d'un verrou libre :
    /// un échec signifie « quelqu'un le tient », jamais « la machine a ramé ».
    const LOCK_TIMEOUT: &str = "SET lock_timeout = '250ms'";

    async fn connect(url: &str) -> Client {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .expect("connexion au test db");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    }

    /// Ouvre une session qui tient un ROW EXCLUSIVE sur chaque table chaude —
    /// exactement le niveau de verrou d'un écrivain — et ne le rend qu'à la
    /// fermeture du client. `LOCK TABLE` est déterministe : aucun sleep, aucune
    /// donnée à insérer, aucune course dans le harnais lui-même.
    async fn hold_writer_locks(url: &str) -> Client {
        let writer = connect(url).await;
        writer
            .batch_execute("BEGIN")
            .await
            .expect("BEGIN côté écrivain");
        for table in HOT_TABLES {
            writer
                .batch_execute(&format!("LOCK TABLE {table} IN ROW EXCLUSIVE MODE"))
                .await
                .unwrap_or_else(|e| panic!("LOCK TABLE {table} a échoué: {e}"));
        }
        writer
    }

    fn is_lock_timeout(err: &tokio_postgres::Error) -> bool {
        err.code() == Some(&SqlState::LOCK_NOT_AVAILABLE)
    }

    // ── Le test principal ────────────────────────────────────────────────

    #[tokio::test]
    async fn canonical_ddl_replay_takes_no_blocking_lock_on_hot_tables() {
        let db = TestDb::create();
        let url = db.url();

        // Le template a déjà appliqué le DDL : ce rejeu est intégralement un
        // no-op, donc il ne doit RIEN verrouiller.
        let _writer = hold_writer_locks(&url).await;
        let applier = connect(&url).await;
        applier
            .batch_execute(LOCK_TIMEOUT)
            .await
            .expect("SET lock_timeout");

        let mut starved: Vec<String> = Vec::new();
        for (idx, statement) in crate::postgres::ddl::generate_global_schema()
            .iter()
            .enumerate()
        {
            if let Err(err) = applier.batch_execute(statement).await {
                if is_lock_timeout(&err) {
                    let head: String = statement.chars().take(140).collect();
                    starved.push(format!("[{idx}] {}", head.replace('\n', " ")));
                } else {
                    panic!(
                        "énoncé DDL {idx} a échoué pour une raison NON liée au verrou \
                         (défaut réel, pas une contention) : {err}\n--- énoncé ---\n{statement}"
                    );
                }
            }
        }

        assert!(
            starved.is_empty(),
            "{} énoncé(s) du DDL canonique réclament un verrou bloquant alors qu'ils \
             n'ont rien à faire — c'est ce qui fait échouer le step 5b du promote \
             (REQ-AXO-902339). Passer par public.add_column_if_absent / \
             set_column_default_if_absent / create_index_if_absent / \
             create_trigger_if_absent :\n{}",
            starved.len(),
            starved.join("\n")
        );
    }

    // ── Le contrôle négatif : sans lui, le test ci-dessus ne prouve rien ──

    #[tokio::test]
    async fn raw_if_not_exists_forms_do_starve_in_the_same_harness() {
        let db = TestDb::create();
        let url = db.url();

        let _writer = hold_writer_locks(&url).await;
        let applier = connect(&url).await;
        applier
            .batch_execute(LOCK_TIMEOUT)
            .await
            .expect("SET lock_timeout");

        // Chaque forme BRUTE ci-dessous est un no-op complet : la colonne,
        // l'index et le trigger existent déjà dans le template. Si l'une d'elles
        // réussissait, cela voudrait dire que le harnais ne tient pas les verrous
        // — et que le test positif est vacuous.
        let raw_noops = [
            (
                "ADD COLUMN IF NOT EXISTS",
                "ALTER TABLE ist.Chunk ADD COLUMN IF NOT EXISTS \
                 embed_attempts INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "CREATE INDEX IF NOT EXISTS",
                "CREATE INDEX IF NOT EXISTS idx_chunk_token_count ON ist.Chunk (token_count)",
            ),
            // NB : `DROP TRIGGER IF EXISTS` sur un trigger ABSENT ne verrouille
            // PAS — mesuré, contre l'intuition. Il résout le trigger avant la
            // table. Ce qui verrouille, c'est le couple réel du DDL : le DROP
            // quand le trigger EXISTE (donc à chaque boot d'une base
            // bootstrappée) et le CREATE qui suit. La forme brute testée ici est
            // donc `CREATE OR REPLACE TRIGGER` à l'identique — un no-op parfait
            // qui réclame quand même ACCESS EXCLUSIVE.
            (
                "CREATE OR REPLACE TRIGGER",
                "CREATE OR REPLACE TRIGGER trg_chunk_notify_pending \
                 AFTER INSERT OR UPDATE OF content_hash ON ist.Chunk \
                 FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_chunk_pending()",
            ),
        ];

        for (label, statement) in raw_noops {
            match applier.batch_execute(statement).await {
                Err(err) if is_lock_timeout(&err) => {}
                Err(err) => panic!("`{label}` a échoué autrement qu'en 55P03 : {err}"),
                Ok(()) => panic!(
                    "CONTRÔLE NÉGATIF CASSÉ : `{label}` a RÉUSSI derrière un écrivain. \
                     Soit PostgreSQL ne prend plus le verrou avant le test d'existence, \
                     soit ce harnais ne tient plus les verrous — dans les deux cas \
                     `canonical_ddl_replay_takes_no_blocking_lock_on_hot_tables` ne \
                     mesure plus rien et doit être réparé avant d'être cru."
                ),
            }
        }

        // Et la forme GARDÉE, dans le MÊME harnais, passe : c'est la
        // substitution qui rend la garde falsifiable.
        let guarded: bool = applier
            .query_one(
                "SELECT public.add_column_if_absent('ist','chunk','embed_attempts',\
                 'INTEGER NOT NULL DEFAULT 0')",
                &[],
            )
            .await
            .expect("la forme gardée doit passer derrière un écrivain")
            .get(0);
        assert!(
            !guarded,
            "add_column_if_absent doit rendre false quand la colonne est déjà là"
        );
    }

    // ── Les gardes font bien le travail quand il y a du travail ──────────

    #[tokio::test]
    async fn guards_apply_the_ddl_when_the_object_is_genuinely_absent() {
        let db = TestDb::create();
        let client = connect(&db.url()).await;

        client
            .batch_execute(
                "CREATE SCHEMA IF NOT EXISTS ddlguard;
                 CREATE TABLE ddlguard.probe (id TEXT PRIMARY KEY)",
            )
            .await
            .expect("table de sonde");

        let added: bool = client
            .query_one(
                "SELECT public.add_column_if_absent('ddlguard','probe','n','INTEGER NOT NULL DEFAULT 7')",
                &[],
            )
            .await
            .expect("add_column_if_absent")
            .get(0);
        assert!(added, "la colonne était absente : elle doit être ajoutée");

        let again: bool = client
            .query_one(
                "SELECT public.add_column_if_absent('ddlguard','probe','n','INTEGER NOT NULL DEFAULT 7')",
                &[],
            )
            .await
            .expect("add_column_if_absent (2e passe)")
            .get(0);
        assert!(!again, "2e passe : rien à faire");

        // La colonne existe RÉELLEMENT — sinon la garde aurait « réussi » en ne
        // faisant rien, ce qui est précisément le mode d'échec à exclure.
        client
            .batch_execute("INSERT INTO ddlguard.probe (id) VALUES ('x')")
            .await
            .expect("insert");
        let n: i32 = client
            .query_one("SELECT n FROM ddlguard.probe WHERE id = 'x'", &[])
            .await
            .expect("relecture")
            .get(0);
        assert_eq!(n, 7, "le DEFAULT de la colonne ajoutée doit s'appliquer");

        // set_column_default_if_absent
        client
            .batch_execute("ALTER TABLE ddlguard.probe ADD COLUMN m INTEGER")
            .await
            .expect("colonne sans défaut");
        let set: bool = client
            .query_one(
                "SELECT public.set_column_default_if_absent('ddlguard','probe','m','42')",
                &[],
            )
            .await
            .expect("set_column_default_if_absent")
            .get(0);
        assert!(set, "aucun défaut posé : la garde doit le poser");
        let set_again: bool = client
            .query_one(
                "SELECT public.set_column_default_if_absent('ddlguard','probe','m','42')",
                &[],
            )
            .await
            .expect("set_column_default_if_absent (2e passe)")
            .get(0);
        assert!(!set_again, "2e passe : le défaut est déjà là");

        // Colonne inexistante = faute de DDL, pas un no-op silencieux.
        let missing = client
            .query_one(
                "SELECT public.set_column_default_if_absent('ddlguard','probe','pas_la','1')",
                &[],
            )
            .await;
        assert!(
            missing.is_err(),
            "poser un défaut sur une colonne absente doit échouer fort, pas passer"
        );

        // create_index_if_absent
        let idx: bool = client
            .query_one(
                "SELECT public.create_index_if_absent('ddlguard','probe_n_idx',
                     'CREATE INDEX probe_n_idx ON ddlguard.probe (n)')",
                &[],
            )
            .await
            .expect("create_index_if_absent")
            .get(0);
        assert!(idx, "index absent : il doit être créé");
        let idx_again: bool = client
            .query_one(
                "SELECT public.create_index_if_absent('ddlguard','probe_n_idx',
                     'CREATE INDEX probe_n_idx ON ddlguard.probe (n)')",
                &[],
            )
            .await
            .expect("create_index_if_absent (2e passe)")
            .get(0);
        assert!(!idx_again, "2e passe : rien à faire");

        // create_trigger_if_absent
        let trg: bool = client
            .query_one(
                "SELECT public.create_trigger_if_absent('ddlguard','probe','probe_trg',
                     'CREATE TRIGGER probe_trg AFTER INSERT ON ddlguard.probe
                        FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_chunk_pending()')",
                &[],
            )
            .await
            .expect("create_trigger_if_absent")
            .get(0);
        assert!(trg, "trigger absent : il doit être créé");
        let trg_again: bool = client
            .query_one(
                "SELECT public.create_trigger_if_absent('ddlguard','probe','probe_trg',
                     'CREATE TRIGGER probe_trg AFTER INSERT ON ddlguard.probe
                        FOR EACH ROW EXECUTE FUNCTION ist.fn_notify_chunk_pending()')",
                &[],
            )
            .await
            .expect("create_trigger_if_absent (2e passe)")
            .get(0);
        assert!(!trg_again, "2e passe : rien à faire");
    }

    // ── Le nom passé à la garde doit désigner l'objet que l'énoncé crée ──

    /// Extrait les appels `public.<fn>('<a>', '<b>'` du DDL canonique.
    /// Retourne les couples (a, b) — respectivement (schéma, index) et
    /// (schéma, table) selon la fonction.
    fn first_two_string_args(statements: &[String], func: &str) -> Vec<(String, String)> {
        // `public.` en préfixe : sans lui, le needle matche aussi la DÉFINITION
        // de la fonction (dans 00_extensions.sql) et l'extraction ramène les
        // littéraux de son propre corps — `('i','I')`. Vu en vrai au premier run.
        let needle = format!("public.{func}(");
        let mut found = Vec::new();
        for statement in statements {
            // La définition de la garde n'est pas un site d'appel.
            if statement.contains("CREATE OR REPLACE FUNCTION public.") {
                continue;
            }
            let mut rest = statement.as_str();
            while let Some(pos) = rest.find(&needle) {
                rest = &rest[pos + needle.len()..];
                let mut args = Vec::new();
                let mut cursor = rest;
                for _ in 0..2 {
                    let Some(open) = cursor.find('\'') else { break };
                    let after = &cursor[open + 1..];
                    let Some(close) = after.find('\'') else { break };
                    args.push(after[..close].to_string());
                    cursor = &after[close + 1..];
                }
                if args.len() == 2 {
                    found.push((args[0].clone(), args[1].clone()));
                }
            }
        }
        found
    }

    #[tokio::test]
    async fn every_guarded_index_and_trigger_really_exists_after_bootstrap() {
        let statements = crate::postgres::ddl::generate_global_schema();

        let indexes = first_two_string_args(&statements, "create_index_if_absent");
        assert!(
            indexes.len() >= 20,
            "extraction des index gardés cassée : {} trouvés, ≥20 attendus — \
             le test se croirait vert en ne vérifiant rien",
            indexes.len()
        );

        let triggers = first_two_string_args(&statements, "create_trigger_if_absent");
        assert!(
            triggers.len() >= 4,
            "extraction des triggers gardés cassée : {} trouvés, ≥4 attendus",
            triggers.len()
        );

        let db = TestDb::create();
        let client = connect(&db.url()).await;

        // Le NOM passé en garde et le nom réellement créé par l'énoncé sont deux
        // chaînes distinctes : rien dans le langage ne les tient ensemble. Une
        // désynchronisation ferait croire l'objet absent à chaque boot, et le
        // CREATE échouerait en « existe déjà » — ici on la nomme directement.
        for (schema, index) in &indexes {
            let exists: i64 = client
                .query_one(
                    "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                      WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind IN ('i','I')",
                    &[schema, index],
                )
                .await
                .expect("lecture pg_class")
                .get(0);
            assert_eq!(
                exists, 1,
                "l'index `{schema}.{index}` est gardé par create_index_if_absent mais \
                 n'existe pas après bootstrap — le nom de garde ne désigne pas l'objet créé"
            );
        }

        for (schema, table) in &triggers {
            let exists: i64 = client
                .query_one(
                    "SELECT count(*) FROM pg_trigger t
                       JOIN pg_class c ON c.oid = t.tgrelid
                       JOIN pg_namespace n ON n.oid = c.relnamespace
                      WHERE n.nspname = $1 AND c.relname = $2 AND NOT t.tgisinternal",
                    &[schema, table],
                )
                .await
                .expect("lecture pg_trigger")
                .get(0);
            assert!(
                exists >= 1,
                "aucun trigger sur `{schema}.{table}` après bootstrap, alors que le DDL \
                 en garde un — le couple (schéma, table) de la garde est faux"
            );
        }
    }
}
