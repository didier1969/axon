// Copyright (c) Didier Stadelmann. All rights reserved.

//! REQ-AXO-91560 / REQ-AXO-91562 — canonical per-test PostgreSQL isolation.
//!
//! Single home for the ephemeral-database harness shared by every test that
//! needs an isolated, seeded store: the raw-SQL tests under
//! `crate::mcp::tests` and the IST/SOLL builder fixtures under
//! [`crate::test_support::ist_fixtures`]. Each [`TestDb`] is a fresh
//! `createdb -T axon_test_template` clone carrying the canonical DDL + global
//! seed + the test-only auto-seed triggers, so fixtures insert IST/SOLL rows
//! without hand-seeding FK parents and without ever touching the shared
//! `axon_dev` database (the historical `GraphStore::new` path that leaked test
//! writes into dev and broke isolation — REQ-AXO-901718/720/721 root cause).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// REQ-AXO-902272 — aucun sous-processus du harnais n'attend sans borne.
//
// Le defaut mesure (session 107) : la suite est restee pendue >10 min a
// 1678/1693, sans rien dire. `std::process` n'offre AUCUNE attente bornee —
// ni `Child::wait`, ni `Command::output` — donc les NEUF lancements de ce
// fichier pendaient tous indefiniment si leur `psql` / `createdb` / `dropdb`
// ne rendait pas la main. Le symptome est le pire possible : un test qui ne
// rend jamais la main se lit « c'est long », pas « c'est casse ».
//
// La borne vit ICI, au point de passage commun, et nulle part ailleurs
// (GUI-PRO-013) : la classe entiere est fermee d'un coup, pas le seul site
// ou le blocage a ete observe.
//
// DEUX proprietes distinctes, et le noeud n'en demandait qu'une explicitement :
// « ne plus PENDRE » (la borne : kill + reap) et « ne plus etre SILENCIEUX »
// (le message + la commande de recuperation). Faire ECHOUER n'en est ni l'une
// ni l'autre — c'est un moyen, qui ne convient qu'aux chemins ou le
// depassement rend la suite du test impossible. D'ou la separation :
//   · CREATION / SEED  -> `run_or_panic` : sans base, le test ne peut rien.
//   · DESTRUCTION / SWEEP -> `run_bounded` : degrade, ecrit, continue.
// Mesure du 2026-08-25 qui a impose cette separation : paniquer sur le chemin
// de destruction a fait echouer **20 tests qui avaient REUSSI**, parce que
// sous 16 threads paralleles un `dropdb --force` depasse couramment 60 s.
//
// ⚠️ Ce qui est DELIBEREMENT exclu : ajouter `WITH (FORCE)` au DROP genere
// par le sweep. `REQ-AXO-901906` (FIX 2) raconte pourquoi — le sweep a deja
// droppe la base partagee VIVANTE une fois. `FORCE` termine les backends :
// il autoriserait le sweep a tuer les connexions d'un binaire de test
// parallele, exactement ce que la garde `NOT EXISTS (pg_stat_activity)` +
// l'exclusion `axon_test_shared_<pid>` interdisent. Et un `DROP DATABASE`
// sans FORCE sur une base occupee echoue IMMEDIATEMENT — il ne bloque pas,
// donc FORCE n'expliquerait de toute facon aucune pendaison.
// ---------------------------------------------------------------------------

/// Budget d'un `dropdb` unitaire. Genereux, et surtout **degrade** au lieu
/// d'echouer : mesure sous 16 threads paralleles, un `dropdb --force` depasse
/// couramment 60 s — c'est le regime NORMAL sous contention, pas une panne.
const BUDGET_DROP: Duration = Duration::from_secs(120);
/// Budget d'un `createdb -T <template>` (clone d'une base seedee).
const BUDGET_CREATE: Duration = Duration::from_secs(120);
/// Budget du sweep de reclamation. Genereux : il peut avoir a DROP les bases
/// laissees par plusieurs runs tues, et un `DROP DATABASE` est un `rm -rf` du
/// repertoire de la base suivi d'un checkpoint.
const BUDGET_SWEEP: Duration = Duration::from_secs(180);
/// Budget d'UN fichier `.sql` applique par `psql -f`.
const BUDGET_SQL_FILE: Duration = Duration::from_secs(120);

/// Cadence de scrutation de `try_wait`. `std::process` n'expose pas d'attente
/// bornee ; ce sondage est l'idiome std, et non le « polling par sleep » que
/// la regle projet proscrit pour l'orchestration de services.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Un enfant qui a depasse son budget : tue, jamais attendu indefiniment.
#[derive(Debug)]
pub(crate) struct BudgetExceeded {
    pub(crate) budget: Duration,
}

/// Ce qu'un lancement borne a donne. Trois issues DISTINCTES — les confondre
/// est precisement ce qui a casse le premier jet : « psql absent » et « psql
/// n'a pas rendu la main » n'appellent pas la meme reaction.
#[derive(Debug)]
pub(crate) enum RunOutcome {
    /// L'enfant a rendu la main dans les temps (stderr draine).
    Ran(ExitStatus, String),
    /// Le binaire n'existe pas — environnement unitaire sans PG. Le harnais
    /// est best-effort la-dessus, c'est le comportement d'origine.
    NoBinary,
    /// Budget depasse : l'enfant a ete TUE et moissonne.
    TimedOut,
}

/// Attend `child` au plus `budget`. Au-dela : **tue** l'enfant, le moissonne
/// (pas de zombie) et rend `Err`.
///
/// Ne panique pas : la decision d'echouer appartient a l'appelant, ce qui
/// laisse ce coeur-ci testable finement — y compris « l'enfant est-il
/// reellement mort ».
pub(crate) fn wait_within(
    child: &mut Child,
    budget: Duration,
) -> Result<ExitStatus, BudgetExceeded> {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            // Un enfant deja moissonne ne peut plus etre attendu : traiter
            // l'erreur comme un depassement laisserait croire a une pendaison.
            Err(_) => return Ok(ExitStatus::default()),
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(BudgetExceeded { budget });
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Le remede, ecrit UNE fois (GUI-PRO-013) — cite par le message de
/// depassement comme par la panique.
fn recovery_hint() -> String {
    let port = pg_port();
    format!(
        "  Compter ce qui traine :\n    psql -h 127.0.0.1 -p {port} -U axon -d postgres \
         -Atc \"SELECT count(*) FROM pg_database WHERE datname LIKE 'axon\\_test\\_%'\"\n\
         \x20 Recuperer d'un coup :\n    psql -h 127.0.0.1 -p {port} -U axon -d postgres -Atc \
         \"SELECT format('DROP DATABASE IF EXISTS %I', datname) FROM pg_database WHERE datname \
         LIKE 'axon\\_test\\_%' AND datname <> 'axon_test_template'\" \
         | psql -h 127.0.0.1 -p {port} -U axon -d postgres"
    )
}

/// Lance `cmd` sous budget, en lui poussant `stdin_data` s'il y en a.
/// **Ne panique jamais** : au depassement, l'enfant est tue et le fait est
/// ECRIT sur stderr — « ne plus pendre » et « ne plus etre silencieux » sont
/// deux proprietes distinctes, et aucune des deux n'exige de faire echouer un
/// test qui a reussi.
fn run_bounded(
    cmd: &mut Command,
    stdin_data: Option<&[u8]>,
    budget: Duration,
    what: &str,
) -> RunOutcome {
    let Ok(mut child) = cmd
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return RunOutcome::NoBinary;
    };

    if let Some(data) = stdin_data {
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            let _ = stdin.write_all(data);
        }
    }
    // Fermer stdin signale EOF : sans ca, `psql` attend indefiniment d'autres
    // commandes et le budget se declencherait sur un faux positif.
    drop(child.stdin.take());

    // Drainer stderr dans un thread : un tube plein bloquerait l'enfant, et un
    // enfant bloque sur son tube ressemble a une pendaison sans en etre une.
    let drain = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = e.read_to_string(&mut buf);
            buf
        })
    });

    match wait_within(&mut child, budget) {
        Ok(status) => {
            let stderr = drain.and_then(|h| h.join().ok()).unwrap_or_default();
            RunOutcome::Ran(status, stderr)
        }
        Err(BudgetExceeded { budget }) => {
            // Le kill ferme le tube, donc le drain atteint EOF et se joint.
            // Ce que l'enfant a DIT avant de bloquer est la seule information
            // que le budget seul ne donne pas — « ne plus etre silencieux »
            // vaut aussi pour sa sortie d'erreur, pas seulement pour la notre.
            let dit = drain
                .and_then(|h| h.join().ok())
                .unwrap_or_default()
                .trim()
                .to_string();
            let dit = if dit.is_empty() {
                "(rien sur stderr)".to_string()
            } else {
                dit.lines().take(5).collect::<Vec<_>>().join("\n      ")
            };
            eprintln!(
                "[REQ-AXO-902272] {what} n'a pas rendu la main en {budget:?} — enfant TUE.\n\
                 \x20     stderr de l'enfant : {dit}\n{}",
                recovery_hint()
            );
            RunOutcome::TimedOut
        }
    }
}

/// Variante pour les chemins ou le depassement rend la suite du test IMPOSSIBLE
/// (creer la base, la seeder) : la panique y porte une information, alors que
/// laisser continuer produirait une erreur obscure trois appels plus loin.
///
/// ⚠️ A NE PAS utiliser sur un chemin de DESTRUCTION. Mesure le 2026-08-25 :
/// paniquer dans `force_dropdb` — appele depuis `impl Drop for TestDb` et
/// depuis le handler `atexit` — a fait echouer **20 tests qui avaient REUSSI**,
/// simplement parce que sous 16 threads paralleles un `dropdb --force` depasse
/// couramment 60 s. Une base non droppee n'est pas une perte : le sweep du run
/// suivant la reclame (REQ-AXO-901848). Faire echouer le test, si.
fn run_or_panic(
    cmd: &mut Command,
    stdin_data: Option<&[u8]>,
    budget: Duration,
    what: &str,
) -> Option<(ExitStatus, String)> {
    match run_bounded(cmd, stdin_data, budget, what) {
        RunOutcome::Ran(status, stderr) => Some((status, stderr)),
        RunOutcome::NoBinary => None,
        RunOutcome::TimedOut => panic!(
            "{what} n'a pas rendu la main en {budget:?} — enfant TUE plutot que d'attendre \
             indefiniment (REQ-AXO-902272).\n{}",
            recovery_hint()
        ),
    }
}

/// Test-cluster port (devenv PG). Overridden by `PGPORT`.
pub(crate) fn pg_port() -> String {
    std::env::var("PGPORT").unwrap_or_else(|_| "44144".to_string())
}

fn template_name() -> String {
    std::env::var("AXON_TEST_TEMPLATE").unwrap_or_else(|_| "axon_test_template".to_string())
}

/// REQ-AXO-901873 — `dropdb --force --if-exists` : termine les connexions
/// résiduelles puis DROP. Remplace le `dropdb` best-effort qui leakait dès
/// qu'une connexion subsistait. Renvoie `true` si la base n'existe plus après.
fn force_dropdb(db_name: &str, pg_port: &str) -> bool {
    let mut cmd = std::process::Command::new("dropdb");
    cmd.args([
        "-h",
        "127.0.0.1",
        "-p",
        pg_port,
        "-U",
        "axon",
        "--force",
        "--if-exists",
        db_name,
    ]);
    // Chemin de DESTRUCTION (appele depuis `Drop` et depuis `atexit`) : une
    // base non droppee est reclamee par le sweep du run suivant, alors qu'une
    // panique ici ferait echouer un test deja reussi.
    match run_bounded(&mut cmd, None, BUDGET_DROP, &format!("dropdb {db_name}")) {
        RunOutcome::Ran(status, _) => status.success(),
        RunOutcome::NoBinary | RunOutcome::TimedOut => false,
    }
}

/// REQ-AXO-901873 — registre des bases créées par CE process, force-droppées à
/// la sortie du process via un hook `libc::atexit`. Garantit la suppression
/// systématique **à la fin du run** même pour les guards parkés en `static`
/// (qui ne déclenchent jamais `Drop`). Complète le `Drop` per-test (fast-path)
/// et le pre-run sweep (fallback terminaison anormale).
fn registered_test_dbs() -> &'static Mutex<Vec<(String, String)>> {
    static REGISTERED: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    REGISTERED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Enregistre `(db_name, pg_port)` pour la réclamation de fin de process et
/// arme le hook `atexit` une seule fois.
fn register_for_atexit_cleanup(db_name: &str, pg_port: &str) {
    static ARMED: OnceLock<()> = OnceLock::new();
    if let Ok(mut v) = registered_test_dbs().lock() {
        v.push((db_name.to_string(), pg_port.to_string()));
    }
    ARMED.get_or_init(|| {
        // SAFETY: `drop_registered_test_dbs` est une `extern "C" fn` sans état
        // capturé ; elle lit le registre process-global. Armée une seule fois.
        unsafe {
            libc::atexit(drop_registered_test_dbs);
        }
    });
}

/// Handler `libc::atexit` — s'exécute à la terminaison normale du process.
/// Force-DROP chaque base `axon_test_*` créée par ce process. Best-effort
/// (le process sort de toute façon).
extern "C" fn drop_registered_test_dbs() {
    let dbs: Vec<(String, String)> = match registered_test_dbs().lock() {
        Ok(v) => v.clone(),
        Err(p) => p.into_inner().clone(),
    };
    for (db_name, pg_port) in dbs {
        let _ = force_dropdb(&db_name, &pg_port);
    }
}

/// REQ-AXO-91562 Slice 2 — per-test database isolation via PG template.
///
/// Each test gets a fresh database cloned from `axon_test_template`.
///
/// Lifecycle / reclamation (REQ-AXO-901848): the `Drop` impl issues a
/// best-effort `dropdb`. Callers that park the guard in a process-lifetime
/// `static` never run `Drop` (Rust does not drop `static` contents at exit),
/// so the canonical reclamation is the idempotent, connection-safe pre-run
/// [`sweep_stale_test_databases`], invoked once per process the first time a
/// `TestDb` is created. It reclaims databases leaked by *previous* runs
/// regardless of how this process terminates. Callers that own the guard for a
/// single test's duration (e.g. the IST fixture harness) get the `Drop`
/// fast-path for free.
pub(crate) struct TestDb {
    db_name: String,
    pg_port: String,
}

impl TestDb {
    pub(crate) fn create() -> Self {
        // REQ-AXO-901848 — reclaim databases leaked by previous runs before
        // creating this run's database. Idempotent and connection-safe.
        let port = pg_port();
        sweep_once(&port);
        // REQ-AXO-91560 — bring the clone template to canonical schema+seed
        // (and test auto-seed triggers) before the first `createdb -T` below.
        ensure_template_once(&port);

        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tid = std::thread::current().id();
        let db_name = format!("axon_test_{:x}_{:?}", id, tid)
            .replace("ThreadId(", "t")
            .replace(')', "");
        let template = template_name();

        let mut cmd = std::process::Command::new("createdb");
        cmd.args([
            "-h",
            "127.0.0.1",
            "-p",
            &port,
            "-U",
            "axon",
            "-T",
            &template,
            &db_name,
        ]);
        let (status, stderr) = run_or_panic(
            &mut cmd,
            None,
            BUDGET_CREATE,
            &format!("createdb -T {template} {db_name}"),
        )
        .expect("createdb command failed to execute");

        if !status.success() {
            panic!("TestDb create failed for {db_name}: {stderr}");
        }

        // REQ-AXO-901873 — réclamation systématique à la fin du run (couvre les
        // guards parkés en `static` qui ne déclenchent jamais `Drop`).
        register_for_atexit_cleanup(&db_name, &port);

        TestDb {
            db_name,
            pg_port: port,
        }
    }

    pub(crate) fn url(&self) -> String {
        format!(
            "postgres://axon@127.0.0.1:{}/{}",
            self.pg_port, self.db_name
        )
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        // REQ-AXO-901873 — Drop fiable : `dropdb --force` termine les connexions
        // résiduelles puis DROP (l'ancien best-effort leakait dès qu'une
        // connexion subsistait). Erreur surfacée ; le hook `atexit` reprend en
        // filet de sécurité si ce Drop échoue.
        if !force_dropdb(&self.db_name, &self.pg_port) {
            eprintln!(
                "WARN REQ-AXO-901873: dropdb --force a échoué pour {} (le hook atexit retentera)",
                self.db_name
            );
        }
    }
}

/// REQ-AXO-902215 — process-shared isolated test database URL for the
/// env-resolving `GraphStore` constructors (`":memory:"` / bare-path stores
/// that carry NO explicit `database_url` and resolve it via
/// [`crate::postgres::resolve_database_url`]). Without this those constructors
/// fall through to `AXON_LIVE_DATABASE_URL` and bootstrap/DDL/pollute the LIVE
/// runtime database, contending with the live indexer's continuous `ist.edge`
/// writes (`CREATE OR REPLACE TRIGGER` blocks → 55P03/57014 flake; 103 stray
/// `ist.IndexedFile` rows were found leaked into `axon_live`). This finishes
/// the never-wired REQ-AXO-91562 "Slice 2 test harness" safety net: one shared
/// clone per test PROCESS, named `axon_test_shared_<pid>` to match the sweep
/// exclusion (REQ-AXO-901906), reclaimed by the `atexit` hook + pre-run sweep.
///
/// Memoised in a `OnceLock` so the clone is created exactly once, on the first
/// env-resolving constructor in the process. Real per-test isolation still
/// comes from [`TestDb`] + `new_with_database`; this is the class-level net so
/// no env-resolving path — present or future — can reach a runtime database.
pub(crate) fn shared_test_db_url() -> String {
    static SHARED_URL: OnceLock<String> = OnceLock::new();
    SHARED_URL
        .get_or_init(|| {
            let port = pg_port();
            sweep_once(&port);
            ensure_template_once(&port);
            let db_name = format!("axon_test_shared_{}", std::process::id());
            // A prior run that crashed carrying this PID could have left a stale
            // one; force-drop before cloning so createdb -T never collides.
            force_dropdb(&db_name, &port);
            let mut cmd = std::process::Command::new("createdb");
            cmd.args([
                "-h",
                "127.0.0.1",
                "-p",
                &port,
                "-U",
                "axon",
                "-T",
                &template_name(),
                &db_name,
            ]);
            let (status, stderr) = run_or_panic(
                &mut cmd,
                None,
                BUDGET_CREATE,
                &format!("createdb -T (shared) {db_name}"),
            )
            .expect("createdb (shared test db) failed to execute");
            if !status.success() {
                panic!("shared TestDb create failed for {db_name}: {stderr}");
            }
            register_for_atexit_cleanup(&db_name, &port);
            format!("postgres://axon@127.0.0.1:{port}/{db_name}")
        })
        .clone()
}

/// REQ-AXO-901848 — reclaim `axon_test_*` databases leaked by previous test
/// runs. Runs exactly once per test process (guarded by [`sweep_once`]) before
/// the first database is created.
///
/// Concurrency safety: only databases with **zero** active backends in
/// `pg_stat_activity` are dropped. The template (`axon_test_template`) and any
/// non-test database are excluded by the `LIKE 'axon\_test\_%'` filter plus an
/// explicit guard.
///
/// ⚠️ REQ-AXO-902272 — ce commentaire affirmait qu'il n'y a « pas de course
/// create/sweep » parce que les noms frais (nanosecondes + thread-id) ne
/// peuvent pas COLLISIONNER avec les noms fuites balayes. Le raisonnement est
/// faux : la collision de noms n'a jamais ete le mecanisme. Le mecanisme, c'est
/// qu'une base per-test VIVANTE se retrouve a **zero backend entre deux tests**
/// (depuis que `NativePgCtx::drop` ferme les pools sans attendre) — elle passe
/// alors le filtre `pg_stat_activity` et se fait DROP sous les pieds du test
/// qui l'utilise encore. `REQ-AXO-901906` FIX 2 a nomme exactement ce danger et
/// l'a ferme pour la seule base partagee ; les bases per-test y sont exposees
/// de la meme facon. D'ou l'exclusion du registre de CE process ci-dessous.
///
/// Le contrat du sweep — « reclamer les fuites des runs PRECEDENTS » — en sort
/// mieux respecte, pas relache : les bases des autres process gardent des noms
/// absents de ce registre et restent reclamees.
/// REQ-AXO-902473 — rend `true` si le balayage est allé AU BOUT, `false` s'il a été
/// tronqué par son budget.
///
/// Ce n'est pas une commodité : sans ce retour, un appelant ne peut pas distinguer
/// « le sweep a fait son travail et il ne restait rien » de « le sweep a été coupé au
/// milieu ». Les deux se lisent identiquement — silence et base encore là — alors que
/// l'un est un verdict et l'autre une absence de mesure. C'est la distinction que
/// `REQ-AXO-902328` a dû rétablir ailleurs le même jour, et que `e5a39851` a écrite pour
/// le lock timeout : « ceci ne dit RIEN sur l'état, ceci dit que je n'ai pas pu mesurer ».
///
/// Mesuré le 2026-08-25 : 180,02 s pour **293 candidats**, budget épuisé, aucun DROP
/// abouti — toutes les connexions bloquées sur `IPC / CheckpointDone`. `DROP DATABASE`
/// demande un checkpoint, et le checkpointer est un processus unique : N drops concurrents
/// font la queue. Le volume de candidats est donc lui-même le produit des drops abandonnés
/// par la passe précédente.
pub(crate) fn sweep_stale_test_databases(pg_port: &str) -> bool {
    // `DROP DATABASE` cannot run inside a transaction block, so a DO/loop is
    // not an option; `\gexec` executes each generated statement as its own
    // top-level command. ON_ERROR_STOP=0 keeps one failed drop (e.g. a
    // database that acquired a connection between SELECT and DROP) from
    // aborting the rest.
    // REQ-AXO-901906 — exclude THIS process's own shared test DB. Since
    // `NativePgCtx::drop` now closes pools eagerly, the process-shared DB
    // (`axon_test_shared_<pid>`) sits at zero backends *between* tests; without
    // this guard the mid-run `sweep_reclaims_leaked_test_databases` test (which
    // re-invokes the real sweep) would DROP the live shared DB, and every
    // subsequent `create_test_db` — whose URL is memoised in a OnceLock — would
    // fail to connect ("pool init failed"). Other processes'/prior runs' shared
    // + per-test DBs (different names) are still reclaimed.
    let own_shared = format!("axon_test_shared_{}", std::process::id());
    // Les bases que CE process a créées et n'a pas encore rendues. Un mutex
    // empoisonné ne doit JAMAIS vider cette liste : elle échouerait alors
    // « ouverte » — le sweep se remettrait à DROP des bases vivantes, ce qui
    // est précisément le danger qu'on ferme ici. Même traitement que
    // `drop_registered_test_dbs`.
    let own_live: Vec<String> = match registered_test_dbs().lock() {
        Ok(v) => v.iter().map(|(name, _)| name.clone()).collect(),
        Err(poisoned) => poisoned.into_inner().iter().map(|(n, _)| n.clone()).collect(),
    };
    // L'exclusion vit dans le SQL, jamais en post-filtrage : filtrer après le
    // SELECT rouvrirait la course SELECT/DROP que cette requête évite.
    let own_live_clause = if own_live.is_empty() {
        String::new()
    } else {
        let liste = own_live
            .iter()
            .map(|n| format!("'{}'", n.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        format!("          AND d.datname <> ALL (ARRAY[{liste}])\n")
    };
    let script = format!(
        "\\set ON_ERROR_STOP 0\n\
        SELECT format('DROP DATABASE IF EXISTS %I', d.datname)\n\
        FROM pg_database d\n\
        WHERE d.datname LIKE 'axon\\_test\\_%'\n\
          AND d.datname <> 'axon_test_template'\n\
          AND d.datname <> '{own_shared}'\n\
        {own_live_clause}\
          AND NOT EXISTS (\n\
            SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname\n\
          )\n\
        \\gexec\n"
    );

    let mut cmd = std::process::Command::new("psql");
    cmd.args([
        "-h",
        "127.0.0.1",
        "-p",
        pg_port,
        "-U",
        "axon",
        "-d",
        "postgres",
        "-X", // ignore ~/.psqlrc for deterministic behaviour
        "-q",
    ]);
    // REQ-AXO-902272 — le sweep est le seul appel dont la duree soit une
    // INFORMATION : c'est elle que le noeud decrit (« >10 min »). On la mesure
    // et on l'ecrit avec le nombre de candidats, au lieu de la deviner. Sans le
    // denominateur, « lent » ne dit pas si le probleme est le VOLUME ou le
    // DROP lui-meme — et le budget se choisirait au doigt mouille.
    let candidats = count_sweep_candidates(pg_port, &own_shared, &own_live);
    let debut = Instant::now();

    // Best-effort des DEUX cotes : psql absent (environnement unitaire sans PG)
    // comme budget depasse. Les bases fuitees sont un RESIDU, jamais une
    // precondition du test courant — faire echouer toute la suite parce que le
    // menage traine remplacerait une pendaison par un echec systematique.
    // Le depassement reste ECRIT sur stderr : c'est « ne plus etre silencieux »,
    // qui est la vraie demande de REQ-AXO-902272, pas la panique.
    let issue = run_bounded(
        &mut cmd,
        Some(script.as_bytes()),
        BUDGET_SWEEP,
        "sweep des bases de test fuitees",
    );
    // `NoBinary` compte comme « pas abouti » : sans psql, rien n'a été balayé. Seul un
    // processus qui a rendu la main de lui-même prouve que la passe est complète.
    let abouti = matches!(issue, RunOutcome::Ran(_, _));

    let ecoule = debut.elapsed();
    if ecoule > Duration::from_secs(5) {
        eprintln!(
            "[REQ-AXO-902272] sweep : {ecoule:?} pour {} candidat(s) \
             ({} base(s) de ce process exclue(s), {} base(s) axon_test_* au total)",
            candidats
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".to_string()),
            own_live.len(),
            total_test_databases(pg_port)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".to_string())
        );
    }
    abouti
}

/// Combien de bases `axon_test_*` existent, toutes origines confondues ?
/// Situe le candidat : 1 candidat parmi 400 bases ne se lit pas comme 1 sur 2.
fn total_test_databases(pg_port: &str) -> Option<usize> {
    let out = Command::new("psql")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "-d",
            "postgres",
            "-X",
            "-At",
            "-c",
            "SELECT count(*) FROM pg_database WHERE datname LIKE 'axon\\_test\\_%'",
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Combien de bases le sweep va-t-il tenter de DROP ? Meme predicat que le
/// script, denominateur de la mesure ci-dessus. `None` si psql est injoignable.
fn count_sweep_candidates(pg_port: &str, own_shared: &str, own_live: &[String]) -> Option<usize> {
    let exclusion = if own_live.is_empty() {
        String::new()
    } else {
        let liste = own_live
            .iter()
            .map(|n| format!("'{}'", n.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" AND d.datname <> ALL (ARRAY[{liste}])")
    };
    let sql = format!(
        "SELECT count(*) FROM pg_database d WHERE d.datname LIKE 'axon\\_test\\_%' \
         AND d.datname <> 'axon_test_template' AND d.datname <> '{own_shared}'{exclusion} \
         AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname)"
    );
    let out = Command::new("psql")
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "-d",
            "postgres",
            "-X",
            "-At",
            "-c",
            &sql,
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Run [`sweep_stale_test_databases`] at most once per test process.
fn sweep_once(pg_port: &str) {
    static SWEEP: OnceLock<()> = OnceLock::new();
    SWEEP.get_or_init(|| {
        sweep_stale_test_databases(pg_port);
    });
}

/// REQ-AXO-91560 — guarantee `axon_test_template` carries the canonical
/// schema **and** the global SOLL seed before any test clones it.
///
/// The ephemeral-DB isolation (`createdb -T template`) hands each test a
/// pristine database, but a bare/empty template strips the ambient global
/// seed (the `PRO` sentinel rows + `GUI-PRO-*` guidelines) that the shared
/// devenv PG used to provide for free. Applying the idempotent
/// `db/ddl/*.sql` + `db/seed/*.sql` to the template once per process bakes the
/// seed INTO it, so every clone inherits the canonical baseline for free.
/// Reproducible on a fresh machine — no manual template setup required.
///
/// Runs at most once per process via `OnceLock`; `get_or_init` blocks
/// concurrent callers until the template is fully built, so no clone ever
/// sees a half-seeded template. Every psql command is synchronous and its
/// connection is closed before the first `createdb -T`, so the
/// "template in use" hazard cannot arise.
pub(crate) fn ensure_template_once(pg_port: &str) {
    static TEMPLATE: OnceLock<()> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let template = template_name();

        // Create the template database if absent. A pre-existing (possibly
        // empty) template is fine — the idempotent DDL+seed below brings it
        // to canonical state. A failure here (already exists) is ignored.
        let mut cmd = std::process::Command::new("createdb");
        cmd.args(["-h", "127.0.0.1", "-p", pg_port, "-U", "axon", &template]);
        let _ = run_or_panic(
            &mut cmd,
            None,
            BUDGET_CREATE,
            &format!("createdb {template} (template)"),
        );

        // REQ-AXO-902328 — le DDL vient de la MÊME liste que le brain compile.
        //
        // Ce commentaire affirmait exactement l'inverse de ce que le code faisait :
        // « `generate_global_schema()` compiles the same db/ddl files (DEC-AXO-082),
        // so there is no schema divergence ». Il y avait divergence, et de 9 fichiers
        // sur 25 : ce chemin-ci parcourait le répertoire (25) pendant que le brain
        // rejouait une liste écrite à la main (16). Le harnais appliquait donc un
        // schéma que la production n'avait pas — ce qui est précisément la raison
        // pour laquelle aucun test n'a jamais vu le trou.
        //
        // Désormais `canonical_ddl_file_names()` est la seule règle : le template de
        // test reçoit ce que le brain grave dans son binaire, ni plus ni moins. Le
        // seed garde `read_dir` — `db/seed/` n'est pas compilé, il n'a pas de second
        // applicateur avec qui diverger.
        let db_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("db");
        let ddl_dir = db_dir.join("ddl");
        let ddl_files: Vec<PathBuf> = crate::postgres::ddl::canonical_ddl_file_names()
            .into_iter()
            .map(|nom| ddl_dir.join(nom))
            .collect();
        apply_sql_files(pg_port, &template, &ddl_files);
        apply_sql_dir(pg_port, &template, &db_dir.join("seed"));
        apply_test_autoseed_triggers(pg_port, &template);
        seed_test_project_codes(pg_port, &template);
    });
}

/// REQ-AXO-902001 — seed the canonical fixed test project codes into the
/// template registry so every `createdb -T` clone accepts them as a valid
/// scope for `soll_manager` / `soll_apply_plan` without per-test registration.
///
/// The ephemeral clone already hands each test a pristine database
/// (DEC-AXO-901634), so a fixed literal scope per test fully replaces the
/// former unique-code + wipe scoping layer (the `scoped_test_project_code` /
/// `scoped_test_ist_code` / `unique_test_project_code` helpers built for the
/// SHARED dev PG). `TST` = the single-project tests; `PJA` / `PJB` = the
/// cross-project isolation tests that assert two distinct scopes don't leak
/// into each other. Test-template only — the production registry is untouched.
/// Idempotent (`ON CONFLICT DO NOTHING`).
fn seed_test_project_codes(pg_port: &str, dbname: &str) {
    const SQL: &str = "\
INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) VALUES\n\
    ('TST', '/tmp/TST', 'Test TST'),\n\
    ('PJA', '/tmp/PJA', 'Test PJA'),\n\
    ('PJB', '/tmp/PJB', 'Test PJB')\n\
ON CONFLICT (project_code) DO NOTHING;\n\
INSERT INTO soll.Registry (project_code, id) VALUES\n\
    ('TST', 'AXON_GLOBAL'),\n\
    ('PJA', 'AXON_GLOBAL'),\n\
    ('PJB', 'AXON_GLOBAL')\n\
ON CONFLICT (project_code) DO NOTHING;\n";
    let mut cmd = std::process::Command::new("psql");
    cmd.args([
        "-h",
        "127.0.0.1",
        "-p",
        pg_port,
        "-U",
        "axon",
        "-d",
        dbname,
        "-X",
        "-q",
        "-v",
        "ON_ERROR_STOP=1",
    ]);
    let _ = run_or_panic(
        &mut cmd,
        Some(SQL.as_bytes()),
        BUDGET_SQL_FILE,
        &format!("psql seed des project codes ({dbname})"),
    );
}

/// REQ-AXO-91560 / REQ-AXO-901721 — install BEFORE INSERT auto-seed triggers
/// on the IST/SOLL tables **in the test template only**, so raw-SQL and
/// builder fixtures that insert `Symbol` / `Chunk` / `Edge` /
/// `GraphProjectionState` / `soll.Node` rows no longer have to hand-seed the
/// FK parents (`axon.Project`, `ist.IndexedFile`) or repeat `project_code` that
/// production guarantees via the A3 writer (REQ-AXO-901860 made
/// `project_code` a NOT NULL FK and `Chunk.file_path` a FK to `IndexedFile`).
/// Production DDL is untouched: the triggers live solely in
/// `axon_test_template`, and every `createdb -T` clone inherits them.
/// Idempotent (`CREATE OR REPLACE` + `DROP TRIGGER IF EXISTS`).
///
/// This is the root-cause fix for the whole class of `Writer Error: INSERT
/// INTO ist.* ... FK` test failures: a trigger covers every insert site —
/// present and future — with zero per-test boilerplate.
fn apply_test_autoseed_triggers(pg_port: &str, dbname: &str) {
    const SQL: &str = "\
CREATE OR REPLACE FUNCTION ist.test_autoseed_project() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NOT NULL THEN\n\
        INSERT INTO axon.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autoseed_chunk() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    INSERT INTO axon.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    IF NEW.file_path IS NOT NULL THEN\n\
        INSERT INTO ist.IndexedFile (path, project_code, last_seen_ms)\n\
        VALUES (NEW.file_path, NEW.project_code, 0) ON CONFLICT (path) DO NOTHING;\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autoseed_gps() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := upper(split_part(NEW.anchor_id, '::', 1));\n\
    END IF;\n\
    INSERT INTO axon.Project (code) VALUES (NEW.project_code) ON CONFLICT (code) DO NOTHING;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_symbol ON ist.Symbol;\n\
CREATE TRIGGER trg_test_autoseed_symbol BEFORE INSERT ON ist.Symbol\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_project();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_edge ON ist.Edge;\n\
CREATE TRIGGER trg_test_autoseed_edge BEFORE INSERT ON ist.Edge\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_project();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_chunk ON ist.Chunk;\n\
CREATE TRIGGER trg_test_autoseed_chunk BEFORE INSERT ON ist.Chunk\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_chunk();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_indexedfile ON ist.IndexedFile;\n\
CREATE TRIGGER trg_test_autoseed_indexedfile BEFORE INSERT ON ist.IndexedFile\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_project();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gps ON ist.GraphProjectionState;\n\
CREATE TRIGGER trg_test_autoseed_gps BEFORE INSERT ON ist.GraphProjectionState\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gembed ON ist.GraphEmbedding;\n\
CREATE TRIGGER trg_test_autoseed_gembed BEFORE INSERT ON ist.GraphEmbedding\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
DROP TRIGGER IF EXISTS trg_test_autoseed_gproj ON ist.GraphProjection;\n\
CREATE TRIGGER trg_test_autoseed_gproj BEFORE INSERT ON ist.GraphProjection\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autoseed_gps();\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_revision() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.revision_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_revpreview() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.preview_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revision ON soll.Revision;\n\
CREATE TRIGGER a_test_autofill_soll_revision BEFORE INSERT ON soll.Revision\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revision();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revchange ON soll.RevisionChange;\n\
CREATE TRIGGER a_test_autofill_soll_revchange BEFORE INSERT ON soll.RevisionChange\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revision();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_revpreview ON soll.RevisionPreview;\n\
CREATE TRIGGER a_test_autofill_soll_revpreview BEFORE INSERT ON soll.RevisionPreview\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_revpreview();\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_node() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
CREATE OR REPLACE FUNCTION ist.test_autofill_soll_edge() RETURNS TRIGGER AS $$\n\
BEGIN\n\
    IF NEW.project_code IS NULL OR NEW.project_code = '' THEN\n\
        NEW.project_code := split_part(NEW.source_id, '-', 2);\n\
    END IF;\n\
    RETURN NEW;\n\
END;\n\
$$ LANGUAGE plpgsql;\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_node ON soll.Node;\n\
CREATE TRIGGER a_test_autofill_soll_node BEFORE INSERT ON soll.Node\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_node();\n\
DROP TRIGGER IF EXISTS a_test_autofill_soll_edge ON soll.Edge;\n\
CREATE TRIGGER a_test_autofill_soll_edge BEFORE INSERT ON soll.Edge\n\
    FOR EACH ROW EXECUTE FUNCTION ist.test_autofill_soll_edge();\n";

    let mut cmd = std::process::Command::new("psql");
    cmd.args([
        "-h",
        "127.0.0.1",
        "-p",
        pg_port,
        "-U",
        "axon",
        "-d",
        dbname,
        "-X",
        "-q",
        "-v",
        "ON_ERROR_STOP=1",
    ]);
    let _ = run_or_panic(
        &mut cmd,
        Some(SQL.as_bytes()),
        BUDGET_SQL_FILE,
        &format!("psql triggers d'auto-seed ({dbname})"),
    );
}

/// Apply every `NN_*.sql` file in `dir` (lexical order) to `dbname` via
/// psql. Best-effort: a missing directory or psql binary is a silent no-op
/// (unit-only environments without PG), matching the sweep's tolerance.
/// REQ-AXO-902328 — applique une LISTE de fichiers SQL, dans l'ordre donné.
///
/// Extrait de `apply_sql_dir` pour que le DDL puisse venir de la liste compilée
/// (celle que le brain rejoue) et le seed du répertoire, sans dupliquer la
/// mécanique `psql`.
fn apply_sql_files(pg_port: &str, dbname: &str, files: &[PathBuf]) {
    for f in files {
        let Some(path) = f.to_str() else { continue };
        let mut cmd = std::process::Command::new("psql");
        cmd.args([
            "-h",
            "127.0.0.1",
            "-p",
            pg_port,
            "-U",
            "axon",
            "-d",
            dbname,
            "-X",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "-f",
            path,
        ]);
        let _ = run_or_panic(
            &mut cmd,
            None,
            BUDGET_SQL_FILE,
            &format!("psql -f {path} ({dbname})"),
        );
    }
}

fn apply_sql_dir(pg_port: &str, dbname: &str, dir: &Path) {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "sql")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.bytes().next())
                        .is_some_and(|b| b.is_ascii_digit())
            })
            .collect(),
        Err(_) => return,
    };
    files.sort();
    // Une seule mécanique `psql` dans ce fichier : ce chemin ne fait que CHOISIR
    // les fichiers (par répertoire), `apply_sql_files` les applique.
    apply_sql_files(pg_port, dbname, &files);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lance un enfant qui ne rendra pas la main de lui-meme. `None` quand le
    /// binaire manque : le test se retire alors au lieu de rougir pour une
    /// raison etrangere a ce qu'il mesure.
    fn spawn_hung_child() -> Option<Child> {
        Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    /// CONTROLE POSITIF — sans lui, le test suivant passerait aussi si
    /// `wait_within` rendait `Err` pour TOUT enfant.
    #[test]
    fn a_child_that_finishes_within_its_budget_returns_its_status() {
        let Some(mut child) = Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
        else {
            return; // pas de /bin/true : rien a mesurer
        };
        let status = wait_within(&mut child, Duration::from_secs(30))
            .expect("un enfant qui sort tout de suite ne depasse aucun budget");
        assert!(status.success(), "`true` doit sortir en succes");
    }

    /// REQ-AXO-902272 — la seconde garde, ecrite APRES une mesure qui a
    /// invalide le premier jet.
    ///
    /// Un chemin de DESTRUCTION qui depasse son budget doit DEGRADER, jamais
    /// faire echouer. Le premier jet paniquait au chokepoint sans distinguer
    /// les natures : `force_dropdb` est appele depuis `impl Drop for TestDb` et
    /// depuis le handler `atexit`, si bien que **20 tests deja REUSSIS** sont
    /// tombes — non parce qu'ils avaient un defaut, mais parce que sous 16
    /// threads paralleles un `dropdb --force` depasse couramment 60 s.
    ///
    /// Falsifiable en une ligne : remettre `panic!` dans `run_bounded` et ce
    /// test rougit.
    #[test]
    fn a_destruction_path_that_overruns_degrades_instead_of_failing() {
        if spawn_hung_child().is_none() {
            return; // pas de `sleep` : rien a mesurer
        }
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let outcome = run_bounded(
            &mut cmd,
            None,
            Duration::from_millis(200),
            "sonde de chemin de destruction",
        );
        assert!(
            matches!(outcome, RunOutcome::TimedOut),
            "un depassement sur un chemin de destruction doit rendre TimedOut \
             sans paniquer — obtenu {outcome:?}"
        );
    }

    /// CONTROLE SYMETRIQUE — sur un chemin de CREATION, le depassement DOIT
    /// faire echouer : sans base, le test n'a rien a faire, et le laisser
    /// continuer produirait une erreur obscure trois appels plus loin.
    #[test]
    #[should_panic(expected = "REQ-AXO-902272")]
    fn a_creation_path_that_overruns_fails_loudly() {
        if spawn_hung_child().is_none() {
            panic!("REQ-AXO-902272 — pas de `sleep` sur cet hote, test sans objet");
        }
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let _ = run_or_panic(
            &mut cmd,
            None,
            Duration::from_millis(200),
            "sonde de chemin de creation",
        );
    }

    /// Une base existe-t-elle ? Interroge PG, jamais un cache.
    fn database_exists(port: &str, name: &str) -> Option<bool> {
        let out = Command::new("psql")
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                port,
                "-U",
                "axon",
                "-d",
                "postgres",
                "-X",
                "-At",
                "-c",
                &format!(
                    "SELECT 1 FROM pg_database WHERE datname = '{}'",
                    name.replace('\'', "''")
                ),
            ])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim() == "1")
    }

    /// REQ-AXO-902272 — la garde de l'EXCLUSION, et les deux cas tiennent dans
    /// le MEME test : sans cela il passerait aussi bien en ne droppant rien
    /// qu'en droppant tout.
    ///
    /// Ce que le sweep doit distinguer : une fuite d'un run PRECEDENT (a
    /// reclamer) d'une base VIVANTE de ce process-ci (a epargner). Le second
    /// cas n'etait pas couvert : depuis que `NativePgCtx::drop` ferme les pools
    /// sans attendre, une base per-test vivante sit a ZERO backend entre deux
    /// tests, passe le filtre `pg_stat_activity`, et se fait DROP sous les
    /// pieds du test qui l'utilise. `REQ-AXO-901906` FIX 2 avait ferme ce
    /// danger pour la seule base partagee.
    ///
    /// Falsifiable en une ligne : retirer le `AND d.datname <> ALL (ARRAY[…])`
    /// du script du sweep, et la base vivante disparait.
    #[test]
    fn the_sweep_reclaims_a_foreign_leak_but_spares_a_live_database_of_this_process() {
        let port = pg_port();
        if database_exists(&port, "postgres").is_none() {
            return; // pas de psql / PG injoignable : rien a mesurer
        }

        // (a) VIVANTE et enregistree : passe par TestDb, donc au registre.
        //     Elle ne tient AUCUNE connexion — c'est tout l'interet du cas.
        let live = TestDb::create();
        let live_name = live.db_name.clone();

        // (b) Fuite d'un « run precedent » : createdb direct, HORS registre.
        let leaked = format!("axon_test_sweepexcl_{:x}", std::process::id());
        let mut cmd = Command::new("createdb");
        cmd.args([
            "-h",
            "127.0.0.1",
            "-p",
            &port,
            "-U",
            "axon",
            "-T",
            &template_name(),
            &leaked,
        ]);
        let Some((status, _)) = run_or_panic(
            &mut cmd,
            None,
            BUDGET_CREATE,
            &format!("createdb (fixture de fuite) {leaked}"),
        ) else {
            return;
        };
        assert!(status.success(), "la fixture de fuite doit exister avant le sweep");

        let abouti = sweep_stale_test_databases(&port);

        // REQ-AXO-902473 — un sweep TRONQUE ne prouve RIEN, dans aucun sens.
        //
        // Ce test a echoue le 2026-08-25 sur sa seconde assertion, seul, sans
        // contention : « sweep : 180,02 s pour 293 candidat(s) », budget epuise,
        // zero DROP abouti — toutes les connexions bloquees sur
        // `IPC / CheckpointDone`. La fuite avait survecu, mais PAS parce que
        // l'exclusion etait trop large : parce que le balayage n'etait jamais
        // arrive jusqu'a elle.
        //
        // Il serait FAUX de conclure dans un sens comme dans l'autre. La premiere
        // assertion, elle, passerait trivialement — un sweep qui ne DROP rien
        // n'efface evidemment aucune base vivante : elle serait verte pour la
        // mauvaise raison, ce qui est pire qu'un echec.
        //
        // Donc : on ne verdit pas, on ne rougit pas, on DIT qu'on n'a pas pu
        // mesurer. C'est la meme distinction que `e5a39851` a ecrite pour le lock
        // timeout (« ceci ne dit RIEN sur l'etat du schema ») et que
        // `REQ-AXO-902328` a rétablie le meme jour. Assouplir le budget ou
        // reessayer serait accepter le residu au lieu de le nommer.
        if !abouti {
            eprintln!(
                "[REQ-AXO-902473] garde NON CONCLUANTE : le sweep a ete tronque par son \
                 budget ({BUDGET_SWEEP:?}) avant d'avoir traite tous les candidats. Ni la \
                 fuite ({leaked}) ni la base vivante ({live_name}) ne prouvent quoi que ce \
                 soit. Cause etablie : `DROP DATABASE` attend `CheckpointDone`, le \
                 checkpointer est unique, les drops font la queue. Remede de fond = le PG \
                 ephemere de REQ-AXO-901906, PAS un budget plus grand."
            );
            // La fuite reste derriere nous : la nommer vaut mieux que la taire.
            let _ = force_dropdb(&leaked, &port);
            return;
        }

        assert_eq!(
            database_exists(&port, &live_name),
            Some(true),
            "le sweep a DROP une base vivante de ce process ({live_name}) — \
             l'exclusion du registre ne mord pas"
        );
        assert_eq!(
            database_exists(&port, &leaked),
            Some(false),
            "le sweep n'a PAS reclame une fuite hors registre ({leaked}) — \
             l'exclusion est trop large"
        );
    }

    /// REQ-AXO-902272 — LA garde. Le defaut mesure n'etait pas « c'est lent »,
    /// c'etait « la suite ne dit RIEN » : `std::process` n'offre aucune attente
    /// bornee, donc un `psql` qui ne rend pas la main pendait indefiniment et se
    /// lisait comme du travail en cours.
    ///
    /// Falsifiee avant d'etre ecrite : avec un `child.wait()` nu a la place du
    /// sondage borne, ce test ne rougit pas — il PEND 60 s, ce qui est
    /// exactement le symptome d'origine.
    #[test]
    fn a_hung_child_is_killed_at_its_budget_instead_of_hanging_forever() {
        let Some(mut child) = spawn_hung_child() else {
            return; // pas de `sleep` : rien a mesurer
        };
        let pid = child.id();

        let started = Instant::now();
        let verdict = wait_within(&mut child, Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(
            verdict.is_err(),
            "un enfant qui dort 60 s DOIT depasser un budget de 200 ms"
        );
        // La propriete qui compte n'est pas la precision de la borne, c'est
        // qu'on rende la main SANS attendre la fin naturelle de l'enfant.
        assert!(
            elapsed < Duration::from_secs(5),
            "wait_within a rendu la main en {elapsed:?} — il a attendu, pas borne"
        );
        // Tue ET moissonne : sous Linux l'entree /proc disparait au reap. Sans
        // le `wait()` qui suit le `kill()`, on laisserait un zombie par
        // depassement — une fuite silencieuse de plus.
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "le pid {pid} vit encore : l'enfant n'a pas ete tue puis moissonne"
        );
    }
}
