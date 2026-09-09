//! REQ-AXO-902111 / DEC-AXO-901662 — declarative control-plane reconciler.
//!
//! T1 read-only slice: collect release-lifecycle facts → evaluate gates (typed
//! Rust predicates; Ascent/Datalog migration is T2) → derive `phase` +
//! `next_action`. The bash promote scripts still ACT; this surfaces the truth they
//! act on so an LLM (or operator) reads `{phase, failed_gates, next_action}`
//! instead of grepping 700 lines of shell. The two failures of session 91 — a
//! manifest/runtime drift after a killed promote, and a stranded `pending.json` —
//! both become a one-line derived verdict here.
//!
//! Scope of T1: the *release* state machine (manifest ↔ running build_id ↔ pending
//! staging). Runtime liveness gates (brain/indexer health) join in a later slice
//! once the in-process health source is wired (the `status` tool already owns it).

use std::path::Path;

use serde_json::Value;

/// Facts about the live release, collected from the on-disk manifests + the
/// running process's own build identity. All reads are cheap and side-effect-free.
#[derive(Debug, Clone, Default)]
pub struct ReleaseFacts {
    /// `AXON_BUILD_ID` of the process serving this call (the running brain).
    pub live_build_id: String,
    /// `runtime_version.build_id` recorded in `current.json` (the promoted truth).
    pub manifest_build_id: Option<String>,
    /// `state` field of `current.json` (e.g. "promoted").
    pub manifest_state: Option<String>,
    /// REQ-AXO-902585 — `promotion_gates.core_qualification.status` du manifeste qui
    /// POSSÈDE la question : `pending.json` quand un staging existe (c'est lui qui
    /// porte la qualification EN VOL), sinon `current.json`.
    ///
    /// L'ancien champ lisait `qualification.verdict` — une clé qu'AUCUN écrivain du
    /// dépôt ne produit : sur 244 manifestes d'historique, 0 en portent une, et 244
    /// portent `"qualification": {"evidence": []}` (provenance de build, écrite par
    /// `create_manifest.py`). Deux mécanismes portaient par accident le même mot, et
    /// la porte lisait celui qui n'est jamais alimenté — d'où un `pass: true`
    /// structurellement impossible à démentir.
    ///
    /// Le vrai verdict est écrit par `axonctl cutover --phase record-gate --gate
    /// core_qualification`, et `cutover_finalize_prepared_files` REFUSE le promote
    /// si les quatre gates requis ne sont pas `passed`.
    pub core_qualification_status: Option<String>,
    /// Preuve attachée au verdict (`release_attempt_id`, `exit_code`, log).
    pub core_qualification_evidence: Option<String>,
    /// Provenance, dite plutôt que devinée — même motif qu'`indexer_source` :
    /// "pending.promotion_gates" | "current.promotion_gates" | "absent".
    pub qualification_source: &'static str,
    /// A `pending.json` exists — a promote is mid-flight OR was stranded by a crash.
    pub pending_present: bool,
    /// `runtime_version.build_id` of `pending.json` when present.
    pub pending_build_id: Option<String>,
    /// `runtime_contract` recorded in `current.json` (e.g. "brain_mcp_indexer_ist").
    /// The presence of "indexer" in it = the live topology runs a SEPARATE indexer
    /// process that must be alive (REQ-AXO-902111 liveness slice). This is the only
    /// declarative source for "is an indexer expected" — the answering brain's own
    /// runtime mode is `brain_only` and would lie.
    pub runtime_contract: Option<String>,
    /// Correlation id written by the promotion transaction into current.json.
    pub release_attempt_id: Option<String>,
    /// Correlation id of a staged candidate, when present.
    pub pending_release_attempt_id: Option<String>,
    /// Primary artifact digest recorded by the promoted manifest.
    pub artifact_sha256: Option<String>,
    /// Last durable attempt projection (`attempt-current.json`).
    pub attempt: Option<Value>,
    /// REQ-AXO-902585 (défaut 2) — la DERNIÈRE tentative de promotion enregistrée.
    ///
    /// `attempt-current.json` est monotone : chaque tentative le réécrit en acquérant
    /// le bail. C'est donc toujours la plus récente, ce qui autorise la lecture
    /// « une promotion a échoué DEPUIS ». Sans ces champs, `phase: "clean"` (vrai au
    /// contrat : manifeste == runtime) produisait `next_action: null`, et un agent
    /// concluait « rien à faire » alors que l'outil portait lui-même la preuve que le
    /// changement qu'on voulait livrer n'est PAS en vigueur. Il fallait ouvrir le
    /// `.jsonl` à la main pour l'apprendre.
    pub attempt_id: Option<String>,
    pub attempt_status: Option<String>,
    pub attempt_phase: Option<String>,
    pub attempt_last_event_detail: Option<String>,
    pub attempt_journal_path: Option<String>,
    /// REQ-AXO-902628 — la preuve de complétion LUE DANS LE JOURNAL, jamais
    /// déduite du statut.
    ///
    /// Sans ce champ, la porte ne pouvait juger que sur la MARQUE que l'écrivain
    /// corrigé inscrit dans `last_event_detail` — donc sur rien du tout pour les
    /// 84 journaux déjà archivés, écrits avant lui. Un promote réellement complet
    /// y ressortait `unknown` : le faux négatif symétrique du faux positif que ce
    /// REQ ferme, et une porte qui se trompe dans les deux sens n'informe plus.
    ///
    /// La lecture vit ICI, dans la collecte de faits, pour que `evaluate_attempt_gate`
    /// reste un prédicat PUR — testable sans écrire un fichier.
    pub attempt_completion_evidence: Option<String>,
    /// REQ-AXO-902591 — scopes systemd `axon-live-cutover-*` vivants dans `nexus-core.slice`.
    pub live_cutover_scopes_count: usize,
    pub orphaned_cutover_scopes: Vec<String>,
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn extract_build_id(v: &Value) -> Option<String> {
    v.get("runtime_version")
        .and_then(|rv| rv.get("build_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

impl ReleaseFacts {
    /// Collect facts from `release_dir` (`.axon/live-release`) + the process build
    /// identity. `live_build_id` is read from `AXON_BUILD_ID` by the caller so this
    /// stays pure/testable.
    pub fn collect(release_dir: &Path, live_build_id: String) -> Self {
        let current = read_json(&release_dir.join("current.json"));
        let pending = read_json(&release_dir.join("pending.json"));
        let manifest_build_id = current.as_ref().and_then(extract_build_id);
        let manifest_state = current
            .as_ref()
            .and_then(|c| c.get("state"))
            .and_then(Value::as_str)
            .map(str::to_string);
        // REQ-AXO-902585 — `pending` D'ABORD : `current.promotion_gates` est
        // tautologiquement tout-vert (le `finalize` refuse de basculer sinon), donc
        // le cas intéressant est toujours l'autre. `gate_with_attestation` termine
        // par `|| true` : un promote tué juste après un gate rouge laisse un
        // `pending.json` qui PORTE ce rouge. Ne lire que `current` le masquerait.
        let lire_core_qualification = |m: &Value| -> Option<(String, Option<String>)> {
            let gate = m.get("promotion_gates")?.get("core_qualification")?;
            let status = gate.get("status")?.as_str()?.to_string();
            let evidence = gate
                .get("evidence")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some((status, evidence))
        };
        let (core_qualification_status, core_qualification_evidence, qualification_source) =
            match pending.as_ref().and_then(&lire_core_qualification) {
                Some((s, e)) => (Some(s), e, "pending.promotion_gates"),
                None => match current.as_ref().and_then(&lire_core_qualification) {
                    Some((s, e)) => (Some(s), e, "current.promotion_gates"),
                    None => (None, None, "absent"),
                },
            };
        let runtime_contract = current
            .as_ref()
            .and_then(|c| c.get("runtime_contract"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let release_attempt_id = current
            .as_ref()
            .and_then(|c| c.get("release_attempt_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let pending_release_attempt_id = pending
            .as_ref()
            .and_then(|c| c.get("release_attempt_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let artifact_sha256 = current
            .as_ref()
            .and_then(|c| c.get("artifact"))
            .and_then(|a| a.get("sha256"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let attempt = read_json(&release_dir.join("attempt-current.json"));
        let champ_attempt = |cle: &str| -> Option<String> {
            attempt
                .as_ref()?
                .get(cle)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let attempt_id = champ_attempt("release_attempt_id");
        let attempt_status = champ_attempt("status");
        let attempt_phase = champ_attempt("phase");
        let attempt_last_event_detail = champ_attempt("last_event_detail");
        let attempt_journal_path = champ_attempt("journal_path");
        // REQ-AXO-902628 — juger le journal, pas la projection qui le résume.
        // Le chemin est déjà là ; il n'était simplement jamais ouvert.
        let attempt_completion_evidence = attempt_journal_path.as_deref().map(|chemin| {
            match std::fs::read_to_string(chemin) {
                Ok(contenu) => preuve_de_completion(&contenu),
                Err(erreur) => format!("{PREFIXE_JOURNAL_ILLISIBLE} ({chemin}) — {erreur}"),
            }
        });
        let live_scopes = probe_live_cutover_scopes();
        let current_pid = std::process::id();
        let is_alive = |pid: u32| std::path::Path::new(&format!("/proc/{pid}")).exists();
        let orphaned_cutover_scopes: Vec<String> =
            filter_orphaned_scopes(&live_scopes, Some(current_pid), is_alive)
                .into_iter()
                .map(|s| s.name)
                .collect();
        let live_cutover_scopes_count = live_scopes.len();
        ReleaseFacts {
            live_build_id,
            manifest_build_id,
            manifest_state,
            core_qualification_status,
            core_qualification_evidence,
            qualification_source,
            pending_present: pending.is_some(),
            pending_build_id: pending.as_ref().and_then(extract_build_id),
            runtime_contract,
            release_attempt_id,
            pending_release_attempt_id,
            artifact_sha256,
            attempt,
            attempt_id,
            attempt_status,
            attempt_phase,
            attempt_last_event_detail,
            attempt_journal_path,
            attempt_completion_evidence,
            live_cutover_scopes_count,
            orphaned_cutover_scopes,
        }
    }

    /// The live topology runs a separate indexer process that must be alive.
    /// Derived from `runtime_contract` (never from the answering process's own mode,
    /// which is `brain_only` in the split deployment and would lie).
    pub fn indexer_expected(&self) -> bool {
        self.runtime_contract
            .as_deref()
            .is_some_and(|c| c.contains("indexer"))
    }

    /// REQ-AXO-902585 — `skipped` et l'absence sont des `Unknown`, jamais des `Pass`.
    /// « Sauté » n'est pas « passé », et un manifeste antérieur au two-phase ne
    /// PORTE simplement pas l'information : la porte ne peut pas mesurer, elle le dit.
    pub fn qualification_status(&self) -> GateStatus {
        match self.core_qualification_status.as_deref() {
            Some("passed") => GateStatus::Pass,
            Some("failed") | Some("timeout") | Some("error") => GateStatus::Fail,
            Some(_) | None => GateStatus::Unknown,
        }
    }
}

/// Runtime liveness facts — populated by the tool wrapper (`tools_release.rs`, which
/// holds `&self`/IO) from the SAME in-process sources the `status` tool trusts:
/// `resolve_indexer_liveness(latest_lifecycle_heartbeat("indexer"))` for the indexer
/// and a `SELECT 1` DB probe for the brain. Kept separate from `ReleaseFacts` so the
/// gates stay pure, IO-free predicates (testable without a runtime).
#[derive(Debug, Clone, Default)]
pub struct LivenessFacts {
    /// Brain answered a `SELECT 1` DB probe (process up AND DB reachable).
    pub brain_serving: bool,
    /// The live `runtime_contract` names a separate indexer (must be alive).
    pub indexer_expected: bool,
    /// Indexer heartbeat is fresh (`resolve_indexer_liveness(..).ready`).
    pub indexer_ready: bool,
    /// Lifecycle verdict: "healthy" | "crashed_or_abandoned" | "never_launched".
    pub indexer_lifecycle: String,
    /// Liveness source: "pg_heartbeat" | "pg_heartbeat_stale" | "no_heartbeat".
    pub indexer_source: String,
    /// REQ-AXO-902616 — qui tient le verrou d'écriture IST. `Default` = non mesuré :
    /// `CutoverFacts::new_healthy()` et tout appelant qui ne sonde pas gardent le
    /// verdict d'avant, au bit près.
    pub ist_ownership: IstOwnershipFacts,
    /// REQ-AXO-902616 — le pid que le SUPERVISEUR suit, pour le recoupement.
    pub supervised_pid: Option<i64>,
    /// REQ-AXO-902594 — profondeur de la file d'acceptation du port MCP/HTTP (:44129). None = non sondé.
    pub accept_queue_depth: Option<usize>,
}

/// REQ-AXO-902594 — Seuil au-delà duquel la file d'acceptation est considérée saturée/anormale.
pub const ACCEPT_QUEUE_MAX_HEALTHY_DEPTH: usize = 5;

/// REQ-AXO-902594 — Extrait la profondeur de la file d'acceptation (Recv-Q)
/// depuis le contenu de `/proc/net/tcp` ou `/proc/net/tcp6` pour un port donné.
/// En état `0A` (TCP_LISTEN), la colonne rx_queue porte les connexions non acceptées.
pub fn parse_listen_queue_depth(proc_net_tcp: &str, port: u16) -> Option<usize> {
    let hex_port = format!("{:04X}", port);
    for line in proc_net_tcp.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // format: sl local_address rem_address st tx_queue:rx_queue ...
        // ex: 18: 00000000:AC61 00000000:0000 0A 00000000:00000000
        if parts.len() >= 5 {
            let local_addr = parts[1];
            let state = parts[3];
            if state == "0A" && local_addr.ends_with(&format!(":{hex_port}")) {
                let queues = parts[4];
                if let Some((_tx, rx)) = queues.split_once(':') {
                    if let Ok(depth) = usize::from_str_radix(rx, 16) {
                        return Some(depth);
                    }
                }
            }
        }
    }
    None
}

/// REQ-AXO-902594 — Sonde la profondeur de la file d'acceptation du socket d'écoute
/// sur le port indiqué. Tente `/proc/net/tcp`, puis `/proc/net/tcp6`, puis fallback `ss -ltn`.
pub fn probe_listen_queue_depth(port: u16) -> Option<usize> {
    if let Ok(content) = std::fs::read_to_string("/proc/net/tcp") {
        if let Some(depth) = parse_listen_queue_depth(&content, port) {
            return Some(depth);
        }
    }
    if let Ok(content) = std::fs::read_to_string("/proc/net/tcp6") {
        if let Some(depth) = parse_listen_queue_depth(&content, port) {
            return Some(depth);
        }
    }
    if let Ok(output) = std::process::Command::new("ss").args(["-ltn"]).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let port_str = format!(":{port}");
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 && parts[0] == "LISTEN" && parts[3].ends_with(&port_str) {
                    if let Ok(depth) = parts[1].parse::<usize>() {
                        return Some(depth);
                    }
                }
            }
        }
    }
    None
}

/// REQ-AXO-902594 — Gate vérifiant que la file d'acceptation du brain n'est pas saturée.
pub fn brain_accept_queue_gate(l: &LivenessFacts) -> Gate {
    match l.accept_queue_depth {
        Some(depth) if depth > ACCEPT_QUEUE_MAX_HEALTHY_DEPTH => Gate::binary(
            "brain_accept_queue",
            false,
            format!(
                "brain MCP accept queue saturated ({depth} > {ACCEPT_QUEUE_MAX_HEALTHY_DEPTH})"
            ),
        ),
        Some(depth) => Gate::binary(
            "brain_accept_queue",
            true,
            format!(
                "brain MCP accept queue healthy ({depth} <= {ACCEPT_QUEUE_MAX_HEALTHY_DEPTH})"
            ),
        ),
        None => Gate::binary(
            "brain_accept_queue",
            true,
            "brain MCP accept queue not probed".to_string(),
        ),
    }
}

/// Evaluate the runtime liveness gates (pure predicates over `LivenessFacts`).
/// `brain_serving` is universal; `indexer_alive` is conditional on the profile
/// (N/A when the `runtime_contract` has no separate indexer).
pub fn evaluate_liveness_gates(l: &LivenessFacts) -> Vec<Gate> {
    vec![
        Gate::binary(
            "brain_serving",
            l.brain_serving,
            if l.brain_serving {
                "brain DB probe SELECT 1 ok"
            } else {
                "brain not serving (db_probe_failed)"
            },
        ),
        indexer_alive_gate(l),
        brain_accept_queue_gate(l),
    ]
}

/// REQ-AXO-902616 critère 1 — `indexer_alive` compare le propriétaire du battement
/// au processus que le SUPERVISEUR suit, et n'est jamais vert quand les deux
/// divergent.
///
/// `Unknown` et non `Fail` : un battement frais n'est pas une panne, c'est une
/// vivacité dont on ne sait plus DE QUI elle parle. Et `Fail` ferait basculer un
/// promote en tier-2 (`stop --hard`, coupure du brain) là où le remède est le
/// redémarrage du seul indexeur — `indexer_alive` et `indexer_process_stable` sont
/// déjà tous deux dans `REPARABLE_PAR_INDEXEUR`.
///
/// Quand la propriété n'est pas mesurée (`Default`), le verdict est celui d'avant
/// au bit près : `CutoverFacts::new_healthy()` en dépend, et un `Unknown` glissé là
/// enverrait chaque cutover en auto-rollback.
fn indexer_alive_gate(l: &LivenessFacts) -> Gate {
    if !l.indexer_expected {
        return Gate::binary(
            "indexer_alive",
            true,
            "no separate indexer in runtime_contract — gate N/A".to_string(),
        );
    }
    if l.indexer_ready && l.ist_ownership.label(l.supervised_pid) == "diverged" {
        return Gate::unknown(
            "indexer_alive",
            format!(
                "heartbeat is fresh ({}) but the IST writer lock is held by pid={} while the \
                 supervisor tracks pid={} — the indexer that works is NOT the one being \
                 supervised, so this heartbeat says nothing about the supervised process",
                l.indexer_source,
                l.ist_ownership
                    .owner_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                l.supervised_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
            ),
        );
    }
    Gate::binary(
        "indexer_alive",
        l.indexer_ready,
        if l.indexer_ready {
            format!("indexer healthy ({})", l.indexer_source)
        } else {
            format!("indexer {} ({})", l.indexer_lifecycle, l.indexer_source)
        },
    )
}

/// Liveness phase, taking precedence over the release-state phase when red.
pub fn liveness_phase(l: &LivenessFacts) -> Option<&'static str> {
    if !l.brain_serving {
        Some("brain_down")
    } else if l.accept_queue_depth.is_some_and(|depth| depth > ACCEPT_QUEUE_MAX_HEALTHY_DEPTH) {
        Some("brain_accept_queue_saturated")
    } else if l.indexer_expected && !l.indexer_ready {
        Some("indexer_down")
    } else {
        None
    }
}

/// The corrective action for a liveness failure, keyed on the lifecycle verdict so a
/// stale heartbeat (restart) is distinguished from a never-launched indexer (start).
pub fn liveness_next_action(l: &LivenessFacts) -> Option<String> {
    if !l.brain_serving {
        return Some(
            "brain process up but DB probe (SELECT 1) failed — check Postgres reachability, then restart the brain."
                .to_string(),
        );
    }
    if let Some(depth) = l.accept_queue_depth {
        if depth > ACCEPT_QUEUE_MAX_HEALTHY_DEPTH {
            return Some(format!(
                "brain MCP accept queue backlog saturated ({depth} > {ACCEPT_QUEUE_MAX_HEALTHY_DEPTH}) — new TCP connections not accepted. Restart the brain: `curl -X POST :8080/process/restart/axon-brain`."
            ));
        }
    }
    if l.indexer_expected && !l.indexer_ready {
        return Some(match l.indexer_lifecycle.as_str() {
            "crashed_or_abandoned" => "indexer heartbeat went stale — restart the indexer only (`curl -X POST :8080/process/restart/axon-indexer`), NOT the whole stack: a full restart takes the brain down with it (PIL-AXO-008, REQ-AXO-902256). Then re-check.".to_string(),
            "never_launched" => "no indexer heartbeat — the split indexer was never started; start the full runtime (`./scripts/axon-live start full`).".to_string(),
            // REQ-AXO-902581 — ces deux verdicts remplacent l'inférence « périmé
            // donc mort ». `exited_clean` est OBSERVÉ : rien à réparer, l'indexeur
            // a fini sa passe. `stopped_or_idle` est INFÉRÉ : la remédiation
            // commence par MESURER, pas par redémarrer un processus sain.
            "exited_clean" => "l'indexeur a terminé sa passe proprement (`Completed`, exit 0) — RIEN à réparer. Relancer une passe si un nouveau travail est en attente : `curl -X POST :8080/process/start/axon-indexer`.".to_string(),
            "stopped_or_idle" => "le battement de l'indexeur s'est tu ; personne n'a observé POURQUOI. Demander au superviseur avant de conclure : `curl -s :8080/processes | jq '.data[] | select(.name==\"axon-indexer\")'`. `Completed` + exit 0 = fin de passe normale ; `Failed` ou `Restarting` = la panne.".to_string(),
            _ => "indexer not ready — inspect the indexer process and its heartbeat.".to_string(),
        });
    }
    None
}

/// REQ-AXO-902585 — l'état d'une porte a TROIS valeurs, pas deux.
///
/// `Unknown` n'est pas une nuance : `qualification_passed` rendait `pass: true`
/// sur l'ABSENCE de preuve (« no qualification recorded »), et un `pass` se lit
/// comme un verdict. Une porte non jouée n'est pas une porte franchie. Même règle
/// que `ok_uncounted` sur `sql` (REQ-AXO-902583) : une surface n'affirme jamais
/// plus qu'elle ne sait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    Pass,
    Fail,
    Unknown,
}

/// A single declarative gate: a named predicate over the facts with a human detail.
///
/// Le champ `pass: bool` a été REMPLACÉ par `status` volontairement : le
/// compilateur force ainsi chaque site de construction à choisir un état, et une
/// porte future ne peut plus oublier le troisième.
#[derive(Debug, Clone)]
pub struct Gate {
    pub name: &'static str,
    pub status: GateStatus,
    pub detail: String,
}

impl Gate {
    pub fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Gate { name, status: GateStatus::Pass, detail: detail.into() }
    }
    pub fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Gate { name, status: GateStatus::Fail, detail: detail.into() }
    }
    pub fn unknown(name: &'static str, detail: impl Into<String>) -> Self {
        Gate { name, status: GateStatus::Unknown, detail: detail.into() }
    }
    /// Porte binaire — pour les prédicats qui SAVENT toujours répondre.
    pub fn binary(name: &'static str, pass: bool, detail: impl Into<String>) -> Self {
        if pass { Gate::pass(name, detail) } else { Gate::fail(name, detail) }
    }
    /// Lecture conservatrice pour les consommateurs existants : `Unknown` n'est
    /// pas un succès.
    pub fn passes(&self) -> bool {
        self.status == GateStatus::Pass
    }
    /// `Unknown` n'est PAS rouge — capital pour la sûreté : `promote_live_safe.sh`
    /// escalade en redémarrage complet du brain dès qu'un gate rouge inattendu
    /// apparaît dans `failed_gates`.
    pub fn is_red(&self) -> bool {
        self.status == GateStatus::Fail
    }
    pub fn status_str(&self) -> &'static str {
        match self.status {
            GateStatus::Pass => "pass",
            GateStatus::Fail => "fail",
            GateStatus::Unknown => "unknown",
        }
    }
}

/// Evaluate the release gates. These are the T1 predicates; T2 re-expresses them in
/// Ascent without changing their meaning.
pub fn evaluate_gates(f: &ReleaseFacts) -> Vec<Gate> {
    let manifest_match = f.manifest_build_id.as_deref() == Some(f.live_build_id.as_str());
    let source = f.qualification_source;
    let evidence = f.core_qualification_evidence.as_deref().unwrap_or("<none>");
    let qualification = Gate {
        name: "qualification_passed",
        status: f.qualification_status(),
        detail: match f.qualification_status() {
            GateStatus::Pass => format!("core_qualification=passed (source={source}) — {evidence}"),
            GateStatus::Fail => format!(
                "core_qualification={} (source={source}) — {evidence}",
                f.core_qualification_status.as_deref().unwrap_or("?")
            ),
            // REQ-AXO-902585 — dit franchement, et ce sera le cas MAJORITAIRE :
            // 243 des 244 manifestes d'historique n'ont pas de `promotion_gates`.
            // Ce n'est pas une régression, c'est la fin d'un faux vert.
            GateStatus::Unknown => match f.core_qualification_status.as_deref() {
                Some(other) => format!("core_qualification={other} (source={source}) — ni passé ni échoué : non mesurable"),
                None if f.pending_present => "un staging est en vol et n'a pas encore enregistré de core_qualification".to_string(),
                None => "ce manifeste ne porte pas de `promotion_gates` (antérieur au promote en deux phases) — le verdict de qualification n'est PAS mesurable ici. Unknown, pas vert.".to_string(),
            },
        },
    };
    vec![
        Gate::binary(
            "manifest_runtime_match",
            manifest_match,
            format!(
                "running={} manifest={}",
                f.live_build_id,
                f.manifest_build_id.as_deref().unwrap_or("<none>")
            ),
        ),
        Gate::binary(
            "no_stale_pending",
            !f.pending_present,
            if f.pending_present {
                format!(
                    "pending.json present (build_id={})",
                    f.pending_build_id.as_deref().unwrap_or("<unknown>")
                )
            } else {
                "no pending staging".to_string()
            },
        ),
        qualification,
    ]
}

/// Derive the release phase from the facts (the projection of the FSM state).
pub fn phase(f: &ReleaseFacts) -> &'static str {
    if f.pending_present {
        // A staging exists: either a promote is mid-flight or it was stranded.
        "staged"
    } else if f.manifest_build_id.is_none() {
        "uninitialized"
    } else if f.manifest_build_id.as_deref() != Some(f.live_build_id.as_str()) {
        "drift"
    } else {
        "clean"
    }
}

/// The single corrective action that closes the gap, or `None` when clean.
pub fn next_action(f: &ReleaseFacts) -> Option<String> {
    match phase(f) {
        "staged" => Some(format!(
            // REQ-AXO-902256 — `promote-live --resume` no longer exists; the resume path is
            // a re-run of promote_live_safe.sh, which detects the stranded pending and
            // replays the cutover on that build's candidate manifest (byte-verified).
            "a promote is mid-flight or stranded (pending build_id={}). If no promote is running: re-run `bash scripts/release/promote_live_safe.sh --project <CODE>` (it auto-resumes the stranded build via the cutover), or roll back with `bash scripts/release/rollback_live.sh`.",
            f.pending_build_id.as_deref().unwrap_or("<unknown>")
        )),
        "drift" => Some(format!(
            "running build_id ({}) != promoted manifest ({}). Re-promote HEAD (`promote_live_safe.sh --project AXO`) or roll back (`rollback_live.sh`).",
            f.live_build_id,
            f.manifest_build_id.as_deref().unwrap_or("<none>")
        )),
        "uninitialized" => {
            Some("no current.json manifest — run an initial promote to record the live release.".to_string())
        }
        _ => None,
    }
}

/// REQ-AXO-902585 (défaut 2) — l'échec que `phase: "clean"` ne dit pas.
///
/// `clean` est vrai AU CONTRAT (manifeste == runtime) et c'est cette vérité étroite
/// qui produisait `next_action: null`. Mais l'outil porte dans son propre `trace` la
/// preuve qu'une promotion a échoué depuis, et que le `release_attempt_id` courant
/// n'est pas celui de cette tentative. Un agent lisait « rien à faire » et repartait.
///
/// Rendu en DERNIÈRE priorité : la liveness et la phase de release parlent d'abord,
/// parce qu'un service à terre prime sur un déploiement raté.
pub fn attempt_next_action(f: &ReleaseFacts) -> Option<String> {
    // `running` n'est PAS un échec — et ce n'est pas un détail : pendant un promote,
    // `attempt-current.status` vaut « running », et le script relit `promote_status`
    // en boucle. Y voir un problème ferait basculer le promote en redémarrage
    // complet, en plein vol.
    // REQ-AXO-902628 — `incomplete` compte AUSSI. Le conseil de reprise etait
    // supprime pour tout ce qui n'etait pas `failed` : un promote tue en plein
    // build sortait `completed`, donc silence total. C'est la moitie symetrique
    // du gate ci-dessus — corriger l'un sans l'autre laisserait l'operateur sans
    // verdict ET sans conseil.
    let statut_actionnable = matches!(f.attempt_status.as_deref(), Some("failed") | Some("incomplete"));
    if !statut_actionnable {
        return None;
    }
    let id = f.attempt_id.as_deref().unwrap_or("<unknown>");
    let phase_echec = f.attempt_phase.as_deref().unwrap_or("<unknown>");
    let detail = f.attempt_last_event_detail.as_deref().unwrap_or("<none>");
    let journal = f.attempt_journal_path.as_deref().unwrap_or("<none>");
    // REQ-AXO-902628 — dire le bon mot. « FAILED » sur une tentative `incomplete`
    // serait faux dans l'autre sens : rien n'a echoue, le script est mort sans le
    // dire. Un verdict faux reste un verdict faux, meme quand il alarme.
    let verdict = if f.attempt_status.as_deref() == Some("incomplete") {
        "exited INCOMPLETE (nothing proves the cutover finished)"
    } else {
        "FAILED"
    };
    let meme_tentative = f.attempt_id.is_some() && f.attempt_id == f.release_attempt_id;
    if meme_tentative {
        Some(format!(
            "the live manifest WAS produced by attempt {id}, and that same attempt then \
             {verdict} at phase={phase_echec} ({detail}). Do not read this as a complete \
             release: a later step failed after the manifest was finalised. Read the \
             journal: `tail -5 {journal}`."
        ))
    } else {
        Some(format!(
            "the release is coherent (running == manifest), BUT the most recent recorded \
             promote attempt {id} {verdict} at phase={phase_echec} ({detail}); the live \
             manifest was produced by a DIFFERENT attempt ({}). Nothing is down right now \
             — but the change you tried to ship is NOT live. Read the journal: \
             `tail -5 {journal}`.",
            f.release_attempt_id.as_deref().unwrap_or("<none>")
        ))
    }
}

/// REQ-AXO-902616 — qui tient VRAIMENT le verrou d'écriture IST.
///
/// La sonde `indexer_alive` juge sur la fraîcheur du battement PG. Elle est restée
/// verte 21 heures pendant que le processus SUPERVISÉ mourait toutes les 30 s : le
/// battement était alimenté par un orphelin que le superviseur ne suivait plus.
/// Une sonde qui reste verte pendant une panne totale de supervision ne mesure pas
/// ce que son nom promet.
///
/// `probed: false` est le défaut et signifie « je n'ai pas mesuré » — jamais
/// « personne ne tient ». C'est exactement la confusion que ce type supprime :
/// le gate disait « nothing holds » sans l'avoir vérifié une seule fois.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IstOwnershipFacts {
    /// La sonde du flock a-t-elle abouti ? Faux ⇒ « unmeasured », jamais un verdict.
    pub probed: bool,
    /// Un processus VIVANT tient le flock (source : `guard_liveness_ist`, qui teste
    /// le flock lui-même et non `/proc/<pid>`, lequel répond vrai pour un zombie).
    pub held_by_live_process: bool,
    /// Pid inscrit dans la métadonnée du verrou, quand elle est lisible.
    pub owner_pid: Option<i64>,
    /// Identité runtime auto-déclarée du propriétaire.
    pub owner_identity: Option<String>,
}

impl IstOwnershipFacts {
    /// REQ-AXO-902616 — quadri-état en CHAÎNE, comme `restart_loop_label` : un
    /// booléen se lirait `false` là où la vérité est « je n'ai pas pu mesurer ».
    ///
    /// `supervised_pid` est le pid que le SUPERVISEUR suit. `diverged` dit
    /// exactement la panne du 2026-09-04 : un indexeur vivant tient le verrou, et
    /// ce n'est pas celui que le superviseur croit piloter.
    pub fn label(&self, supervised_pid: Option<i64>) -> &'static str {
        if !self.probed {
            return "unmeasured";
        }
        if !self.held_by_live_process {
            return "free";
        }
        match (self.owner_pid, supervised_pid) {
            (Some(owner), Some(supervised)) if owner == supervised => "supervised",
            (Some(_), Some(_)) => "diverged",
            _ => "held_by_unknown",
        }
    }

    /// REQ-AXO-902616 critère 3 — nommer le propriétaire réel quand il existe, et
    /// dire s'il est vivant. Jamais supposer.
    pub fn describe(&self) -> String {
        if !self.probed {
            return "IST writer lock NOT probed".to_string();
        }
        if !self.held_by_live_process {
            return "no live process holds the IST writer lock".to_string();
        }
        let who = match (self.owner_pid, self.owner_identity.as_deref()) {
            (Some(pid), Some(identity)) => format!("pid={pid}, identity={identity}"),
            (Some(pid), None) => format!("pid={pid}"),
            (None, Some(identity)) => format!("identity={identity}"),
            (None, None) => "owner metadata unreadable".to_string(),
        };
        format!("a LIVE process holds the IST writer lock ({who})")
    }
}

/// REQ-AXO-902585 (défaut 3) — ce que le superviseur sait et que le battement PG
/// ne peut pas savoir. Peuplé par `tools_release.rs` (qui tient l'IO) ; les gates
/// restent des prédicats purs.
#[derive(Debug, Clone, Default)]
pub struct SupervisorFacts {
    /// La sonde a-t-elle abouti ? Faux ⇒ tout verdict est `Unknown`, jamais vert.
    pub reachable: bool,
    /// Cause exacte quand elle n'aboutit pas — dite, pas devinée.
    pub error: Option<String>,
    /// Le rôle a-t-il été trouvé dans la réponse ? Un corps parsé sans ce rôle et
    /// un corps illisible sont deux verdicts différents.
    pub role_found: bool,
    pub status: String,
    pub restarts: i64,
    pub pid: i64,
    pub age_ms: i64,
    /// REQ-AXO-902581 — le code de sortie que le SUPERVISEUR a observé. `Completed`
    /// avec 0 est une fin de passe NORMALE ; sans ce champ, le battement PG périmé
    /// restait la seule source et concluait au crash.
    pub exit_code: i64,
    /// REQ-AXO-902581 — le processus tourne-t-il encore ? Un processus en marche
    /// mais muet n'est ni une fin de passe ni une sortie : c'est un blocage.
    pub is_running: bool,
    /// Fraîcheur du battement PG, pour le recoupement.
    pub heartbeat_age_ms: Option<i64>,
    /// REQ-AXO-902616 — qui tient le verrou d'écriture IST. `Default` = non mesuré,
    /// donc un appelant qui ne sonde pas garde exactement l'ancien comportement.
    pub ist_ownership: IstOwnershipFacts,
}

impl SupervisorFacts {
    /// REQ-AXO-902585 — quadri-état en CHAÎNE. Un booléen se lirait « false » là où
    /// la vérité est « je n'ai pas pu mesurer », et c'est exactement la confusion
    /// que cette tranche supprime. Même principe qu'`ok_uncounted` sur `sql`.
    pub fn restart_loop_label(&self) -> &'static str {
        if !self.reachable {
            "unmeasured"
        } else if !self.role_found {
            "unmeasured"
        } else if self.status == "Restarting"
            || (self.restarts >= crate::supervisor_probe::SUPERVISOR_RESTART_LOOP_MIN_RESTARTS
                && self.age_ms < crate::supervisor_probe::SUPERVISOR_YOUNG_PROCESS_MS)
        {
            "detected"
        } else if self.restarts >= 1
            && self.age_ms < crate::supervisor_probe::SUPERVISOR_YOUNG_PROCESS_MS
        {
            "unproven"
        } else {
            "not_detected"
        }
    }
}

/// REQ-AXO-902585 — le verdict sur la STABILITÉ du rôle, distinct de sa vivacité.
///
/// `indexer_alive` répond « un battement récent existe-t-il ? ». Ce gate répond
/// « le même processus tient-il ? ». Les deux sont vrais séparément : pendant la
/// boucle mesurée, le premier passait et le second aurait dû rougir.
pub fn evaluate_supervisor_gates(s: &SupervisorFacts) -> Vec<Gate> {
    let gate = if !s.reachable {
        Gate::unknown(
            "indexer_process_stable",
            format!(
                "supervisor unreachable ({}) — restart-loop detection is NOT measurable here",
                s.error.as_deref().unwrap_or("no reason given")
            ),
        )
    } else if !s.role_found {
        Gate::unknown(
            "indexer_process_stable",
            "supervisor answered but lists no `axon-indexer` — nothing to judge",
        )
    } else if s.status == "Restarting" {
        // Détecteur PRIMAIRE, et il ne dépend d'aucun seuil : le superviseur dit
        // lui-même qu'il relance.
        // REQ-AXO-902616 défaut 2 — le message disait « nothing holds ». C'était FAUX
        // le 2026-09-04 : 650712 tenait, vivant, propriétaire légitime du flock IST.
        // Et les instances éphémères n'écrivent AUCUN battement — elles sont refusées
        // au boot, avant toute écriture observable (`runtime_boot.rs:815-830`). Le
        // gate décrit désormais ce qu'il a MESURÉ, et se tait quand il n'a rien sondé.
        Gate::fail(
            "indexer_process_stable",
            format!(
                "axon-indexer is `Restarting` (restarts={}, pid={}) — {}",
                s.restarts,
                s.pid,
                s.ist_ownership.describe()
            ),
        )
    } else if s.restarts >= crate::supervisor_probe::SUPERVISOR_RESTART_LOOP_MIN_RESTARTS
        && s.age_ms < crate::supervisor_probe::SUPERVISOR_YOUNG_PROCESS_MS
    {
        Gate::fail(
            "indexer_process_stable",
            format!(
                "restart loop: {} restarts and the current process is only {} ms old — {}",
                s.restarts,
                s.age_ms,
                s.ist_ownership.describe()
            ),
        )
    } else if s.restarts >= 1 && s.age_ms < crate::supervisor_probe::SUPERVISOR_YOUNG_PROCESS_MS {
        // ⚠ Unknown, PAS Fail. Un rouge ici se déclencherait pendant 60 s après le
        // remède que l'outil recommande lui-même (« restart the indexer only ») et
        // pendant chaque cutover, qui redémarre le rôle en place. L'outil dirait de
        // redémarrer, puis crierait « boucle » sur son propre conseil.
        Gate::unknown(
            "indexer_process_stable",
            format!(
                "axon-indexer restarted recently ({} restarts, up {} ms) — too early to tell a \
                 deliberate restart from a loop",
                s.restarts, s.age_ms
            ),
        )
    } else {
        Gate::pass(
            "indexer_process_stable",
            format!(
                "axon-indexer {} (pid={}, restarts={}, up {} ms)",
                s.status, s.pid, s.restarts, s.age_ms
            ),
        )
    };
    vec![gate]
}

/// REQ-AXO-902585 — la porte qui rend visible ce que `attempt_next_action` explique.
/// `running` → `Unknown`, jamais `Fail` : voir la note ci-dessus.
/// REQ-AXO-902628 — la MARQUE de completion que `promote_live_safe.sh` inscrit
/// desormais dans le detail de sa ligne terminale.
///
/// Elle n'est pas cosmetique : c'est ce qui distingue « ce promote a franchi le
/// cutover » de « ce script s'est arrete proprement ». Les deux s'ecrivaient
/// `completed`.
pub(crate) const MARQUE_DE_COMPLETION: &str = "cutover_finalize passed";

/// La dernière étape d'un promote canonique. Franchie ⇒ le cutover a eu lieu.
pub(crate) const ETAPE_TERMINALE: &str = "cutover_finalize";
/// Rendu d'une phase absente — le MÊME mot des deux côtés du jumelage.
const PHASE_ABSENTE: &str = "<none>";
/// Préfixe du seul « incomplete » qui ne juge RIEN : le journal n'a pas pu être lu.
pub(crate) const PREFIXE_JOURNAL_ILLISIBLE: &str = "incomplete:journal unreadable";

/// REQ-AXO-902628 — le juge de complétion, en Rust, jumeau de
/// `scripts/release/promote_completion_evidence.py`.
///
/// POURQUOI DEUX IMPLÉMENTATIONS, dit franchement. L'écrivain
/// (`promote_live_safe.sh`) est du shell et ne peut pas appeler du Rust ; la porte
/// (`promote_status`) est du Rust et ne doit pas forker un `python3` par appel —
/// `promote_live_safe.sh` relit `promote_status` EN BOUCLE pendant le cutover, et
/// REQ-AXO-902589 vient de payer le prix d'une latence périodique sur cette surface
/// exacte.
///
/// La dette est assumée, pas subie : `release_scripts_tests.rs` joue les MÊMES
/// journaux réels à travers les deux chemins et exige le même texte, caractère pour
/// caractère. Sans cette garde croisée les deux dériveraient, et la surface
/// rementirait — autrement.
///
/// Deux conditions, et AUCUNE constante de compte à maintenir : le nœud annonçait
/// « sept étapes », le vrai nombre est QUINZE, et quatorze pour un promote
/// légitimement raccourci (`--skip-build` n'émet pas de `run_step`).
/// 1. toute étape DÉMARRÉE porte son `step_completed` — une étape restée ouverte
///    est une mort en plein vol ;
/// 2. `cutover_finalize` figure parmi les étapes terminées.
pub fn preuve_de_completion(journal: &str) -> String {
    let mut demarrees: Vec<String> = Vec::new();
    let mut terminees: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for ligne in journal.lines() {
        let ligne = ligne.trim();
        if ligne.is_empty() {
            continue;
        }
        // Une ligne illisible n'est PAS une preuve d'incomplétude : le journal est
        // append-only + fsync, une troncature partielle reste possible sur une mort
        // brutale. On l'ignore et on juge sur le reste.
        let Ok(evenement) = serde_json::from_str::<Value>(ligne) else {
            continue;
        };
        let phase = evenement
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or(PHASE_ABSENTE)
            .to_string();
        match evenement.get("event").and_then(Value::as_str) {
            Some("step_started") => demarrees.push(phase),
            Some("step_completed") => {
                terminees.insert(phase);
            }
            _ => {}
        }
    }
    let orphelines: Vec<&str> = demarrees
        .iter()
        .filter(|etape| !terminees.contains(etape.as_str()))
        .map(String::as_str)
        .collect();
    if !orphelines.is_empty() {
        return format!(
            "incomplete:step(s) started but never completed: {} ({} step(s) done)",
            orphelines.join(", "),
            terminees.len()
        );
    }
    if !terminees.contains(ETAPE_TERMINALE) {
        let dernier = demarrees.last().map(String::as_str).unwrap_or(PHASE_ABSENTE);
        return format!(
            "incomplete:{ETAPE_TERMINALE} never completed — {} step(s) done, last started `{dernier}`",
            terminees.len()
        );
    }
    format!("complete:{} steps, {MARQUE_DE_COMPLETION}", terminees.len())
}

/// Ce que le JOURNAL rend sur une tentative — en trois états, jamais deux.
///
/// `Indisponible` n'est pas `Incomplete` : « je n'ai pas pu lire » et « le promote
/// est mort en vol » sont deux faits différents, et les confondre rendrait
/// `unknown` sur un promote sain dont le journal a simplement été archivé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreuveJournal {
    Complete,
    Incomplete,
    Indisponible,
}

pub fn lire_preuve_journal(preuve: Option<&str>) -> PreuveJournal {
    match preuve {
        Some(p) if p.starts_with("complete:") => PreuveJournal::Complete,
        Some(p) if p.starts_with(PREFIXE_JOURNAL_ILLISIBLE) => PreuveJournal::Indisponible,
        Some(_) => PreuveJournal::Incomplete,
        None => PreuveJournal::Indisponible,
    }
}

/// REQ-AXO-902628 — un statut terminal `completed` prouve-t-il quelque chose ?
///
/// Pris en ENTREE plutot que lu sur le disque, pour que les deux verdicts soient
/// atteignables sans fabriquer une projection.
pub fn completion_est_prouvee(statut: Option<&str>, detail: Option<&str>) -> bool {
    statut == Some("completed") && detail.is_some_and(|d| d.contains(MARQUE_DE_COMPLETION))
}

pub fn evaluate_attempt_gate(f: &ReleaseFacts) -> Gate {
    // REQ-AXO-902628 — `completed` n'est PLUS une preuve à lui seul.
    //
    // Mesure du 2026-09-07 sur les 84 journaux archivés : SEPT portent
    // `completed` sans avoir franchi le cutover, dont CINQ sans une seule étape
    // terminée. Cette porte les rendait tous `pass`. Une session qui lit
    // `promote_status` y voyait un promote réussi là où le script avait été tué
    // en plein build — le mode d'échec de `CPT-AXO-025` branche 1, produit par
    // notre propre surface.
    //
    // DEUX TÉMOINS, indépendants, aucun déduit de l'autre :
    //  1. le JOURNAL, rejugé à chaque appel (`attempt_completion_evidence`) —
    //     c'est lui qui rend le verdict RÉTROACTIF sur les 84 journaux déjà
    //     archivés, écrits bien avant l'écrivain corrigé ;
    //  2. la MARQUE inscrite par l'écrivain dans le détail terminal — elle reste
    //     le seul témoin quand le journal n'est plus lisible (archivé, purgé).
    //
    // Le journal prime quand il parle : c'est la source, la marque n'en est que
    // le résumé. Et `unknown` n'est délibérément PAS rouge — `promote_live_safe.sh`
    // escalade en redémarrage complet du brain sur un gate rouge inattendu, ce
    // qu'une projection muette ne justifie pas.
    let id = f.attempt_id.as_deref().unwrap_or("<unknown>");
    let detail = f.attempt_last_event_detail.as_deref().unwrap_or("<none>");
    let journal = f.attempt_journal_path.as_deref().unwrap_or("<none>");
    match f.attempt_status.as_deref() {
        Some("completed") => match lire_preuve_journal(f.attempt_completion_evidence.as_deref()) {
            PreuveJournal::Complete => Gate::pass(
                "last_promote_attempt",
                format!(
                    "attempt {id} completed — {detail} (journal: {})",
                    f.attempt_completion_evidence.as_deref().unwrap_or("<none>")
                ),
            ),
            // Le journal existe et il DÉMENT la projection. C'est le cas mesuré,
            // et le dire est tout l'objet du REQ.
            PreuveJournal::Incomplete => Gate::unknown(
                "last_promote_attempt",
                format!(
                    "attempt {id} is recorded `completed`, but its own journal does NOT \
                     prove the cutover finished — {}. A promote killed mid-build records \
                     exactly this. Read it: `tail -5 {journal}`.",
                    f.attempt_completion_evidence.as_deref().unwrap_or("<none>")
                ),
            ),
            // Journal illisible ou absent : il reste la marque, écrite par
            // l'écrivain au moment où il tenait le journal complet sous les yeux.
            PreuveJournal::Indisponible => {
                if completion_est_prouvee(
                    f.attempt_status.as_deref(),
                    f.attempt_last_event_detail.as_deref(),
                ) {
                    Gate::pass(
                        "last_promote_attempt",
                        format!(
                            "attempt {id} completed — {detail} (journal unreadable: the \
                             `{MARQUE_DE_COMPLETION}` mark is the only witness left)"
                        ),
                    )
                } else {
                    Gate::unknown(
                        "last_promote_attempt",
                        format!(
                            "attempt {id} is recorded `completed`, but NOTHING proves the \
                             cutover finished: its journal could not be read ({}) and the \
                             detail carries no `{MARQUE_DE_COMPLETION}` mark (detail: \
                             {detail}). Read the journal before trusting it: \
                             `tail -5 {journal}`.",
                            f.attempt_completion_evidence
                                .as_deref()
                                .unwrap_or("no journal path in the projection")
                        ),
                    )
                }
            }
        },
        // REQ-AXO-902628 — le statut que l'ecrivain pose quand il sort a rc=0
        // sans pouvoir prouver la completion. Ni `pass` (rien n'est prouve), ni
        // `fail` (rien ne dit qu'un service est a terre).
        Some("incomplete") => Gate::unknown(
            "last_promote_attempt",
            format!(
                "attempt {id} exited cleanly but INCOMPLETE — {detail}. Nothing was \
                 necessarily promoted. Read the journal: `tail -5 {journal}`."
            ),
        ),
        Some("failed") => Gate::fail(
            "last_promote_attempt",
            format!(
                "attempt {id} FAILED at phase={} — {detail}",
                f.attempt_phase.as_deref().unwrap_or("<unknown>")
            ),
        ),
        Some("running") => Gate::unknown(
            "last_promote_attempt",
            "a promote is running right now — its verdict is not knowable yet",
        ),
        Some(other) => Gate::unknown(
            "last_promote_attempt",
            format!("attempt status `{other}` — neither completed nor failed"),
        ),
        None => Gate::unknown(
            "last_promote_attempt",
            "no attempt projection on disk — nothing to say about the last promote",
        ),
    }
}

// ---------------------------------------------------------------------------
// Cutover FSM (REQ-AXO-902165 — health-gated cutover + auto-rollback).
//
// True blue-green is INFEASIBLE here: the SOLL/IST writer guards are EXCLUSIVE and
// acquired at boot (runtime_boot.rs — a second writer instance is refused startup),
// so the new and old runtimes cannot coexist. The cutover is therefore in-place
// (stop old → start new) with a health-gate + AUTO-ROLLBACK: the new runtime must
// prove the FULL runtime_contract healthy within a deadline, otherwise the previous
// release is restored — turning a failed promote from a stranded outage (the s94
// incident) into a brief blip + auto-recovery. Pure predicates, same shape as the
// release/stop FSMs: facts in, `Vec<Gate>` + derived `phase`/`next_action` out.
// ---------------------------------------------------------------------------

/// Facts about an in-flight in-place cutover, sampled after the new runtime is started.
#[derive(Debug, Clone, Default)]
pub struct CutoverFacts {
    /// Liveness of the NEW (candidate) runtime.
    pub new_liveness: LivenessFacts,
    /// Qualify verdict on the new runtime (`None` = not run / not yet).
    pub new_qualify_ok: Option<bool>,
    /// The health-gate deadline elapsed without the new runtime going healthy.
    pub deadline_exceeded: bool,
    /// Auto-rollback finished: the previous binary + manifest are restored & serving.
    pub old_restored: bool,
}

impl CutoverFacts {
    /// The new runtime is fully healthy: brain serving + indexer alive (per the
    /// runtime_contract) AND qualify not-failed. Reuses the liveness gates as the
    /// single source of truth (an absent qualify verdict is not a failure).
    pub fn new_healthy(&self) -> bool {
        evaluate_liveness_gates(&self.new_liveness)
            .iter()
            .all(|g| g.passes())
            && self.new_qualify_ok != Some(false)
    }
}

/// Evaluate the cutover gate (pure predicate over `CutoverFacts`).
pub fn evaluate_cutover_gates(f: &CutoverFacts) -> Vec<Gate> {
    vec![Gate::binary(
        "new_runtime_healthy",
        f.new_healthy(),
        if f.new_healthy() {
            "new runtime healthy (full runtime_contract + qualify)".to_string()
        } else if f.deadline_exceeded {
            "new runtime NOT healthy within the deadline → auto-rollback".to_string()
        } else {
            "new runtime not yet healthy → awaiting".to_string()
        },
    )]
}

/// Derive the cutover phase (projection of the cutover FSM state). A `healthy` new
/// runtime wins even at the deadline; otherwise a passed deadline triggers rollback.
pub fn cutover_phase(f: &CutoverFacts) -> &'static str {
    if f.new_healthy() {
        "healthy"
    } else if f.old_restored {
        "rolled_back"
    } else if f.deadline_exceeded {
        "rolling_back"
    } else {
        "awaiting_health"
    }
}

/// The single corrective action that advances the cutover, or `None` when the new
/// runtime is healthy (the promote finalizes).
pub fn cutover_next_action(f: &CutoverFacts) -> Option<String> {
    match cutover_phase(f) {
        "healthy" => None,
        "awaiting_health" => Some(
            "new runtime started — poll its liveness (brain_serving + indexer_alive) until healthy or the deadline elapses.".to_string(),
        ),
        "rolling_back" => Some(
            "new runtime failed the health-gate within the deadline — AUTO-ROLLBACK: restore the previous binary + manifest and restart the old release.".to_string(),
        ),
        "rolled_back" => Some(
            "auto-rollback complete: the previous release is serving again; the promote did NOT apply. Investigate the candidate before retrying.".to_string(),
        ),
        _ => None,
    }
}

/// REQ-AXO-902165 — the cutover DRIVER: poll the new runtime's health up to `max_polls`
/// times, returning `Promoted` the instant it is healthy, or `RolledBack` once the polls
/// are exhausted (the deadline). Both effects — the health probe and the inter-poll wait
/// — are INJECTED, so the finalize-vs-rollback decision flow is unit-testable without a
/// runtime or a real clock. The caller (`axonctl cutover`) supplies the real probe
/// (`axonctl liveness`) + the wait (sleep) and performs finalize/rollback on the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutoverOutcome {
    /// New runtime went healthy within the deadline → finalize the promote.
    Promoted,
    /// New runtime never healthy within the deadline → restore the previous release.
    RolledBack,
}

pub fn run_cutover_loop(
    mut probe_healthy: impl FnMut() -> bool,
    max_polls: usize,
    mut wait_between_polls: impl FnMut(),
) -> CutoverOutcome {
    for _ in 0..max_polls.max(1) {
        if probe_healthy() {
            return CutoverOutcome::Promoted;
        }
        wait_between_polls();
    }
    CutoverOutcome::RolledBack
}

// ---------------------------------------------------------------------------
// Cutover CHOREOGRAPHY (REQ-AXO-902165 — the I/O executor's decision layer).
//
// `drive_cutover` sequences the side-effecting steps of an in-place cutover
// (snapshot → stage → restart → poll-health → finalize|rollback) around the
// already-tested `run_cutover_loop` driver. Every effect is behind the injected
// `CutoverIo` trait + a separate health probe/wait, so the WHOLE finalize-vs-
// rollback decision flow — including the s94 incident guard (an unhealthy
// candidate must ALWAYS restore the old release, never strand a half-finalized
// manifest) — is unit-testable without a runtime, a clock, or disk (practice 128:
// the decision + driver are pure/injected; only the real `CutoverIo` impl in
// `axonctl` touches bin/*, manifests, and processes, and that is gated on an E2E
// DEV fault-injection run before it may drive a live promote).
// ---------------------------------------------------------------------------

/// The side-effecting steps of an in-place cutover, injected so the choreography is
/// testable without a runtime. The real impl (`axonctl`'s `RealCutoverIo`) replicates
/// the real manifest/bin I/O (axonctl); a fake records the call order + scripted errors.
/// NOTE: the fake replaces the RESTART step, so it cannot exercise start.sh's
/// re-materialisation of bin/* from a manifest — the exact gap that let promote 1400 ship
/// the wrong binary (REQ-AXO-902258). Byte equality is asserted in axonctl's own tests.
///
/// Invariant every impl must uphold: after `rollback()` returns `Ok`, the PREVIOUS
/// release (captured by `snapshot_current`) is restored on disk and restarting.
pub trait CutoverIo {
    /// Capture the currently-serving release (bin/* + current.json) as the rollback
    /// target. Runs BEFORE anything is mutated; `Err` aborts with nothing touched.
    fn snapshot_current(&mut self) -> Result<(), String>;
    /// Stage the candidate: write pending.json (state=staged) + swap the candidate
    /// bin/* into place. `Err` → the old release is restored (bin/* may be partial).
    fn stage_candidate(&mut self) -> Result<(), String>;
    /// Restart the runtime onto the swapped binaries (stop old → start new).
    fn restart_runtime(&mut self) -> Result<(), String>;
    /// Finalize the promote: archive current→history, pending→current (state=promoted).
    fn finalize(&mut self) -> Result<(), String>;
    /// AUTO-ROLLBACK: restore bin/* from the snapshot (current.json), drop pending,
    /// restart the previous release. Must leave the OLD release serving.
    fn rollback(&mut self) -> Result<(), String>;
}

/// The terminal verdict of a cutover: either the candidate went healthy and was
/// finalized, or it failed (at a named step) and the old release was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutoverVerdict {
    /// New runtime healthy within the deadline → promote finalized.
    Promoted,
    /// Candidate failed at `failed_step`; the old release was restored. `rollback_ok`
    /// is the result of the restore itself — `false` means the rollback ALSO failed
    /// (a genuine outage requiring operator action, surfaced distinctly).
    RolledBack {
        failed_step: &'static str,
        rollback_ok: bool,
        detail: Option<String>,
    },
}

impl CutoverVerdict {
    pub fn is_promoted(&self) -> bool {
        matches!(self, CutoverVerdict::Promoted)
    }
}

/// REQ-AXO-902165 — the in-place cutover choreography. Composes the tested cutover
/// driver (`run_cutover_loop`) with the injected I/O steps + a health probe/wait
/// (kept separate from `io` so the poll never double-borrows the effects object).
///
/// Failure handling (the incident guard): a failure of `stage_candidate`,
/// `restart_runtime`, OR the health-gate all funnel into `rollback()` and return
/// `RolledBack` — so a bad candidate is a blip + auto-recovery, never a stranded
/// outage. A `snapshot_current` failure aborts BEFORE any mutation (old release
/// untouched, so no rollback is attempted — nothing was changed).
pub fn drive_cutover<Io, Probe, Wait>(
    io: &mut Io,
    mut probe_healthy: Probe,
    max_polls: usize,
    wait_between_polls: Wait,
) -> CutoverVerdict
where
    Io: CutoverIo,
    Probe: FnMut() -> bool,
    Wait: FnMut(),
{
    // Snapshot first. If we cannot even capture a rollback target, do NOT touch the
    // running release — abort with everything intact (no rollback needed/possible).
    if let Err(e) = io.snapshot_current() {
        return CutoverVerdict::RolledBack {
            failed_step: "snapshot_current",
            rollback_ok: true, // nothing was mutated; the old release still serves.
            detail: Some(e),
        };
    }
    // From here on, any failure restores the snapshot.
    if let Err(e) = io.stage_candidate() {
        return rolled_back(io, "stage_candidate", e);
    }
    if let Err(e) = io.restart_runtime() {
        return rolled_back(io, "restart_runtime", e);
    }
    // Health-gate the new runtime. `run_cutover_loop` returns the instant it is
    // healthy, or `RolledBack` once the deadline (max_polls) is exhausted.
    match run_cutover_loop(&mut probe_healthy, max_polls, wait_between_polls) {
        CutoverOutcome::Promoted => match io.finalize() {
            Ok(()) => CutoverVerdict::Promoted,
            // Candidate was healthy but the manifest finalize failed: the new runtime
            // IS serving, but current.json wasn't advanced. Roll back to the coherent
            // old release rather than leave bin/* ↔ manifest drift (the s91 failure).
            Err(e) => rolled_back(io, "finalize", e),
        },
        CutoverOutcome::RolledBack => rolled_back(
            io,
            "health_gate",
            "new runtime never healthy within the deadline".to_string(),
        ),
    }
}

/// Restore the old release and build the `RolledBack` verdict, recording whether the
/// restore itself succeeded (a failed rollback = a real outage, surfaced distinctly).
fn rolled_back<Io: CutoverIo>(
    io: &mut Io,
    failed_step: &'static str,
    detail: String,
) -> CutoverVerdict {
    let rollback_ok = io.rollback().is_ok();
    CutoverVerdict::RolledBack {
        failed_step,
        rollback_ok,
        detail: Some(detail),
    }
}

// ---------------------------------------------------------------------------
// Cutover scopes lifecycle (REQ-AXO-902591).
//
// Transient cutover scopes `axon-live-cutover-{pid}-{sequence}.scope` placed
// into `nexus-core.slice` by `axonctl cutover` can outlive their cutover when
// background children (like watchman) stay alive. These helpers allow probing
// and purging orphaned scopes so they stop retaining memory in nexus-core.slice.
// ---------------------------------------------------------------------------

/// Informations extraites d'une unité de scope systemd de cutover `axon-live-cutover-{pid}-{sequence}.scope` (REQ-AXO-902591).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutoverScopeInfo {
    pub name: String,
    pub creator_pid: u32,
    pub sequence: u64,
}

/// Parse un nom d'unité cutover (avec ou sans `.scope`) au format `axon-live-cutover-{pid}-{seq}`.
pub fn parse_cutover_scope_unit(unit_name: &str) -> Option<CutoverScopeInfo> {
    let clean_name = unit_name.strip_suffix(".scope").unwrap_or(unit_name);
    let rest = clean_name.strip_prefix("axon-live-cutover-")?;
    let mut parts = rest.split('-');
    let pid_str = parts.next()?;
    let seq_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let creator_pid = pid_str.parse::<u32>().ok()?;
    let sequence = seq_str.parse::<u64>().ok()?;
    Some(CutoverScopeInfo {
        name: unit_name.to_string(),
        creator_pid,
        sequence,
    })
}

/// Extrait les unités de scope cutover depuis la sortie textuelle de `systemctl list-units`.
pub fn parse_cutover_scopes_listing(listing: &str) -> Vec<CutoverScopeInfo> {
    listing
        .lines()
        .filter_map(|line| {
            let unit = line.split_whitespace().next()?;
            parse_cutover_scope_unit(unit)
        })
        .collect()
}

/// Filtre les scopes de cutover orphelins :
/// - soit leur créateur n'est plus vivant selon `pid_alive`,
/// - soit un `current_pid` est spécifié et différent du créateur du scope (scope d'un promote antérieur).
pub fn filter_orphaned_scopes<F>(
    scopes: &[CutoverScopeInfo],
    current_pid: Option<u32>,
    pid_alive: F,
) -> Vec<CutoverScopeInfo>
where
    F: Fn(u32) -> bool,
{
    scopes
        .iter()
        .filter(|scope| {
            if !pid_alive(scope.creator_pid) {
                return true;
            }
            if let Some(cur) = current_pid {
                if scope.creator_pid != cur {
                    return true;
                }
            }
            false
        })
        .cloned()
        .collect()
}

/// Interroge systemd (en mode user) pour lister les scopes cutover vivants.
pub fn probe_live_cutover_scopes() -> Vec<CutoverScopeInfo> {
    let output = std::process::Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--type=scope",
            "--all",
            "--no-legend",
            "--plain",
            "--full",
            "axon-live-cutover-*.scope",
        ])
        .output()
        .ok();
    match output {
        Some(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            parse_cutover_scopes_listing(&stdout)
        }
        _ => Vec::new(),
    }
}

/// Purge les scopes de cutover orphelins :
/// Exécute `systemctl --user stop <unit>` pour libérer la mémoire retenue dans `nexus-core.slice`.
pub fn purge_orphaned_cutover_scopes(current_pid: Option<u32>) -> Result<usize, String> {
    let scopes = probe_live_cutover_scopes();
    let is_alive = |pid: u32| std::path::Path::new(&format!("/proc/{pid}")).exists();
    let orphans = filter_orphaned_scopes(&scopes, current_pid, is_alive);
    if orphans.is_empty() {
        return Ok(0);
    }
    let mut purged = 0;
    for orphan in &orphans {
        let res = std::process::Command::new("systemctl")
            .args(["--user", "stop", &orphan.name])
            .output();
        if let Ok(out) = res {
            if out.status.success() {
                purged += 1;
            }
        }
    }
    Ok(purged)
}

// ---------------------------------------------------------------------------
// Stop FSM (REQ-AXO-902111 — stop-verdict slice).
//
// The stop verdict must live where it SURVIVES the thing being stopped: in
// `axonctl` (the supervisor, which outlives the brain it tears down), NOT in an
// MCP tool of the brain (which dies mid-answer the moment its own listener is
// reaped). So the gates live here as pure predicates and `axonctl::cmd_stop`
// populates `StopFacts` from `find_instance_all_pids` + a PC-daemon probe (the
// wiring step is orchestrator-side; see WIRING.md). Same shape as the release
// gates above: facts in, `Vec<Gate>` + derived `phase`/`next_action` out.
// ---------------------------------------------------------------------------

/// Facts about an in-flight stop, collected by `axonctl` AFTER it has emitted the
/// teardown signals. All scoped to the role being stopped (`stop_role`): "all" for
/// a full teardown, or a single role ("brain"/"indexer") for a role-scoped stop
/// that intentionally preserves the other role (PIL-AXO-004 split deployment).
#[derive(Debug, Clone, Default)]
pub struct StopFacts {
    /// Which role we asked to stop: "all" | "brain" | "indexer".
    pub stop_role: String,
    /// Live PIDs still bound to the canonical listeners for `stop_role` (post-SIGTERM).
    /// A non-empty set means a process survived the teardown = orphaned.
    pub canonical_listeners: Vec<i32>,
    /// The brain MCP port is still bound (may be kernel TIME_WAIT draining even when
    /// `canonical_listeners` is already empty).
    pub brain_port_bound: bool,
    /// The supervisor (PC-daemon / axonctl supervise loop) is still alive. For a full
    /// teardown this is an orphan (it will respawn the role we killed); for a
    /// role-scoped stop it is expected (it keeps the surviving role up).
    pub supervisor_healthy: bool,
    /// Writer locks still held on disk (e.g. IST writer lock files) for `stop_role`.
    pub writer_locks_held: Vec<String>,
    /// Control sockets (telemetry/mcp) still present on disk.
    pub sockets_present: bool,
    /// The indexer heartbeat is still fresh (draining indicator when the indexer is
    /// the role being stopped).
    pub indexer_heartbeat_fresh: bool,
}

impl StopFacts {
    fn is_full_teardown(&self) -> bool {
        self.stop_role.eq_ignore_ascii_case("all")
    }
}

/// Evaluate the stop gates (pure predicates over `StopFacts`).
/// `no_canonical_listeners` + `writer_locks_released` + `sockets_cleaned` are
/// universal; `supervisor_quiesced` is N/A for a role-scoped stop (the supervisor
/// stays up for the surviving role by design).
pub fn evaluate_stop_gates(f: &StopFacts) -> Vec<Gate> {
    let full = f.is_full_teardown();
    vec![
        Gate::binary(
            "no_canonical_listeners",
            f.canonical_listeners.is_empty(),
            if f.canonical_listeners.is_empty() {
                format!("no canonical listeners left for role '{}'", f.stop_role)
            } else {
                format!(
                    "listeners survived for role '{}' (pids={:?})",
                    f.stop_role, f.canonical_listeners
                )
            },
        ),
        Gate::binary(
            "supervisor_quiesced",
            // N/A unless this is a full teardown: a role-scoped stop intentionally
            // leaves the supervisor running for the surviving role (PIL-AXO-004).
            !full || !f.supervisor_healthy,
            if !full {
                format!(
                    "role-scoped stop ('{}') — supervisor stays up for the other role; gate N/A",
                    f.stop_role
                )
            } else if f.supervisor_healthy {
                "supervisor still healthy — it will respawn the role just killed".to_string()
            } else {
                "supervisor quiesced".to_string()
            },
        ),
        Gate::binary(
            "writer_locks_released",
            f.writer_locks_held.is_empty(),
            if f.writer_locks_held.is_empty() {
                "no writer locks held".to_string()
            } else {
                format!(
                    "writer locks still held: {}",
                    f.writer_locks_held.join(", ")
                )
            },
        ),
        Gate::binary(
            "sockets_cleaned",
            !f.sockets_present,
            if f.sockets_present {
                "control sockets still present on disk".to_string()
            } else {
                "control sockets cleaned".to_string()
            },
        ),
    ]
}

/// Derive the stop phase (the projection of the stop FSM state).
///
/// Precedence: orphaned (a live listener survived OR a full-teardown supervisor is
/// still alive) > stopping (listeners gone but ports/heartbeat draining or cleanup
/// pending) > partial (role-scoped success, the other role preserved by design) /
/// stopped (full teardown, everything clean).
pub fn stop_phase(f: &StopFacts) -> &'static str {
    let full = f.is_full_teardown();
    // Orphaned: a real listener PID survived the teardown, or — on a full teardown —
    // the supervisor is still alive and will respawn what we just killed.
    if !f.canonical_listeners.is_empty() || (full && f.supervisor_healthy) {
        return "orphaned";
    }
    // Live listeners are gone. Still draining (kernel port TIME_WAIT / heartbeat TTL)
    // or cleanup not yet done?
    let draining = f.brain_port_bound
        || f.indexer_heartbeat_fresh
        || f.sockets_present
        || !f.writer_locks_held.is_empty();
    if draining {
        return "stopping";
    }
    // Fully clean. A role-scoped stop that left the other role alive by design is a
    // first-class success (PIL-AXO-004), reported distinctly from a full teardown.
    if full {
        "stopped"
    } else {
        "partial"
    }
}

/// The corrective action that closes an orphaned stop, or `None` when the stop
/// reached a terminal good state (stopped/partial) or is merely still draining.
pub fn stop_next_action(f: &StopFacts) -> Option<String> {
    if stop_phase(f) != "orphaned" {
        return None;
    }
    // Supervisor first: killing the listeners is futile while a live supervisor will
    // respawn them.
    if f.is_full_teardown() && f.supervisor_healthy {
        return Some(
            "supervisor still alive — it will respawn the role you killed. Reap the supervisor and re-run the teardown with --hard (`axonctl stop --hard`).".to_string(),
        );
    }
    if !f.canonical_listeners.is_empty() {
        let pids = f
            .canonical_listeners
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        return Some(format!(
            "listeners survived SIGTERM for role '{}' — kill them by PID and re-verify: `kill -9 {}`.",
            f.stop_role, pids
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-AXO-902590 — anti-dérive ENTRE DEUX LANGAGES.
    ///
    /// `scripts/lib/axon-promote-recovery.sh` classe chaque gate en trois : réparable
    /// par le seul indexeur · hors disponibilité courante · inconnu. La troisième
    /// classe escalade en `stop --hard`, qui coupe le service pour tous les tenants.
    ///
    /// Un gate ajouté ICI sans être classé LÀ-BAS tombe donc silencieusement dans
    /// « inconnu », et la première panne d'indexeur qui suit prend le brain avec elle.
    /// Le compilateur ne peut rien voir : les deux fichiers ne se connaissent pas. Ce
    /// test est le seul lien, et il rougit en NOMMANT le fichier à mettre à jour.
    ///
    /// Le contrôle porte sur les NOMS, pas sur les verdicts : c'est le nom qui est la
    /// clé de la classification côté shell.
    #[test]
    fn promote_status_n_emet_que_des_gates_classes_cote_shell() {
        // Les deux listes du shell, recopiées ici — c'est la recopie que ce test
        // EXISTE pour surveiller. Toute divergence doit rougir plutôt que de dormir.
        const REPARABLE_PAR_INDEXEUR: &[&str] = &["indexer_alive", "indexer_process_stable"];
        const EXIGE_REPRISE_COMPLETE: &[&str] = &["brain_serving", "brain_accept_queue"];
        const HORS_DISPONIBILITE: &[&str] = &[
            "last_promote_attempt",
            "qualification_passed",
            "manifest_runtime_match",
            "no_stale_pending",
        ];

        let mut emis: Vec<&str> = Vec::new();
        emis.extend(evaluate_gates(&ReleaseFacts::default()).iter().map(|g| g.name));
        emis.extend(
            evaluate_liveness_gates(&LivenessFacts::default())
                .iter()
                .map(|g| g.name),
        );
        emis.push(evaluate_attempt_gate(&ReleaseFacts::default()).name);
        emis.extend(
            evaluate_supervisor_gates(&SupervisorFacts::default())
                .iter()
                .map(|g| g.name),
        );

        let non_classes: Vec<&str> = emis
            .iter()
            .copied()
            .filter(|nom| {
                !REPARABLE_PAR_INDEXEUR.contains(nom)
                    && !EXIGE_REPRISE_COMPLETE.contains(nom)
                    && !HORS_DISPONIBILITE.contains(nom)
            })
            .collect();

        assert!(
            non_classes.is_empty(),
            "gate(s) émis par promote_status et NON classés dans \
             scripts/lib/axon-promote-recovery.sh : {non_classes:?}\n\
             Un gate non classé tombe dans « inconnu » et escalade en `stop --hard`, \
             qui COUPE LE BRAIN pour tous les tenants. Classez-le dans \
             AXON_PROMOTE_INDEXER_ONLY_GATES (un redémarrage du seul indexeur le \
             répare), AXON_PROMOTE_FULL_RESTART_GATES (il faut la reprise complète) \
             ou AXON_PROMOTE_NON_RUNTIME_GATES (aucun redémarrage ne le répare), \
             puis reportez le nom dans les constantes de CE test."
        );

        // Symétrique : un nom classé côté shell qui n'est plus émis est une entrée
        // morte. Moins grave — elle ne casse rien — mais elle fait croire à une
        // couverture qui n'existe plus, et c'est ainsi qu'une liste devient fausse.
        let orphelins: Vec<&str> = REPARABLE_PAR_INDEXEUR
            .iter()
            .chain(EXIGE_REPRISE_COMPLETE.iter())
            .chain(HORS_DISPONIBILITE.iter())
            .copied()
            .filter(|nom| !emis.contains(nom))
            .collect();
        assert!(
            orphelins.is_empty(),
            "nom(s) classés côté shell mais plus émis par promote_status : \
             {orphelins:?} — entrées mortes, à retirer des deux endroits"
        );
    }

    fn facts(live: &str, manifest: Option<&str>, pending: bool) -> ReleaseFacts {
        ReleaseFacts {
            live_build_id: live.to_string(),
            manifest_build_id: manifest.map(str::to_string),
            manifest_state: Some("promoted".to_string()),
            core_qualification_status: Some("passed".to_string()),
            core_qualification_evidence: Some("exit_code=0".to_string()),
            qualification_source: "current.promotion_gates",
            pending_present: pending,
            pending_build_id: if pending {
                Some("v0.0.0-staged".to_string())
            } else {
                None
            },
            runtime_contract: Some("brain_mcp_indexer_ist".to_string()),
            release_attempt_id: None,
            pending_release_attempt_id: None,
            artifact_sha256: None,
            attempt: None,
            ..Default::default()
        }
    }

    fn live(
        brain: bool,
        expected: bool,
        ready: bool,
        lifecycle: &str,
        source: &str,
    ) -> LivenessFacts {
        LivenessFacts {
            brain_serving: brain,
            indexer_expected: expected,
            indexer_ready: ready,
            indexer_lifecycle: lifecycle.to_string(),
            indexer_source: source.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn indexer_expected_from_runtime_contract() {
        let f = facts("v1", Some("v1"), false);
        assert!(f.indexer_expected()); // "brain_mcp_indexer_ist" contains "indexer"
        let mut g = f.clone();
        g.runtime_contract = Some("brain_only".to_string());
        assert!(!g.indexer_expected());
    }

    #[test]
    fn liveness_clean_when_brain_serves_and_indexer_fresh() {
        let l = live(true, true, true, "healthy", "pg_heartbeat");
        assert!(evaluate_liveness_gates(&l).iter().all(|g| g.passes()));
        assert!(liveness_phase(&l).is_none());
        assert!(liveness_next_action(&l).is_none());
    }

    #[test]
    fn brain_down_takes_precedence() {
        let l = live(false, true, true, "healthy", "pg_heartbeat");
        assert_eq!(liveness_phase(&l), Some("brain_down"));
        assert!(liveness_next_action(&l).unwrap().contains("DB probe"));
        assert!(evaluate_liveness_gates(&l)
            .iter()
            .any(|g| g.name == "brain_serving" && !g.passes()));
    }

    #[test]
    fn indexer_stale_vs_never_launched_actions_differ() {
        let stale = live(
            true,
            true,
            false,
            "crashed_or_abandoned",
            "pg_heartbeat_stale",
        );
        assert_eq!(liveness_phase(&stale), Some("indexer_down"));
        assert!(liveness_next_action(&stale).unwrap().contains("restart"));
        let never = live(true, true, false, "never_launched", "no_heartbeat");
        assert!(liveness_next_action(&never)
            .unwrap()
            .contains("start the full runtime"));
    }

    #[test]
    fn indexer_gate_na_when_not_expected() {
        // brain-only contract: a missing indexer is not a failure.
        let l = live(true, false, false, "never_launched", "no_heartbeat");
        assert!(evaluate_liveness_gates(&l).iter().all(|g| g.passes()));
        assert!(liveness_phase(&l).is_none());
    }

    #[test]
    fn clean_when_manifest_matches_and_no_pending() {
        let f = facts("v1-gabc", Some("v1-gabc"), false);
        assert_eq!(phase(&f), "clean");
        assert!(next_action(&f).is_none());
        assert!(evaluate_gates(&f).iter().all(|g| g.passes()));
    }

    #[test]
    fn drift_when_running_differs_from_manifest() {
        let f = facts("v2-gnew", Some("v1-gold"), false);
        assert_eq!(phase(&f), "drift");
        assert!(next_action(&f).unwrap().contains("Re-promote"));
        let gates = evaluate_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "manifest_runtime_match" && !g.passes()));
    }

    #[test]
    fn staged_when_pending_present() {
        // The session-91 stranded-pending failure.
        let f = facts("v1-gabc", Some("v1-gabc"), true);
        assert_eq!(phase(&f), "staged");
        let gates = evaluate_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "no_stale_pending" && !g.passes()));
        assert!(next_action(&f).unwrap().contains("resume"));
    }

    #[test]
    fn release_facts_collect_reads_current_and_pending() {
        // REQ-AXO-902190 — cover ReleaseFacts::collect, a top untested HUB surfaced by
        // structural_health_worklist (449 callers, tested=false). It reads current.json +
        // pending.json from the release dir. Tempdir, no runtime — the SHI remediation loop
        // in action: worklist named it → test it → the indexer flips `tested` → ΔSHI.
        use std::fs;
        let dir = std::env::temp_dir().join(format!("axon-relfacts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("current.json"),
            // REQ-AXO-902585 — la forme REELLE d'un manifeste : `promotion_gates`
            // porte le verdict, et `qualification` ne porte que la provenance de
            // build. L'ancienne fixture inventait `qualification.verdict`, une cle
            // qu'aucun ecrivain du depot ne produit — elle validait une fiction.
            r#"{"release_attempt_id":"attempt-current","runtime_version":{"build_id":"v-old"},"state":"promoted","qualification":{"evidence":[]},"promotion_gates":{"core_qualification":{"status":"passed","evidence":"exit_code=0"}},"runtime_contract":"brain_mcp_indexer_ist","artifact":{"sha256":"abc123"}}"#,
        )
        .unwrap();

        let f = ReleaseFacts::collect(&dir, "v-running".to_string());
        assert_eq!(f.live_build_id, "v-running");
        assert_eq!(f.manifest_build_id.as_deref(), Some("v-old"));
        assert_eq!(f.manifest_state.as_deref(), Some("promoted"));
        assert_eq!(f.core_qualification_status.as_deref(), Some("passed"));
        assert_eq!(f.qualification_source, "current.promotion_gates");
        assert_eq!(f.qualification_status(), GateStatus::Pass);
        assert!(!f.pending_present);
        assert!(f.indexer_expected()); // "brain_mcp_indexer_ist" names an indexer
        assert_eq!(f.release_attempt_id.as_deref(), Some("attempt-current"));
        assert_eq!(f.artifact_sha256.as_deref(), Some("abc123"));

        // A stranded/mid-flight staging: pending.json present with its own build_id.
        fs::write(
            dir.join("pending.json"),
            r#"{"release_attempt_id":"attempt-pending","runtime_version":{"build_id":"v-staged"}}"#,
        )
        .unwrap();
        let f2 = ReleaseFacts::collect(&dir, "v-running".to_string());
        assert!(f2.pending_present);
        assert_eq!(f2.pending_build_id.as_deref(), Some("v-staged"));
        assert_eq!(
            f2.pending_release_attempt_id.as_deref(),
            Some("attempt-pending")
        );

        fs::write(
            dir.join("attempt-current.json"),
            r#"{"release_attempt_id":"attempt-pending","phase":"qualify","status":"running","last_event":"step_started"}"#,
        )
        .unwrap();
        let f3 = ReleaseFacts::collect(&dir, "v-running".to_string());
        assert_eq!(
            f3.attempt.as_ref().and_then(|v| v["phase"].as_str()),
            Some("qualify")
        );

        // Absent manifest → all-None, indexer not expected (safe default).
        let empty =
            std::env::temp_dir().join(format!("axon-relfacts-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&empty);
        fs::create_dir_all(&empty).unwrap();
        let f3 = ReleaseFacts::collect(&empty, "v-x".to_string());
        assert_eq!(f3.manifest_build_id, None);
        assert!(!f3.indexer_expected());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&empty);
    }

    /// REQ-AXO-902585 — la porte lisait `qualification.verdict`, une clé qu'AUCUN
    /// écrivain du dépôt ne produit : 0 manifeste sur 244 en porte une. Le vrai
    /// verdict, écrit par `axonctl cutover --phase record-gate` et rendu bloquant
    /// par le `finalize`, vit sous `promotion_gates.core_qualification.status`.
    /// Deux mécanismes portaient par accident le même mot.
    #[test]
    fn qualification_gate_reads_promotion_gates_not_build_provenance() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "axon-req902585-a-{}",
            crate::clock::now_unix_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        // La forme RÉELLE : les deux clés coexistent, et seule la seconde compte.
        fs::write(
            dir.join("current.json"),
            r#"{"runtime_version":{"build_id":"v1"},"state":"promoted","qualification":{"evidence":[]},"promotion_gates":{"core_qualification":{"status":"failed","evidence":"exit_code=65"}}}"#,
        )
        .unwrap();
        let f = ReleaseFacts::collect(&dir, "v1".to_string());
        assert_eq!(
            f.qualification_status(),
            GateStatus::Fail,
            "un verdict rouge doit rendre la porte rouge : {:?}",
            f.core_qualification_status
        );
        let gates = evaluate_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "qualification_passed" && g.is_red()));
        let _ = fs::remove_dir_all(&dir);
    }

    /// L'absence de preuve n'est pas une preuve : 243 des 244 manifestes
    /// d'historique n'ont pas de `promotion_gates`. Ils doivent rendre `unknown`,
    /// jamais `pass` — c'est la fin d'un faux vert, pas une régression.
    #[test]
    fn a_manifest_without_promotion_gates_is_unknown_not_green() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "axon-req902585-b-{}",
            crate::clock::now_unix_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("current.json"),
            r#"{"runtime_version":{"build_id":"v1"},"state":"promoted","qualification":{"evidence":[]}}"#,
        )
        .unwrap();
        let f = ReleaseFacts::collect(&dir, "v1".to_string());
        assert_eq!(f.qualification_source, "absent");
        assert_eq!(f.qualification_status(), GateStatus::Unknown);
        let gate = evaluate_gates(&f)
            .into_iter()
            .find(|g| g.name == "qualification_passed")
            .expect("gate présent");
        assert!(!gate.passes(), "ne PAS rendre `pass` faute de preuve");
        assert!(
            !gate.is_red(),
            "et ne PAS rendre rouge non plus : un Unknown dans `failed_gates` \
             ferait couper le brain à chaque promote (promote_live_safe.sh)"
        );
    }

    /// Un staging en vol POSSÈDE la question : `current.promotion_gates` est
    /// tautologiquement tout-vert (le `finalize` refuse de basculer sinon), donc
    /// c'est `pending` qui porte le verdict intéressant.
    #[test]
    fn an_in_flight_staging_answers_the_qualification_question() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "axon-req902585-c-{}",
            crate::clock::now_unix_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("current.json"),
            r#"{"runtime_version":{"build_id":"v1"},"state":"promoted","promotion_gates":{"core_qualification":{"status":"passed","evidence":"ancien"}}}"#,
        )
        .unwrap();
        fs::write(
            dir.join("pending.json"),
            r#"{"runtime_version":{"build_id":"v2"},"state":"prepared","promotion_gates":{"core_qualification":{"status":"failed","evidence":"exit_code=1"}}}"#,
        )
        .unwrap();
        let f = ReleaseFacts::collect(&dir, "v1".to_string());
        assert_eq!(f.qualification_source, "pending.promotion_gates");
        assert_eq!(
            f.qualification_status(),
            GateStatus::Fail,
            "le rouge en vol ne doit pas être masqué par le vert déjà promu"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// REQ-AXO-902585 (défaut 2) — `phase: "clean"` est vrai au contrat et taisait
    /// pourtant qu'une promotion venait d'échouer. Mesuré deux fois le 2026-09-01 :
    /// après un `cutover_finalize` refusé puis un `qualify_mcp` en warn, l'outil
    /// rendait `phase: clean`, `failed_gates: []`, `next_action: null` — « rien à
    /// faire » — alors que le changement qu'on voulait livrer n'était pas en vigueur.
    #[test]
    fn a_failed_last_attempt_is_not_silent_when_the_release_is_clean() {
        let mut f = facts("v1", Some("v1"), false);
        f.release_attempt_id = Some("attempt-QUI-A-PROMU".to_string());
        f.attempt_id = Some("attempt-QUI-A-ECHOUE".to_string());
        f.attempt_status = Some("failed".to_string());
        f.attempt_phase = Some("qualify_mcp".to_string());
        f.attempt_last_event_detail = Some("exit_code=1".to_string());
        f.attempt_journal_path = Some("/tmp/attempts/x.jsonl".to_string());

        assert_eq!(phase(&f), "clean", "la phase reste vraie AU CONTRAT");
        let action = attempt_next_action(&f).expect("l'échec ne doit plus être muet");
        assert!(
            action.contains("attempt-QUI-A-ECHOUE") && action.contains("qualify_mcp"),
            "l'action doit nommer la tentative et l'étape : {action}"
        );
        assert!(
            action.contains("/tmp/attempts/x.jsonl"),
            "et pointer le journal, pour ne pas le chercher à la main : {action}"
        );
        assert!(evaluate_attempt_gate(&f).is_red());
    }

    /// REQ-AXO-902628 — LE test du défaut mesuré : un promote tué en plein build
    /// se journalise `completed` / `rc=0`, et cette porte le rendait `pass`.
    ///
    /// Cas réel, verbatim : `20260906T135356Z-873834-0b5c9f57af94.jsonl`, une
    /// seule étape démarrée sur quinze, aucune terminée, `lease_released final
    /// completed "promotion process exited with rc=0"`. Rien n'avait été promu.
    /// Sept journaux sur les 84 archivés portent cette signature.
    #[test]
    fn un_completed_SANS_preuve_de_cutover_n_est_plus_un_succes() {
        let mut f = facts("v1", Some("v1"), false);
        f.attempt_id = Some("20260906T135356Z-873834-0b5c9f57af94".to_string());
        f.attempt_status = Some("completed".to_string());
        f.attempt_last_event_detail = Some("promotion process exited with rc=0".to_string());
        f.attempt_journal_path = Some("/tmp/attempts/tue.jsonl".to_string());

        let gate = evaluate_attempt_gate(&f);
        assert!(
            !gate.passes(),
            "un `completed` sans marque de cutover ne prouve RIEN : {gate:?}"
        );
        assert!(
            !gate.is_red(),
            "et il n'est pas rouge non plus — un rouge inattendu fait escalader \
             `promote_live_safe.sh` en redémarrage complet du brain : {gate:?}"
        );
        assert!(
            gate.detail.contains("20260906T135356Z"),
            "le verdict doit NOMMER la tentative : {}",
            gate.detail
        );
        assert!(
            gate.detail.contains("/tmp/attempts/tue.jsonl"),
            "et pointer le journal, sinon on le cherche à la main : {}",
            gate.detail
        );
    }

    /// L'autre moitié : un `completed` qui PORTE la preuve reste vert. Sans ce
    /// test, rendre la porte muette suffirait à faire passer le précédent — et
    /// on aurait échangé un faux positif contre un faux négatif.
    #[test]
    fn un_completed_AVEC_la_preuve_de_cutover_reste_vert() {
        let mut f = facts("v1", Some("v1"), false);
        f.attempt_id = Some("20260907T095139Z-657299-2508214063a9".to_string());
        f.attempt_status = Some("completed".to_string());
        f.attempt_last_event_detail = Some(
            "promotion process exited with rc=0; complete:15 steps, cutover_finalize passed"
                .to_string(),
        );
        let gate = evaluate_attempt_gate(&f);
        assert!(gate.passes(), "la preuve est là, la porte doit passer : {gate:?}");
        assert_eq!(attempt_next_action(&f), None, "rien à conseiller sur un succès prouvé");
    }

    /// REQ-AXO-902628 — le statut que l'écrivain pose désormais quand il sort
    /// proprement sans pouvoir prouver la complétion.
    #[test]
    fn un_attempt_incomplete_est_dit_ET_conseille() {
        let mut f = facts("v1", Some("v1"), false);
        f.release_attempt_id = Some("attempt-QUI-A-PROMU".to_string());
        f.attempt_id = Some("attempt-TUE".to_string());
        f.attempt_status = Some("incomplete".to_string());
        f.attempt_phase = Some("build".to_string());
        f.attempt_last_event_detail =
            Some("step(s) started but never completed: build (0 step(s) done)".to_string());
        f.attempt_journal_path = Some("/tmp/attempts/tue.jsonl".to_string());

        let gate = evaluate_attempt_gate(&f);
        assert!(!gate.passes() && !gate.is_red(), "ni preuve, ni alarme : {gate:?}");
        assert!(
            gate.detail.contains("INCOMPLETE"),
            "le mot doit y être, l'opérateur lit le texte : {}",
            gate.detail
        );

        // La moitié symétrique : le conseil de reprise était supprimé pour tout
        // ce qui n'était pas `failed`. Corriger la porte sans le conseil aurait
        // laissé l'opérateur sans verdict ET sans geste.
        let action = attempt_next_action(&f).expect("un incomplet doit être conseillé");
        assert!(action.contains("attempt-TUE"), "{action}");
        assert!(
            action.contains("INCOMPLETE") && !action.contains("FAILED"),
            "dire `FAILED` sur un incomplet serait un verdict faux dans l'autre \
             sens — rien n'a échoué, le script est mort sans le dire : {action}"
        );
    }

    /// La règle de preuve, sur ses deux moitiés, prise en entrée directe.
    #[test]
    #[allow(non_snake_case)]
    fn MUTANT_la_preuve_de_completion_exige_le_statut_ET_la_marque() {
        const MARQUE: &str = "complete:15 steps, cutover_finalize passed";
        assert!(completion_est_prouvee(Some("completed"), Some(MARQUE)));
        // Le statut seul ne suffit pas — c'est le défaut d'origine.
        assert!(!completion_est_prouvee(Some("completed"), Some("exited with rc=0")));
        assert!(!completion_est_prouvee(Some("completed"), None));
        // La marque seule ne suffit pas non plus : un `failed` qui aurait franchi
        // le cutover puis échoué après reste un échec.
        assert!(!completion_est_prouvee(Some("failed"), Some(MARQUE)));
        assert!(!completion_est_prouvee(Some("incomplete"), Some(MARQUE)));
        assert!(!completion_est_prouvee(None, Some(MARQUE)));
    }

    /// REQ-AXO-902628 — le verdict doit être RÉTROACTIF, et sans ce test il ne
    /// l'était pas.
    ///
    /// Cas RÉEL au moment de l'écriture : `attempt-current.json` porte
    /// `status: completed` / `last_event_detail: "promotion process exited with
    /// rc=0"` — projection écrite AVANT l'écrivain corrigé, donc sans la marque.
    /// Le promote, lui, est bel et bien complet (15 étapes, cutover franchi).
    /// Juger sur la seule marque rendait `unknown` sur un promote sain : le faux
    /// négatif symétrique du faux positif que ce REQ ferme. Une porte qui se
    /// trompe dans les DEUX sens n'informe plus personne.
    ///
    /// La correction : la collecte OUVRE le journal dont elle avait déjà le
    /// chemin. Les 84 journaux archivés sont jugés sur leur contenu, pas sur le
    /// résumé qu'une ancienne version du script en avait fait.
    #[test]
    fn le_gate_relit_le_JOURNAL_donc_le_verdict_est_RETROACTIF() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "axon-902628-retro-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("complet.jsonl");
        let mut lignes = String::new();
        for etape in ["build", "qualify", "cutover_finalize"] {
            lignes.push_str(&format!(
                "{{\"event\":\"step_started\",\"phase\":\"{etape}\"}}\n\
                 {{\"event\":\"step_completed\",\"phase\":\"{etape}\"}}\n"
            ));
        }
        fs::write(&journal, &lignes).unwrap();
        fs::write(
            dir.join("attempt-current.json"),
            format!(
                r#"{{"release_attempt_id":"20260907T095139Z-657299-2508214063a9","status":"completed","phase":"final","last_event_detail":"promotion process exited with rc=0","journal_path":"{}"}}"#,
                journal.display()
            ),
        )
        .unwrap();

        let f = ReleaseFacts::collect(&dir, "v1".to_string());
        assert!(
            f.attempt_completion_evidence
                .as_deref()
                .is_some_and(|p| p.starts_with("complete:")),
            "la collecte doit LIRE le journal : {:?}",
            f.attempt_completion_evidence
        );
        let gate = evaluate_attempt_gate(&f);
        assert!(
            gate.passes(),
            "le journal prouve le cutover, la porte doit passer même sans la \
             marque dans la projection : {gate:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// L'autre moitié, et elle est indispensable : sans elle, une collecte qui
    /// écrirait « complete: » sur tout ferait passer le test précédent seule.
    #[test]
    fn un_journal_qui_DEMENT_la_projection_rend_unknown() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "axon-902628-dement-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("tue.jsonl");
        // Le cas mesuré : une étape démarrée sur quinze, aucune terminée.
        fs::write(&journal, "{\"event\":\"step_started\",\"phase\":\"build\"}\n").unwrap();
        fs::write(
            dir.join("attempt-current.json"),
            format!(
                r#"{{"release_attempt_id":"20260906T135356Z-873834-0b5c9f57af94","status":"completed","phase":"final","last_event_detail":"promotion process exited with rc=0","journal_path":"{}"}}"#,
                journal.display()
            ),
        )
        .unwrap();

        let f = ReleaseFacts::collect(&dir, "v1".to_string());
        let gate = evaluate_attempt_gate(&f);
        assert!(!gate.passes(), "le journal DÉMENT la projection : {gate:?}");
        assert!(
            !gate.is_red(),
            "et il n'est pas rouge — un rouge inattendu fait escalader \
             `promote_live_safe.sh` en redémarrage complet du brain : {gate:?}"
        );
        assert!(
            gate.detail.contains("build"),
            "l'étape restée ouverte doit être NOMMÉE : {}",
            gate.detail
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Le juge Rust lui-même, sur ses deux moitiés. Le prédicat n'a AUCUNE
    /// constante de compte : c'est ce qui lui permet d'accepter un promote
    /// légitimement raccourci (14 étapes) sans accepter un promote mort.
    #[test]
    #[allow(non_snake_case)]
    fn MUTANT_le_juge_Rust_refuse_les_deux_moities() {
        let complet = "{\"event\":\"step_started\",\"phase\":\"cutover_finalize\"}\n\
                       {\"event\":\"step_completed\",\"phase\":\"cutover_finalize\"}\n";
        assert!(preuve_de_completion(complet).starts_with("complete:"));
        // Moitié 1 — une étape ouverte.
        let orpheline = format!("{complet}{{\"event\":\"step_started\",\"phase\":\"post\"}}\n");
        let v = preuve_de_completion(&orpheline);
        assert!(v.starts_with("incomplete:") && v.contains("post"), "{v}");
        // Moitié 2 — tout fermé, mais le cutover jamais atteint.
        let sans_cutover = "{\"event\":\"step_started\",\"phase\":\"build\"}\n\
                            {\"event\":\"step_completed\",\"phase\":\"build\"}\n";
        let v = preuve_de_completion(sans_cutover);
        assert!(v.starts_with("incomplete:") && v.contains("cutover_finalize"), "{v}");
        // Une ligne illisible n'est pas une preuve d'incomplétude.
        assert!(preuve_de_completion(&format!("pas du json\n{complet}")).starts_with("complete:"));
        // Et jamais muet.
        assert!(preuve_de_completion("").starts_with("incomplete:"));
    }

    /// `Indisponible` n'est pas `Incomplete` — confondre les deux rendrait
    /// `unknown` sur un promote sain dont le journal a simplement été archivé.
    #[test]
    fn un_journal_ILLISIBLE_laisse_la_marque_temoigner() {
        let mut f = facts("v1", Some("v1"), false);
        f.attempt_id = Some("attempt-archivé".to_string());
        f.attempt_status = Some("completed".to_string());
        f.attempt_journal_path = Some("/ce/chemin/n/existe/pas.jsonl".to_string());
        f.attempt_completion_evidence =
            Some(format!("{PREFIXE_JOURNAL_ILLISIBLE} (/ce/chemin/n/existe/pas.jsonl) — nope"));

        // Sans la marque : rien ne prouve, donc `unknown`.
        f.attempt_last_event_detail = Some("promotion process exited with rc=0".to_string());
        let gate = evaluate_attempt_gate(&f);
        assert!(!gate.passes() && !gate.is_red(), "{gate:?}");

        // Avec la marque : le témoin restant suffit.
        f.attempt_last_event_detail = Some(format!(
            "promotion process exited with rc=0; complete:15 steps, {MARQUE_DE_COMPLETION}"
        ));
        assert!(evaluate_attempt_gate(&f).passes());
    }

    /// Un promote EN COURS n'est pas un échec. Cette distinction n'est pas
    /// cosmétique : pendant un promote, `attempt-current.status` vaut « running » et
    /// le script relit `promote_status` en boucle — un rouge ici le ferait basculer
    /// en redémarrage complet du brain, EN PLEIN VOL.
    #[test]
    fn a_running_attempt_is_unknown_not_failed() {
        let mut f = facts("v1", Some("v1"), false);
        f.attempt_status = Some("running".to_string());
        let gate = evaluate_attempt_gate(&f);
        assert!(!gate.is_red(), "jamais rouge pendant un promote : {gate:?}");
        assert!(!gate.passes(), "et pas vert non plus : rien n'est encore su");
        assert_eq!(attempt_next_action(&f), None);
    }

    /// Le manifeste servi a été produit par la tentative qui a ensuite échoué :
    /// autre conseil, autre phrase.
    #[test]
    fn a_failure_after_finalisation_says_so_explicitly() {
        let mut f = facts("v1", Some("v1"), false);
        f.release_attempt_id = Some("attempt-MEME".to_string());
        f.attempt_id = Some("attempt-MEME".to_string());
        f.attempt_status = Some("failed".to_string());
        f.attempt_phase = Some("cutover_finalize".to_string());
        let action = attempt_next_action(&f).expect("action attendue");
        assert!(
            action.contains("that same attempt then"),
            "le cas « échec APRÈS finalisation » doit être dit à part : {action}"
        );
    }

    /// REQ-AXO-902585 (défaut 3) — LE test qui nomme le défaut mesuré : la liveness
    /// PASSE (battement frais) pendant que le superviseur relance en boucle.
    #[test]
    fn a_restart_loop_is_caught_even_when_the_heartbeat_looks_healthy() {
        let l = live(true, true, true, "healthy", "pg_heartbeat");
        assert!(
            evaluate_liveness_gates(&l).iter().all(|g| g.passes()),
            "le battement PG est frais : `indexer_alive` PASSE, et c'est correct"
        );
        // Et pourtant, les valeurs exactes de l'incident du 2026-09-01 :
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Restarting".to_string(),
            restarts: 13,
            pid: 527117,
            age_ms: 8_000,
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s)
            .into_iter()
            .next()
            .expect("un gate");
        assert!(
            gate.is_red(),
            "le superviseur dit `Restarting` : c'est une panne, pas une santé : {gate:?}"
        );
    }

    #[test]
    fn an_unreachable_supervisor_is_unknown_never_green() {
        let s = SupervisorFacts {
            reachable: false,
            error: Some("connect 127.0.0.1:8080: refused".to_string()),
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s).into_iter().next().unwrap();
        assert!(!gate.passes(), "une sonde muette n'est jamais un feu vert");
        assert!(!gate.is_red(), "et pas un rouge non plus : on ne sait pas");
        assert!(
            gate.detail.contains("refused"),
            "la cause remonte telle quelle : {}",
            gate.detail
        );
    }

    /// Un redémarrage DÉLIBÉRÉ n'est pas une boucle — et c'est ce qui empêche
    /// l'outil de crier « boucle » sur le remède qu'il vient lui-même de conseiller.
    #[test]
    fn an_intentional_single_restart_is_not_a_loop() {
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Running".to_string(),
            restarts: 1,
            age_ms: 5_000,
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s).into_iter().next().unwrap();
        assert!(!gate.is_red() && !gate.passes(), "ni l'un ni l'autre : {gate:?}");
    }

    /// Beaucoup de redémarrages MAIS un processus qui tient depuis une heure : ce
    /// n'est plus une boucle, c'est une histoire.
    #[test]
    fn many_restarts_on_an_old_process_is_a_pass() {
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Running".to_string(),
            restarts: 29,
            age_ms: 3_600_000,
            ..Default::default()
        };
        assert!(evaluate_supervisor_gates(&s)[0].passes());
    }

    /// Aucune projection sur disque : on ne sait rien, et on le dit.
    #[test]
    fn an_absent_attempt_projection_is_unknown_not_green() {
        let f = facts("v1", Some("v1"), false);
        let gate = evaluate_attempt_gate(&f);
        assert!(!gate.passes() && !gate.is_red());
        assert_eq!(attempt_next_action(&f), None);
    }

    /// « Sauté » n'est pas « passé ».
    #[test]
    fn a_skipped_qualification_is_not_a_pass() {
        let mut f = facts("v1", Some("v1"), false);
        f.core_qualification_status = Some("skipped".to_string());
        assert_eq!(f.qualification_status(), GateStatus::Unknown);
        let gate = evaluate_gates(&f)
            .into_iter()
            .find(|g| g.name == "qualification_passed")
            .expect("gate présent");
        assert!(!gate.passes() && !gate.is_red());
    }

    #[test]
    fn failed_qualification_fails_only_that_gate() {
        let mut f = facts("v1-gabc", Some("v1-gabc"), false);
        f.core_qualification_status = Some("failed".to_string());
        let gates = evaluate_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "qualification_passed" && !g.passes()));
        assert!(gates
            .iter()
            .any(|g| g.name == "manifest_runtime_match" && g.passes()));
    }

    // --- Cutover FSM ------------------------------------------------------

    #[test]
    fn cutover_healthy_new_finalizes() {
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(true),
            deadline_exceeded: false,
            old_restored: false,
        };
        assert!(f.new_healthy());
        assert_eq!(cutover_phase(&f), "healthy");
        assert!(evaluate_cutover_gates(&f).iter().all(|g| g.passes()));
        assert!(cutover_next_action(&f).is_none());
    }

    #[test]
    fn cutover_awaits_while_new_converging() {
        let f = CutoverFacts {
            new_liveness: live(true, true, false, "never_launched", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: false,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "awaiting_health");
        assert!(cutover_next_action(&f).unwrap().contains("poll"));
    }

    #[test]
    fn cutover_rolls_back_when_deadline_exceeded_unhealthy() {
        // THE s94 failure mode: the new runtime never becomes healthy. Must AUTO-ROLLBACK,
        // never strand the live in an outage with a half-finalized manifest.
        let f = CutoverFacts {
            new_liveness: live(false, true, false, "crashed_or_abandoned", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: true,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "rolling_back");
        assert!(evaluate_cutover_gates(&f)
            .iter()
            .any(|g| g.name == "new_runtime_healthy" && !g.passes()));
        assert!(cutover_next_action(&f).unwrap().contains("AUTO-ROLLBACK"));
    }

    #[test]
    fn cutover_rolled_back_after_restore() {
        let f = CutoverFacts {
            new_liveness: live(false, true, false, "crashed_or_abandoned", "no_heartbeat"),
            new_qualify_ok: None,
            deadline_exceeded: true,
            old_restored: true,
        };
        assert_eq!(cutover_phase(&f), "rolled_back");
        assert!(cutover_next_action(&f)
            .unwrap()
            .contains("previous release is serving"));
    }

    #[test]
    fn cutover_healthy_wins_even_at_deadline() {
        // Went healthy right as the deadline passed → finalize, do NOT roll back.
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(true),
            deadline_exceeded: true,
            old_restored: false,
        };
        assert_eq!(cutover_phase(&f), "healthy");
    }

    #[test]
    fn cutover_failed_qualify_blocks_health_even_when_live() {
        // brain+indexer live but qualify FAILED → not healthy → rollback on deadline.
        let f = CutoverFacts {
            new_liveness: live(true, true, true, "healthy", "pg_heartbeat"),
            new_qualify_ok: Some(false),
            deadline_exceeded: true,
            old_restored: false,
        };
        assert!(!f.new_healthy());
        assert_eq!(cutover_phase(&f), "rolling_back");
    }

    #[test]
    fn cutover_loop_promotes_on_first_healthy_poll() {
        let mut polls = 0;
        let out = run_cutover_loop(
            || {
                polls += 1;
                true
            },
            10,
            || {},
        );
        assert_eq!(out, CutoverOutcome::Promoted);
        assert_eq!(polls, 1, "should stop probing the instant it is healthy");
    }

    #[test]
    fn cutover_loop_rolls_back_when_never_healthy() {
        // THE incident guard: an unhealthy new runtime must roll back after the deadline,
        // never hang or strand.
        let mut waits = 0;
        let out = run_cutover_loop(|| false, 5, || waits += 1);
        assert_eq!(out, CutoverOutcome::RolledBack);
        assert_eq!(waits, 5);
    }

    #[test]
    fn cutover_loop_promotes_when_healthy_on_third_poll() {
        let mut n = 0;
        let out = run_cutover_loop(
            || {
                n += 1;
                n >= 3
            },
            10,
            || {},
        );
        assert_eq!(out, CutoverOutcome::Promoted);
        assert_eq!(n, 3);
    }

    // --- Cutover CHOREOGRAPHY (drive_cutover) -----------------------------

    /// Records the ordered I/O steps a cutover performed, with a scripted failure at a
    /// chosen step, so `drive_cutover`'s sequencing + rollback decisions are asserted
    /// without a runtime. `fail_at` names the step whose call returns `Err`.
    #[derive(Default)]
    struct FakeIo {
        calls: Vec<&'static str>,
        fail_at: Option<&'static str>,
        rollback_fails: bool,
    }

    impl FakeIo {
        fn failing(step: &'static str) -> Self {
            FakeIo {
                fail_at: Some(step),
                ..Default::default()
            }
        }
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                Err(format!("scripted failure at {name}"))
            } else {
                Ok(())
            }
        }
    }

    impl CutoverIo for FakeIo {
        fn snapshot_current(&mut self) -> Result<(), String> {
            self.step("snapshot_current")
        }
        fn stage_candidate(&mut self) -> Result<(), String> {
            self.step("stage_candidate")
        }
        fn restart_runtime(&mut self) -> Result<(), String> {
            self.step("restart_runtime")
        }
        fn finalize(&mut self) -> Result<(), String> {
            self.step("finalize")
        }
        fn rollback(&mut self) -> Result<(), String> {
            self.calls.push("rollback");
            if self.rollback_fails {
                Err("scripted rollback failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn cutover_healthy_candidate_finalizes_never_rolls_back() {
        let mut io = FakeIo::default();
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert_eq!(verdict, CutoverVerdict::Promoted);
        // The happy path: snapshot → stage → restart → finalize, and NO rollback.
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "finalize"
            ]
        );
        assert!(!io.calls.contains(&"rollback"));
    }

    #[test]
    fn cutover_unhealthy_candidate_auto_rolls_back() {
        // THE s94 incident guard: a candidate that never goes healthy MUST restore the
        // old release (rollback) and NEVER finalize a half-promoted manifest.
        let mut io = FakeIo::default();
        let verdict = drive_cutover(&mut io, || false, 3, || {});
        assert_eq!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "health_gate",
                rollback_ok: true,
                detail: Some("new runtime never healthy within the deadline".to_string()),
            }
        );
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "rollback"
            ]
        );
        assert!(
            !io.calls.contains(&"finalize"),
            "must NOT finalize a bad candidate"
        );
    }

    #[test]
    fn cutover_snapshot_failure_aborts_before_touching_anything() {
        // Cannot capture a rollback target → do NOT stage/restart. Old release intact,
        // no rollback attempted (nothing was mutated).
        let mut io = FakeIo::failing("snapshot_current");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        match verdict {
            CutoverVerdict::RolledBack {
                failed_step,
                rollback_ok,
                ..
            } => {
                assert_eq!(failed_step, "snapshot_current");
                assert!(rollback_ok, "nothing mutated → old release still serves");
            }
            other => panic!("expected RolledBack, got {other:?}"),
        }
        assert_eq!(io.calls, vec!["snapshot_current"]);
        assert!(!io.calls.contains(&"stage_candidate"));
        assert!(!io.calls.contains(&"rollback"));
    }

    #[test]
    fn cutover_stage_failure_rolls_back_without_restart() {
        let mut io = FakeIo::failing("stage_candidate");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "stage_candidate",
                rollback_ok: true,
                ..
            }
        ));
        assert_eq!(
            io.calls,
            vec!["snapshot_current", "stage_candidate", "rollback"]
        );
        assert!(!io.calls.contains(&"restart_runtime"));
    }

    #[test]
    fn cutover_restart_failure_rolls_back() {
        let mut io = FakeIo::failing("restart_runtime");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "restart_runtime",
                ..
            }
        ));
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "rollback"
            ]
        );
    }

    #[test]
    fn cutover_finalize_failure_rolls_back_to_coherent_old_release() {
        // Healthy candidate but the manifest finalize failed: roll back rather than
        // leave bin/* ↔ current.json drift (the s91 stranded-pending class).
        let mut io = FakeIo::failing("finalize");
        let verdict = drive_cutover(&mut io, || true, 5, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "finalize",
                ..
            }
        ));
        assert_eq!(
            io.calls,
            vec![
                "snapshot_current",
                "stage_candidate",
                "restart_runtime",
                "finalize",
                "rollback"
            ]
        );
    }

    #[test]
    fn cutover_failed_rollback_is_surfaced_distinctly() {
        // A rollback that ALSO fails = a genuine outage; `rollback_ok:false` lets the
        // caller escalate (operator action) instead of reporting a clean auto-recovery.
        let mut io = FakeIo {
            rollback_fails: true,
            ..Default::default()
        };
        let verdict = drive_cutover(&mut io, || false, 2, || {});
        assert!(matches!(
            verdict,
            CutoverVerdict::RolledBack {
                failed_step: "health_gate",
                rollback_ok: false,
                ..
            }
        ));
    }

    #[test]
    fn cutover_healthy_on_second_poll_finalizes() {
        let mut n = 0;
        let mut waits = 0;
        let mut io = FakeIo::default();
        let verdict = drive_cutover(
            &mut io,
            || {
                n += 1;
                n >= 2
            },
            5,
            || waits += 1,
        );
        assert_eq!(verdict, CutoverVerdict::Promoted);
        assert_eq!(n, 2);
        assert_eq!(
            waits, 1,
            "one wait between the failed first poll and the healthy second"
        );
    }

    // --- Stop FSM ---------------------------------------------------------

    /// A fully clean full teardown: nothing left to do.
    fn stop_clean_all() -> StopFacts {
        StopFacts {
            stop_role: "all".to_string(),
            canonical_listeners: vec![],
            brain_port_bound: false,
            supervisor_healthy: false,
            writer_locks_held: vec![],
            sockets_present: false,
            indexer_heartbeat_fresh: false,
        }
    }

    #[test]
    fn stop_clean_full_teardown_is_stopped() {
        let f = stop_clean_all();
        assert_eq!(stop_phase(&f), "stopped");
        assert!(evaluate_stop_gates(&f).iter().all(|g| g.passes()));
        assert!(stop_next_action(&f).is_none());
    }

    #[test]
    fn stop_orphaned_when_supervisor_alive_on_full_teardown() {
        let mut f = stop_clean_all();
        f.supervisor_healthy = true;
        assert_eq!(stop_phase(&f), "orphaned");
        let gates = evaluate_stop_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "supervisor_quiesced" && !g.passes()));
        // Supervisor takes priority: the action is reap + --hard, not kill-by-pid.
        let action = stop_next_action(&f).unwrap();
        assert!(action.contains("--hard"));
        assert!(action.contains("supervisor"));
    }

    #[test]
    fn stop_orphaned_when_listeners_survive() {
        let mut f = stop_clean_all();
        f.canonical_listeners = vec![4242, 4243];
        assert_eq!(stop_phase(&f), "orphaned");
        let gates = evaluate_stop_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "no_canonical_listeners" && !g.passes()));
        let action = stop_next_action(&f).unwrap();
        assert!(action.contains("kill -9 4242 4243"));
    }

    #[test]
    fn stop_partial_when_role_scoped_supervisor_is_na() {
        // Role-scoped stop of the indexer: the supervisor stays up for the brain by
        // design (PIL-AXO-004), so supervisor_quiesced is N/A and the verdict is a
        // first-class success (partial), NOT orphaned.
        let mut f = stop_clean_all();
        f.stop_role = "indexer".to_string();
        f.supervisor_healthy = true;
        assert_eq!(stop_phase(&f), "partial");
        let gates = evaluate_stop_gates(&f);
        assert!(gates
            .iter()
            .any(|g| g.name == "supervisor_quiesced" && g.passes()));
        assert!(gates.iter().all(|g| g.passes()));
        assert!(stop_next_action(&f).is_none());
    }

    #[test]
    fn stop_stopping_while_draining() {
        // Listeners gone, but the kernel port is still in TIME_WAIT and sockets not
        // yet unlinked — transient, no corrective action.
        let mut f = stop_clean_all();
        f.brain_port_bound = true;
        f.sockets_present = true;
        assert_eq!(stop_phase(&f), "stopping");
        assert!(stop_next_action(&f).is_none());
    }

    // ------------------------------------------------------------------
    // REQ-AXO-902616 — la vivacité doit dire QUEL indexeur vit, et le gate
    // ne doit pas expliquer faux.
    //
    // Valeurs réelles de l'incident du 2026-09-04 : l'orphelin 650712 tenait
    // le flock IST et alimentait le battement PG pendant que le superviseur
    // relançait en vain le pid 544703, 2 686 fois en 22 heures.
    // ------------------------------------------------------------------

    fn proprietaire_vivant(pid: i64) -> IstOwnershipFacts {
        IstOwnershipFacts {
            probed: true,
            held_by_live_process: true,
            owner_pid: Some(pid),
            owner_identity: Some("axon-indexer@live".to_string()),
        }
    }

    #[test]
    fn l_etiquette_de_propriete_ist_distingue_les_quatre_cas() {
        assert_eq!(
            IstOwnershipFacts::default().label(Some(1)),
            "unmeasured",
            "ne pas avoir mesuré n'est pas « personne ne tient »"
        );
        let libre = IstOwnershipFacts {
            probed: true,
            ..Default::default()
        };
        assert_eq!(libre.label(Some(1)), "free");
        assert_eq!(proprietaire_vivant(544_703).label(Some(544_703)), "supervised");
        assert_eq!(proprietaire_vivant(650_712).label(Some(544_703)), "diverged");
        let anonyme = IstOwnershipFacts {
            probed: true,
            held_by_live_process: true,
            ..Default::default()
        };
        assert_eq!(anonyme.label(Some(544_703)), "held_by_unknown");
    }

    /// Critère 1 — jamais vert sur une divergence.
    #[test]
    fn indexer_alive_ne_reste_pas_vert_quand_le_proprietaire_du_verrou_diverge() {
        let l = LivenessFacts {
            brain_serving: true,
            indexer_expected: true,
            indexer_ready: true,
            indexer_lifecycle: "healthy".to_string(),
            indexer_source: "pg_heartbeat".to_string(),
            ist_ownership: proprietaire_vivant(650_712),
            supervised_pid: Some(544_703),
            ..Default::default()
        };
        let gate = evaluate_liveness_gates(&l)
            .into_iter()
            .find(|g| g.name == "indexer_alive")
            .expect("le gate existe");
        assert!(
            !gate.passes(),
            "un battement frais écrit par un indexeur que le superviseur ne suit plus \
             n'est pas une vivacité : {gate:?}"
        );
        assert!(
            gate.detail.contains("650712") && gate.detail.contains("544703"),
            "le message doit nommer LES DEUX pids : {}",
            gate.detail
        );
    }

    /// Le gate reste vert quand le propriétaire EST le processus supervisé —
    /// sinon la correction crierait au loup à chaque runtime sain.
    #[test]
    fn indexer_alive_reste_vert_quand_le_proprietaire_est_le_processus_supervise() {
        let l = LivenessFacts {
            brain_serving: true,
            indexer_expected: true,
            indexer_ready: true,
            indexer_lifecycle: "healthy".to_string(),
            indexer_source: "pg_heartbeat".to_string(),
            ist_ownership: proprietaire_vivant(544_703),
            supervised_pid: Some(544_703),
            ..Default::default()
        };
        assert!(evaluate_liveness_gates(&l).iter().all(|g| g.passes()));
    }

    /// Rétrocompatibilité stricte : un appelant qui ne sonde pas la propriété —
    /// `CutoverFacts::new_healthy()` en tête — garde le verdict d'avant au bit près.
    /// Un `Unknown` glissé ici enverrait chaque cutover en auto-rollback.
    #[test]
    fn indexer_alive_garde_son_verdict_quand_la_propriete_n_est_pas_mesuree() {
        let l = LivenessFacts {
            brain_serving: true,
            indexer_expected: true,
            indexer_ready: true,
            indexer_lifecycle: "healthy".to_string(),
            indexer_source: "pg_heartbeat".to_string(),
            ..Default::default()
        };
        assert!(
            evaluate_liveness_gates(&l).iter().all(|g| g.passes()),
            "sans mesure de propriété, le comportement historique est conservé"
        );
    }

    /// Critère 2 + 4 — « nothing holds » n'est affirmé qu'après vérification.
    #[test]
    fn le_gate_de_stabilite_n_affirme_nothing_holds_que_s_il_l_a_verifie() {
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Restarting".to_string(),
            restarts: 2_686,
            pid: 544_703,
            age_ms: 40,
            ist_ownership: proprietaire_vivant(650_712),
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s).into_iter().next().unwrap();
        assert!(gate.is_red(), "le verdict reste rouge, c'est bien une panne");
        assert!(
            !gate.detail.contains("nothing holds"),
            "quelque chose tenait : 650712, vivant. Le message ment : {}",
            gate.detail
        );
        assert!(
            gate.detail.contains("650712"),
            "critère 3 : nommer le pid du propriétaire réel : {}",
            gate.detail
        );
    }

    /// L'autre moitié du critère 4 — un verrou réellement libre se dit tel quel.
    #[test]
    fn le_gate_de_stabilite_dit_le_verrou_libre_quand_il_l_a_mesure_libre() {
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Restarting".to_string(),
            restarts: 45,
            pid: 544_703,
            age_ms: 40,
            ist_ownership: IstOwnershipFacts {
                probed: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s).into_iter().next().unwrap();
        assert!(gate.is_red());
        assert!(
            gate.detail.contains("no live process holds"),
            "un verrou mesuré libre se dit, sans supposer : {}",
            gate.detail
        );
    }

    /// Sans sonde, le gate décrit le symptôme SANS en supposer la cause.
    #[test]
    fn sans_sonde_le_gate_de_stabilite_ne_suppose_aucune_cause() {
        let s = SupervisorFacts {
            reachable: true,
            role_found: true,
            status: "Restarting".to_string(),
            restarts: 45,
            pid: 544_703,
            age_ms: 40,
            ..Default::default()
        };
        let gate = evaluate_supervisor_gates(&s).into_iter().next().unwrap();
        assert!(gate.is_red());
        assert!(
            !gate.detail.contains("nothing holds")
                && !gate.detail.contains("no live process holds"),
            "sans mesure, aucune affirmation sur le verrou : {}",
            gate.detail
        );
        assert!(
            gate.detail.contains("NOT probed"),
            "et l'absence de mesure est DITE : {}",
            gate.detail
        );
    }

    /// REQ-AXO-902594 — parse_listen_queue_depth extrait la longueur hexadécimale du Recv-Q
    #[test]
    fn parse_listen_queue_depth_extrait_exactement_le_compte_hexadecimal() {
        let fake_proc_net_tcp = "\
  sl  local_address rem_address   st tx_queue:rx_queue tr tm->when retrnsmt   uid  timeout inode
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0 100 0 0 10 0
  18: 00000000:AC61 00000000:0000 0A 00000000:0000001F 00:00000000 00000000  1000        0 32639875 1 0 100 0 0 10 0
  85: 0100007F:AC61 0100007F:C31A 01 00000000:00000000 00:00000000 00000000  1000        0 69973244 1 0 20 4 28 10 -1";

        // Port 44129 (0xAC61) en LISTEN avec rx_queue = 0x1F = 31 (le cas DOC !)
        assert_eq!(parse_listen_queue_depth(fake_proc_net_tcp, 44129), Some(31));
        // Port 22 (0x0016) en LISTEN avec rx_queue = 0
        assert_eq!(parse_listen_queue_depth(fake_proc_net_tcp, 22), Some(0));
        // Port absent
        assert_eq!(parse_listen_queue_depth(fake_proc_net_tcp, 80), None);
    }

    /// REQ-AXO-902594 — contrôle positif obligatoire :
    /// saturer volontairement la file fait rougir la sonde et l'évaluation de liveness.
    #[test]
    fn saturer_la_file_d_acceptation_fait_rougir_la_sonde_et_l_evaluation_de_liveness() {
        // (1) File saturée (ex: 31 connexions en attente comme mesuré par DOC)
        let l_saturated = LivenessFacts {
            brain_serving: true,
            indexer_expected: false,
            indexer_ready: false,
            accept_queue_depth: Some(31),
            ..Default::default()
        };
        let gates_saturated = evaluate_liveness_gates(&l_saturated);
        let accept_gate = gates_saturated
            .iter()
            .find(|g| g.name == "brain_accept_queue")
            .expect("gate brain_accept_queue must exist");

        assert!(
            accept_gate.is_red(),
            "une file d'acceptation saturée DOIT faire rougir le gate brain_accept_queue"
        );
        assert!(
            accept_gate.detail.contains("saturated") && accept_gate.detail.contains("31"),
            "le détail doit expliciter la saturation et le compte: {}",
            accept_gate.detail
        );
        assert_eq!(
            liveness_phase(&l_saturated),
            Some("brain_accept_queue_saturated"),
            "la phase de liveness doit refléter la saturation de la file d'acceptation"
        );
        let next_action = liveness_next_action(&l_saturated).expect("next action required");
        assert!(
            next_action.contains("accept queue backlog saturated")
                && next_action.contains("restart/axon-brain"),
            "l'action corrective doit ordonner le redémarrage du brain: {next_action}"
        );

        // (2) Contre-exemple obligatoire : file saine (ex: 0 ou 2)
        let l_healthy = LivenessFacts {
            brain_serving: true,
            indexer_expected: false,
            indexer_ready: false,
            accept_queue_depth: Some(0),
            ..Default::default()
        };
        let gates_healthy = evaluate_liveness_gates(&l_healthy);
        let accept_gate_healthy = gates_healthy
            .iter()
            .find(|g| g.name == "brain_accept_queue")
            .expect("gate brain_accept_queue must exist");
        assert!(
            accept_gate_healthy.passes(),
            "une file saine doit passer verte"
        );
    }

    /// REQ-AXO-902591 — parse_cutover_scope_unit extrait le pid créateur et le numéro de séquence
    #[test]
    fn parse_cutover_scope_unit_extrait_pid_et_sequence() {
        let u1 = parse_cutover_scope_unit("axon-live-cutover-2589023-1.scope").expect("parse u1");
        assert_eq!(u1.name, "axon-live-cutover-2589023-1.scope");
        assert_eq!(u1.creator_pid, 2589023);
        assert_eq!(u1.sequence, 1);

        let u2 = parse_cutover_scope_unit("axon-live-cutover-42-7").expect("parse u2 sans suffixe");
        assert_eq!(u2.name, "axon-live-cutover-42-7");
        assert_eq!(u2.creator_pid, 42);
        assert_eq!(u2.sequence, 7);

        assert!(parse_cutover_scope_unit("unrelated-service.scope").is_none());
        assert!(parse_cutover_scope_unit("axon-live.service").is_none());
    }

    /// REQ-AXO-902591 — parse_cutover_scopes_listing extrait toutes les unités cutover d'un listing systemctl
    #[test]
    fn parse_cutover_scopes_listing_extrait_toutes_les_unites() {
        let fake_listing = "\
axon-live-cutover-2589023-1.scope loaded active running [systemd-run] bash
unrelated.scope                  loaded active running unrelated
axon-live-cutover-314159-2.scope  loaded active running [systemd-run] bash
";
        let scopes = parse_cutover_scopes_listing(fake_listing);
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].creator_pid, 2589023);
        assert_eq!(scopes[1].creator_pid, 314159);
    }

    /// REQ-AXO-902591 — filter_orphaned_scopes discrimine les scopes dont l'orchestrateur est mort ou antérieur
    #[test]
    fn filter_orphaned_scopes_distingue_orphelins_et_courant() {
        let scopes = vec![
            CutoverScopeInfo {
                name: "axon-live-cutover-100-1.scope".to_string(),
                creator_pid: 100,
                sequence: 1,
            },
            CutoverScopeInfo {
                name: "axon-live-cutover-200-1.scope".to_string(),
                creator_pid: 200,
                sequence: 1,
            },
        ];

        // PID 100 est mort, PID 200 est vivant
        let is_alive = |pid: u32| pid == 200;

        // Sans current_pid : seul 100 est orphelin (car mort)
        let orphelins_sans_current = filter_orphaned_scopes(&scopes, None, is_alive);
        assert_eq!(orphelins_sans_current.len(), 1);
        assert_eq!(orphelins_sans_current[0].creator_pid, 100);

        // Avec current_pid = Some(200) : 100 est orphelin (car PID != 200), 200 est préservé
        let orphelins_avec_current = filter_orphaned_scopes(&scopes, Some(200), is_alive);
        assert_eq!(orphelins_avec_current.len(), 1);
        assert_eq!(orphelins_avec_current[0].creator_pid, 100);

        // Avec current_pid = Some(300) : 100 ET 200 sont orphelins (car aucun n'est 300)
        let orphelins_avec_nouveau = filter_orphaned_scopes(&scopes, Some(300), is_alive);
        assert_eq!(orphelins_avec_nouveau.len(), 2);
    }
}

