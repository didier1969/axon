use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tracing::{info, warn};

use crate::graph_bootstrap::{canonical_ist_db_path, canonical_soll_db_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterTarget {
    Ist,
    Soll,
}

impl WriterTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ist => "ist",
            Self::Soll => "soll",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Ist => "IST",
            Self::Soll => "SOLL",
        }
    }

    fn canonical_db_path(self, db_root: &str) -> Option<PathBuf> {
        match self {
            Self::Ist => canonical_ist_db_path(db_root),
            Self::Soll => canonical_soll_db_path(db_root),
        }
    }

    fn lock_path(self, db_root: &str) -> Option<PathBuf> {
        if db_root == ":memory:" {
            return None;
        }

        let mut path = PathBuf::from(db_root);
        path.push(format!(".axon-{}.writer.lock", self.as_str()));
        Some(path)
    }
}

#[derive(Debug)]
pub struct WriterGuard {
    _file: File,
    pub target: WriterTarget,
    pub lock_path: Option<PathBuf>,
    pub db_path: Option<PathBuf>,
    pub owner_identity: String,
}

impl WriterGuard {
    pub fn acquire_ist(db_root: &str) -> Result<Self> {
        Self::acquire(WriterTarget::Ist, db_root)
    }

    pub fn acquire_soll(db_root: &str) -> Result<Self> {
        Self::acquire(WriterTarget::Soll, db_root)
    }

    fn acquire(target: WriterTarget, db_root: &str) -> Result<Self> {
        let owner_identity = runtime_owner_identity();
        let db_path = target.canonical_db_path(db_root);
        let Some(lock_path) = target.lock_path(db_root) else {
            return Ok(Self {
                _file: open_memory_backed_placeholder()?,
                target,
                lock_path: None,
                db_path,
                owner_identity,
            });
        };

        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create writer guard directory for {}",
                    target.display_name()
                )
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| {
                format!(
                    "Failed to open {} writer guard at {}",
                    target.display_name(),
                    lock_path.display()
                )
            })?;

        let mut rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 && target == WriterTarget::Ist {
            // REQ-AXO-902614 — le verrou est tenu. Si le propriétaire est un frère
            // orphelin du MÊME superviseur, le reprendre : sinon le superviseur
            // relance à l'infini un processus toujours refusé (2 686 fois en 22 h
            // le 2026-09-04), et son `stop` ne coupe pas celui qui travaille.
            let metadata = read_lock_metadata(&mut file).unwrap_or_default();
            if attempt_ist_takeover(&metadata, &file) {
                rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            }
        }
        if rc != 0 {
            let metadata = read_lock_metadata(&mut file).unwrap_or_default();
            let operator_hint = if metadata.is_empty() {
                "current owner metadata unavailable".to_string()
            } else {
                format!("recorded owner: {}", metadata.replace('\n', "; "))
            };
            return Err(anyhow!(
                "Refusing startup: {} writer ownership is already held for {}. Stop the active runtime before starting another writer. Lock={} ({})",
                target.display_name(),
                db_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| format!("{}/{} writer", db_root, target.display_name())),
                lock_path.display(),
                operator_hint
            ));
        }

        write_lock_metadata(&mut file, target, &owner_identity, db_path.as_deref())?;

        Ok(Self {
            _file: file,
            target,
            lock_path: Some(lock_path),
            db_path,
            owner_identity,
        })
    }
}

/// REQ-AXO-902614 — tenter la reprise du verrou IST. Rend `true` si le
/// propriétaire a été terminé et que le verrou peut être retenté.
///
/// UNE seule tentative, un SIGTERM et jamais un SIGKILL, et une attente bornée du
/// teardown (`REQ-AXO-902263` : un SIGKILL en plein teardown CUDA a déjà laissé un
/// worker en état D). Chaque refus est journalisé AVEC sa raison — un refus muet
/// fait rouvrir l'enquête depuis zéro.
fn attempt_ist_takeover(metadata: &str, file: &File) -> bool {
    let (identity, pid) = parse_recorded_owner(metadata);
    let self_identity = runtime_identity_only();
    let Some(owner_pid) = pid else {
        warn!("IST writer takeover refused: owner pid is not readable from the lock metadata");
        return false;
    };
    let (owner_ppid, owner_start) = match read_proc_ppid_and_starttime(owner_pid) {
        Some((ppid, start)) => (Some(ppid), Some(start)),
        None => (None, None),
    };
    let (self_ppid, self_start) = match read_proc_ppid_and_starttime(std::process::id() as i64) {
        Some(pair) => pair,
        None => {
            warn!("IST writer takeover refused: our own /proc entry is unreadable");
            return false;
        }
    };
    let owner = OwnerProbe {
        pid: Some(owner_pid),
        identity,
        ppid: owner_ppid,
        starttime_ticks: owner_start,
    };
    match decide_takeover(
        &owner,
        &self_identity,
        self_ppid,
        self_start,
        takeover_enabled(),
    ) {
        TakeoverDecision::Refuse { reason } => {
            warn!("IST writer takeover refused (pid {owner_pid}): {reason}");
            false
        }
        TakeoverDecision::TerminateOwner { pid } => {
            warn!(
                "IST writer takeover: pid {pid} holds the lock, shares our supervisor {self_ppid} \
                 and our identity `{self_identity}`, and started before us. The supervisor lost \
                 its reference to it, so it can no longer be stopped through the supervisor. \
                 Sending SIGTERM and waiting up to {} ms for its teardown.",
                TAKEOVER_TEARDOWN_TIMEOUT_MS
            );
            terminate_and_wait_for_lock(pid, file, TAKEOVER_TEARDOWN_TIMEOUT_MS)
        }
    }
}

/// REQ-AXO-902614 — SIGTERM, puis attente bornée que le VERROU se libère.
/// Jamais de SIGKILL : on laisse le teardown se faire, ou on renonce.
///
/// L'attente sonde le **flock**, jamais le pid. `kill(pid, 0)` réussit sur un
/// **zombie**, et le propriétaire est souvent un enfant : il reste zombie tant que
/// son parent n'a pas appelé `wait()`. Une attente écrite sur le pid ne voyait donc
/// jamais la mort et brûlait les 120 s complètes — c'est exactement l'erreur que
/// `REQ-AXO-902157` avait déjà corrigée côté bash (`[ -e /proc/$pid ]` répond vrai
/// pour un zombie). Le noyau, lui, libère le flock à la mort du propriétaire,
/// zombie inclus : c'est la seule autorité.
fn terminate_and_wait_for_lock(pid: i64, file: &File, timeout_ms: u64) -> bool {
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        warn!("IST writer takeover: SIGTERM to pid {pid} failed — leaving it alone");
        return false;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            info!("IST writer takeover: pid {pid} released the lock, it is ours now");
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    warn!(
        "IST writer takeover: pid {pid} still holds the lock after {timeout_ms} ms — NOT \
         escalating to SIGKILL (REQ-AXO-902263), refusing startup instead"
    );
    false
}

/// REQ-AXO-902614 — l'identité runtime SEULE, sans le `;pid=` que
/// [`runtime_owner_identity`] y ajoute pour la métadonnée du verrou.
fn runtime_identity_only() -> String {
    std::env::var("AXON_RUNTIME_IDENTITY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNDECLARED_RUNTIME_IDENTITY.to_string())
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// REQ-AXO-902157 — authoritative liveness of a writer guard. The guard is a
/// `flock` (advisory lock the kernel releases on the owner's death, INCLUDING a
/// zombie/`<defunct>` process). The ONLY truth is therefore "can it be re-acquired?".
/// The `pid=` recorded in the lock-file metadata must NOT be trusted for liveness:
/// the bash `verify_writer_guard_release` did exactly that (`[ -e /proc/$pid ]`,
/// which reads TRUE for a zombie) and wrongly refused a live restart when a guard
/// owner had become an orphaned zombie. This tests the flock itself — the same
/// mechanism [`WriterGuard::acquire`] uses — so bash callers stop re-deriving a
/// worse answer than the Rust truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardLiveness {
    /// No live owner: lock file absent, or `flock` re-acquires cleanly (a prior
    /// owner — even a zombie — has released it). Safe to (re)start / take over.
    Free { recorded_owner: Option<String> },
    /// A LIVE process holds the flock. `recorded_owner` is its self-declared id.
    HeldByLiveProcess { recorded_owner: Option<String> },
}

/// REQ-AXO-902157 — probe [`GuardLiveness`] for the IST writer guard.
pub fn guard_liveness_ist(db_root: &str) -> Result<GuardLiveness> {
    guard_liveness(WriterTarget::Ist, db_root)
}

/// REQ-AXO-902157 — probe [`GuardLiveness`] for the SOLL writer guard.
pub fn guard_liveness_soll(db_root: &str) -> Result<GuardLiveness> {
    guard_liveness(WriterTarget::Soll, db_root)
}

fn guard_liveness(target: WriterTarget, db_root: &str) -> Result<GuardLiveness> {
    let Some(lock_path) = target.lock_path(db_root) else {
        return Ok(GuardLiveness::Free { recorded_owner: None });
    };
    if !lock_path.exists() {
        return Ok(GuardLiveness::Free { recorded_owner: None });
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("Failed to open writer guard at {}", lock_path.display()))?;
    let recorded_owner = read_lock_metadata(&mut file).ok().filter(|s| !s.is_empty());
    // Try to grab the flock non-blocking. Success => no live holder (the kernel
    // released it on the previous owner's death, zombie included); release it
    // immediately so this probe never becomes the owner. EWOULDBLOCK => a live
    // process holds it.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        Ok(GuardLiveness::Free { recorded_owner })
    } else {
        Ok(GuardLiveness::HeldByLiveProcess { recorded_owner })
    }
}

/// REQ-AXO-902614 — identité runtime par défaut quand rien n'est déclaré. Une
/// reprise de verrou ne s'autorise JAMAIS sur cette valeur : reprendre exige de
/// savoir qui on est, et de savoir que l'autre est le même rôle.
pub const UNDECLARED_RUNTIME_IDENTITY: &str = "unknown-runtime";

/// REQ-AXO-902614 / REQ-AXO-902263 — délai laissé au propriétaire pour son
/// teardown après SIGTERM. Un SIGKILL en plein teardown CUDA a déjà laissé un
/// worker en état D : on attend, on ne force pas.
pub const TAKEOVER_TEARDOWN_TIMEOUT_MS: u64 = 120_000;

/// REQ-AXO-902614 — ce qu'on sait du propriétaire actuel du verrou.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerProbe {
    pub pid: Option<i64>,
    pub identity: Option<String>,
    /// Parent du propriétaire, lu dans `/proc/<pid>/stat`. Même parent que nous
    /// ⇒ même superviseur, donc un superviseur qui a perdu sa référence.
    pub ppid: Option<i64>,
    /// Date de démarrage en tops d'horloge (`/proc/<pid>/stat`, champ 22).
    /// Comparable entre processus du même hôte, et monotone : c'est ce qui dit
    /// lequel des deux est l'ancien.
    pub starttime_ticks: Option<u64>,
}

/// REQ-AXO-902614 — reprendre le verrou, ou refuser comme avant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeoverDecision {
    /// Le propriétaire est un frère orphelin du même superviseur : le terminer
    /// (SIGTERM) puis prendre le verrou.
    TerminateOwner { pid: i64 },
    /// Comportement historique : refus, code 1, le superviseur relance.
    Refuse { reason: String },
}

/// REQ-AXO-902614 — les conditions de la reprise, en une fonction pure.
///
/// `process-compose` 1.94.0 n'offre AUCUNE politique de redémarrage conditionnée
/// au code de sortie (`always` / `on_failure` / `exit_on_failure` / `no`). Un
/// processus refusé ne peut donc pas arrêter sa propre relance : la boucle se
/// casse en faisant que le refus n'ait pas lieu.
///
/// La règle est « celui que le superviseur suit doit être celui qui travaille » —
/// et c'est une exigence de SÛRETÉ : le 2026-09-04, `process stop axon-indexer` a
/// rendu `Successfully stopped` sans tuer l'indexeur qui travaillait.
///
/// Les cinq gardes sont cumulatives, et chacune protège un cas réel : un indexeur
/// lancé à la main, un autre superviseur, un autre rôle, une identité non
/// déclarée, ou un propriétaire plus récent que nous — aucun n'est repris.
pub fn decide_takeover(
    owner: &OwnerProbe,
    self_identity: &str,
    self_ppid: i64,
    self_starttime_ticks: u64,
    enabled: bool,
) -> TakeoverDecision {
    let refus = |raison: &str| TakeoverDecision::Refuse {
        reason: raison.to_string(),
    };
    if !enabled {
        return refus("takeover disabled by operator (AXON_IST_WRITER_TAKEOVER=0)");
    }
    // Garde 1 — reprendre exige de savoir QUI on est. Deux identités non
    // déclarées ne prouvent pas un même rôle : c'est l'absence de preuve.
    if self_identity.is_empty() || self_identity == UNDECLARED_RUNTIME_IDENTITY {
        return refus("this runtime declares no identity — refusing to take over blindly");
    }
    let Some(owner_pid) = owner.pid else {
        return refus("owner pid is not readable from the lock metadata");
    };
    // Garde 2 — même rôle, déclaré des deux côtés.
    match owner.identity.as_deref() {
        Some(identity) if identity == self_identity => {}
        Some(identity) => {
            return refus(&format!(
                "owner runtime identity `{identity}` differs from ours `{self_identity}`"
            ))
        }
        None => return refus("owner declares no runtime identity"),
    }
    // Garde 3 — même superviseur. Un parent différent, c'est un indexeur lancé à
    // la main ou par un autre superviseur : il n'est pas à nous de le terminer.
    match owner.ppid {
        Some(ppid) if ppid == self_ppid => {}
        Some(ppid) => {
            return refus(&format!(
                "owner parent {ppid} is not our supervisor {self_ppid} — not a sibling"
            ))
        }
        None => return refus("owner parent is not readable — cannot prove a shared supervisor"),
    }
    // Garde 4 — le superviseur ne garde que la référence la PLUS RÉCENTE. Si le
    // propriétaire est plus jeune que nous, c'est LUI le processus suivi, et c'est
    // nous qui sommes de trop.
    match owner.starttime_ticks {
        Some(ticks) if ticks < self_starttime_ticks => {}
        Some(_) => return refus("owner started after us — it is the supervised process, not us"),
        None => return refus("owner start time is not readable — cannot prove which one is older"),
    }
    TakeoverDecision::TerminateOwner { pid: owner_pid }
}

/// REQ-AXO-902614 — `ppid` et `starttime` d'un processus, lus dans `/proc`.
///
/// Le nom de la commande est entre parenthèses et peut contenir des espaces ET
/// des parenthèses : on découpe après la DERNIÈRE `)`, jamais par `split` naïf.
pub fn read_proc_ppid_and_starttime(pid: i64) -> Option<(i64, u64)> {
    parse_proc_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// REQ-AXO-902614 — le parsing seul, testable sans `/proc`.
///
/// Format : `pid (comm) state ppid ... starttime`, `starttime` étant le 22e
/// champ. `comm` est entre parenthèses et peut contenir des espaces ET des
/// parenthèses — un `split_whitespace` naïf sur la ligne entière lirait de
/// travers dès qu'un binaire s'appelle `(a b)`. On découpe donc après la
/// DERNIÈRE `)` : le premier champ restant est `state` (3e), donc `ppid` est à
/// l'indice 1 et `starttime` à l'indice 22 - 3 = 19.
pub fn parse_proc_stat(brut: &str) -> Option<(i64, u64)> {
    let apres = &brut[brut.rfind(')')? + 1..];
    let champs: Vec<&str> = apres.split_whitespace().collect();
    let ppid = champs.get(1)?.parse::<i64>().ok()?;
    let starttime = champs.get(19)?.parse::<u64>().ok()?;
    Some((ppid, starttime))
}

/// REQ-AXO-902614 — la reprise est-elle autorisée par l'opérateur ?
/// Activée par défaut ; `AXON_IST_WRITER_TAKEOVER=0` la coupe.
pub fn takeover_enabled() -> bool {
    !matches!(
        std::env::var("AXON_IST_WRITER_TAKEOVER")
            .unwrap_or_default()
            .trim(),
        "0" | "false" | "off" | "no"
    )
}

/// REQ-AXO-902616 — extraire l'identité et le pid de la métadonnée du verrou.
///
/// Forme réelle écrite par [`write_lock_metadata`], lue le 2026-09-04 :
///
/// ```text
/// target=IST
/// owner=axon-live-axon-indexer;pid=650712
/// db_path=/home/dstadel/projects/axon/.axon/graph_v2/ist.db
/// ```
///
/// `acquire` aplatit les retours à la ligne en `"; "` avant de les journaliser ;
/// cette fonction accepte les deux formes, parce qu'un appelant qui reçoit l'une
/// ou l'autre ne doit pas avoir à le savoir.
pub fn parse_recorded_owner(recorded: &str) -> (Option<String>, Option<i64>) {
    let mut identity = None;
    let mut pid = None;
    for champ in recorded.split(['\n', ';']) {
        let champ = champ.trim();
        if let Some(value) = champ.strip_prefix("owner=") {
            let value = value.trim();
            if !value.is_empty() {
                identity = Some(value.to_string());
            }
        } else if let Some(value) = champ.strip_prefix("pid=") {
            // Un pid non numérique n'est PAS un pid : ne rien affirmer plutôt que
            // rendre 0, qui se lirait comme un vrai processus.
            pid = value.trim().parse::<i64>().ok();
        }
    }
    (identity, pid)
}

/// REQ-AXO-902616 — la racine de base canonique, résolue une seule fois et de la
/// même façon partout. `runtime_boot` la dérivait en ligne ; le brain a besoin de
/// la MÊME valeur pour sonder le verrou que l'indexeur a posé — deux dérivations
/// séparées finiraient par diverger et la sonde regarderait le mauvais fichier.
pub fn resolve_db_root() -> String {
    std::env::var("AXON_DB_ROOT").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.local/share/axon/db"))
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .map(|dir| format!("{}/.axon/graph_v2", dir.display()))
                    .unwrap_or_else(|_| ".axon/graph_v2".to_string())
            })
    })
}

fn runtime_owner_identity() -> String {
    let runtime_identity = std::env::var("AXON_RUNTIME_IDENTITY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown-runtime".to_string());
    format!("{runtime_identity};pid={}", std::process::id())
}

fn open_memory_backed_placeholder() -> Result<File> {
    let path = std::env::temp_dir().join(format!(
        "axon-memory-writer-guard-{}-{}.lock",
        std::process::id(),
        std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
    ));
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(path)
        .context("Failed to create memory-backed writer guard placeholder")
}

fn read_lock_metadata(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut payload = String::new();
    file.read_to_string(&mut payload)?;
    Ok(payload.trim().to_string())
}

fn write_lock_metadata(
    file: &mut File,
    target: WriterTarget,
    owner_identity: &str,
    db_path: Option<&Path>,
) -> Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "target={}", target.display_name())?;
    writeln!(file, "owner={owner_identity}")?;
    if let Some(path) = db_path {
        writeln!(file, "db_path={}", path.display())?;
    }
    file.sync_data()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        guard_liveness, guard_liveness_ist, guard_liveness_soll, GuardLiveness, WriterGuard,
        WriterTarget,
    };
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    // REQ-AXO-902190 — guard_liveness is the private core of the zombie-safe writer probe
    // (a top uncovered hub; existing tests only reach the guard_liveness_ist/soll wrappers).
    // Called DIRECTLY: no lock file (or :memory: root) ⇒ Free with no owner — the safe default
    // that lets a restart take over a dead owner's slot. No real flock touched.
    #[test]
    fn guard_liveness_free_when_lock_absent_or_memory_root() {
        let mem = guard_liveness(WriterTarget::Ist, ":memory:").unwrap();
        assert!(matches!(mem, GuardLiveness::Free { recorded_owner: None }));
        let missing = guard_liveness(WriterTarget::Soll, "/nonexistent-axon-dir-902190").unwrap();
        assert!(matches!(missing, GuardLiveness::Free { recorded_owner: None }));
    }

    fn wait_for_ready_file(path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "helper process did not create ready file at {}",
            path.display()
        );
    }

    #[test]
    fn writer_guard_subprocess_helper() {
        let mode = std::env::var("AXON_WRITER_GUARD_HELPER_MODE").ok();
        if mode.is_none() {
            return;
        }

        let db_root = std::env::var("AXON_WRITER_GUARD_DB_ROOT").unwrap();
        let ready_file = std::env::var("AXON_WRITER_GUARD_READY_FILE")
            .ok()
            .map(std::path::PathBuf::from);

        match mode.as_deref() {
            Some("hold_ist") => {
                let _guard = WriterGuard::acquire_ist(&db_root).expect("helper must acquire IST");
                if let Some(path) = ready_file {
                    fs::write(path, "ready").expect("helper must write ready file");
                }
                thread::sleep(Duration::from_secs(3));
            }
            // REQ-AXO-902614 — tenir assez longtemps pour qu'un frere tente la reprise.
            Some("hold_ist_long") => {
                let _guard = WriterGuard::acquire_ist(&db_root).expect("helper must acquire IST");
                if let Some(path) = ready_file {
                    fs::write(path, "ready").expect("helper must write ready file");
                }
                thread::sleep(Duration::from_secs(20));
            }
            Some("assert_acquires_ist") => {
                let acquired = WriterGuard::acquire_ist(&db_root);
                assert!(
                    acquired.is_ok(),
                    "second process failed to take over a sibling's IST writer lock: {:?}",
                    acquired.err()
                );
            }
            Some("assert_refused_ist") => {
                let acquired = WriterGuard::acquire_ist(&db_root);
                assert!(
                    acquired.is_err(),
                    "second process unexpectedly acquired IST writer guard"
                );
            }
            Some(other) => panic!("unknown helper mode: {other}"),
            None => {}
        }
    }

    #[test]
    fn indexer_refuses_second_ist_writer() {
        let db_root = tempdir().unwrap();
        let first = WriterGuard::acquire_ist(db_root.path().to_str().unwrap()).unwrap();
        let second = WriterGuard::acquire_ist(db_root.path().to_str().unwrap());
        assert!(second.is_err());
        drop(first);
    }

    #[test]
    fn soll_refuses_second_writer() {
        let db_root = tempdir().unwrap();
        let first = WriterGuard::acquire_soll(db_root.path().to_str().unwrap()).unwrap();
        let second = WriterGuard::acquire_soll(db_root.path().to_str().unwrap());
        assert!(second.is_err());
        drop(first);
    }

    #[test]
    fn ist_writer_lock_is_released_on_drop() {
        let db_root = tempdir().unwrap();
        {
            let _first = WriterGuard::acquire_ist(db_root.path().to_str().unwrap()).unwrap();
        }
        let reacquired = WriterGuard::acquire_ist(db_root.path().to_str().unwrap());
        assert!(reacquired.is_ok());
    }

    #[test]
    fn indexer_refuses_second_ist_writer_across_processes() {
        let db_root = tempdir().unwrap();
        let ready_file = db_root.path().join("helper-ready");
        let exe = std::env::current_exe().unwrap();
        let helper_name = "runtime_writer_guard::tests::writer_guard_subprocess_helper";

        let mut holder = Command::new(&exe)
            .arg("--exact")
            .arg(helper_name)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "hold_ist")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .env("AXON_WRITER_GUARD_READY_FILE", &ready_file)
            .spawn()
            .expect("failed to spawn holder process");

        wait_for_ready_file(&ready_file);

        let refused = Command::new(&exe)
            .arg("--exact")
            .arg(helper_name)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "assert_refused_ist")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .status()
            .expect("failed to spawn refusal probe");

        assert!(
            refused.success(),
            "second process was not refused while first held the IST writer lock"
        );

        let holder_status = holder.wait().expect("failed waiting for holder process");
        assert!(
            holder_status.success(),
            "holder process did not exit cleanly"
        );
    }

    // --- REQ-AXO-902157 — authoritative guard liveness (flock truth) ---

    #[test]
    fn guard_liveness_free_when_no_lock_file() {
        let db_root = tempdir().unwrap();
        let live = guard_liveness_soll(db_root.path().to_str().unwrap()).unwrap();
        assert_eq!(live, GuardLiveness::Free { recorded_owner: None });
    }

    #[test]
    fn guard_liveness_free_when_owner_pid_metadata_is_stale() {
        // THE fix, encoded: a lock file that EXISTS with a recorded owner pid but
        // that NO live process flock-holds (owner died — zombie or gone) must read
        // Free. The bash `[ -e /proc/$pid ]` check wrongly reported this as held.
        let db_root = tempdir().unwrap();
        let lock_path = db_root.path().join(".axon-soll.writer.lock");
        fs::write(
            &lock_path,
            "target=SOLL\nowner=axon-live-axon-brain;pid=999999\ndb_path=/x/soll.db\n",
        )
        .unwrap();
        let live = guard_liveness_soll(db_root.path().to_str().unwrap()).unwrap();
        match live {
            GuardLiveness::Free { recorded_owner } => {
                // metadata is surfaced (for diagnostics) but NOT trusted for liveness.
                assert!(recorded_owner.unwrap().contains("pid=999999"));
            }
            other => panic!("stale-owner lock must read Free, got {other:?}"),
        }
    }

    #[test]
    fn guard_liveness_held_while_live_owner_holds_flock_then_free_on_drop() {
        let db_root = tempdir().unwrap();
        let root = db_root.path().to_str().unwrap();
        {
            let _held = WriterGuard::acquire_ist(root).unwrap();
            let live = guard_liveness_ist(root).unwrap();
            assert!(
                matches!(live, GuardLiveness::HeldByLiveProcess { .. }),
                "a live flock holder must read HeldByLiveProcess, got {live:?}"
            );
        }
        // holder dropped -> flock released -> Free.
        let after = guard_liveness_ist(root).unwrap();
        assert!(matches!(after, GuardLiveness::Free { .. }), "got {after:?}");
    }

    // ------------------------------------------------------------------
    // REQ-AXO-902616 — le brain doit pouvoir NOMMER le propriétaire du verrou.
    // Valeurs verbatim du verrou live du 2026-09-04.
    // ------------------------------------------------------------------

    #[test]
    fn le_proprietaire_inscrit_se_lit_sous_sa_forme_multiligne() {
        let (identity, pid) = super::parse_recorded_owner(
            "target=IST\nowner=axon-live-axon-indexer;pid=650712\ndb_path=/x/ist.db\n",
        );
        assert_eq!(identity.as_deref(), Some("axon-live-axon-indexer"));
        assert_eq!(pid, Some(650_712));
    }

    #[test]
    fn le_proprietaire_inscrit_se_lit_aussi_aplati_en_point_virgule() {
        // `acquire` aplatit avant de journaliser ; l'appelant ne doit pas le savoir.
        let (identity, pid) = super::parse_recorded_owner(
            "target=IST; owner=axon-live-axon-indexer;pid=650712; db_path=/x/ist.db",
        );
        assert_eq!(identity.as_deref(), Some("axon-live-axon-indexer"));
        assert_eq!(pid, Some(650_712));
    }

    #[test]
    fn une_metadonnee_illisible_ne_fabrique_pas_de_proprietaire() {
        assert_eq!(super::parse_recorded_owner(""), (None, None));
        assert_eq!(super::parse_recorded_owner("bruit sans cle"), (None, None));
        // Un pid non numérique n'est pas un pid : ne rien affirmer plutôt que zéro.
        let (identity, pid) = super::parse_recorded_owner("owner=axon-live;pid=abc");
        assert_eq!(identity.as_deref(), Some("axon-live"));
        assert_eq!(pid, None);
    }

    #[test]
    fn la_racine_de_base_honore_la_variable_d_environnement() {
        // Vérifie le contrat, pas l'environnement : la fonction DOIT préférer
        // AXON_DB_ROOT, sinon le brain sonderait un autre verrou que l'indexeur.
        let resolved = super::resolve_db_root();
        assert!(
            !resolved.is_empty(),
            "une racine vide ferait sonder le repertoire courant en silence"
        );
    }

    // ------------------------------------------------------------------
    // REQ-AXO-902614 — la reprise du verrou, et les cinq gardes qui
    // l'interdisent. Valeurs de l'incident du 2026-09-04 : l'orphelin 650712
    // et le superviseur 473194.
    // ------------------------------------------------------------------

    fn orphelin_frere() -> super::OwnerProbe {
        super::OwnerProbe {
            pid: Some(650_712),
            identity: Some("axon-live-axon-indexer".to_string()),
            ppid: Some(473_194),
            starttime_ticks: Some(1_000),
        }
    }

    #[test]
    fn un_frere_orphelin_du_meme_superviseur_est_repris() {
        let decision = super::decide_takeover(
            &orphelin_frere(),
            "axon-live-axon-indexer",
            473_194,
            2_000, // nous sommes plus récent : c'est nous que le superviseur suit
            true,
        );
        assert_eq!(
            decision,
            super::TakeoverDecision::TerminateOwner { pid: 650_712 },
            "sans reprise, le superviseur relance a l'infini un processus toujours refuse"
        );
    }

    #[test]
    fn un_proprietaire_d_un_autre_superviseur_n_est_jamais_repris() {
        let decision =
            super::decide_takeover(&orphelin_frere(), "axon-live-axon-indexer", 999_999, 2_000, true);
        assert!(
            matches!(decision, super::TakeoverDecision::Refuse { .. }),
            "un indexeur lance a la main ou par un autre superviseur ne se tue pas"
        );
    }

    #[test]
    fn un_proprietaire_d_une_autre_identite_n_est_jamais_repris() {
        let decision =
            super::decide_takeover(&orphelin_frere(), "axon-dev-axon-indexer", 473_194, 2_000, true);
        assert!(matches!(decision, super::TakeoverDecision::Refuse { .. }));
    }

    #[test]
    fn une_identite_non_declaree_n_autorise_aucune_reprise() {
        // Reprendre exige de savoir qui on est. Deux `unknown-runtime` ne sont
        // PAS la preuve d'un meme role — c'est l'absence de preuve.
        let mut owner = orphelin_frere();
        owner.identity = Some(super::UNDECLARED_RUNTIME_IDENTITY.to_string());
        let decision = super::decide_takeover(
            &owner,
            super::UNDECLARED_RUNTIME_IDENTITY,
            473_194,
            2_000,
            true,
        );
        assert!(matches!(decision, super::TakeoverDecision::Refuse { .. }));
    }

    #[test]
    fn un_proprietaire_plus_recent_que_nous_n_est_jamais_repris() {
        // Le superviseur ne garde que la reference la PLUS RECENTE : si le
        // proprietaire est plus jeune, c'est LUI le processus suivi.
        let decision = super::decide_takeover(
            &orphelin_frere(),
            "axon-live-axon-indexer",
            473_194,
            500, // nous sommes plus ancien
            true,
        );
        assert!(matches!(decision, super::TakeoverDecision::Refuse { .. }));
    }

    #[test]
    fn l_operateur_peut_eteindre_la_reprise() {
        let decision = super::decide_takeover(
            &orphelin_frere(),
            "axon-live-axon-indexer",
            473_194,
            2_000,
            false,
        );
        assert!(matches!(decision, super::TakeoverDecision::Refuse { .. }));
    }

    #[test]
    fn un_proprietaire_sans_pid_lisible_n_est_jamais_repris() {
        let mut owner = orphelin_frere();
        owner.pid = None;
        let decision =
            super::decide_takeover(&owner, "axon-live-axon-indexer", 473_194, 2_000, true);
        assert!(matches!(decision, super::TakeoverDecision::Refuse { .. }));
    }

    /// Chaque refus DIT pourquoi : un opérateur qui lit « refus » sans raison
    /// rouvre l'enquête depuis zéro, ce que cette session a fait pendant 22 h.
    #[test]
    fn chaque_refus_de_reprise_porte_sa_raison() {
        for decision in [
            super::decide_takeover(&orphelin_frere(), "autre", 473_194, 2_000, true),
            super::decide_takeover(&orphelin_frere(), "axon-live-axon-indexer", 1, 2_000, true),
            super::decide_takeover(&orphelin_frere(), "axon-live-axon-indexer", 473_194, 1, true),
        ] {
            match decision {
                super::TakeoverDecision::Refuse { reason } => {
                    assert!(!reason.is_empty(), "un refus muet fait rouvrir l'enquete");
                }
                other => panic!("attendu un refus, recu {other:?}"),
            }
        }
    }

    /// REQ-AXO-902614 — un `comm` bavard ne doit pas décaler la lecture.
    #[test]
    fn le_stat_proc_se_lit_meme_quand_le_nom_du_binaire_contient_des_parentheses() {
        // 52 champs, `comm` = "(axon (a b))" — le cas qui casse un split naïf.
        let mut champs: Vec<String> = vec!["650712".into(), "(axon (a b))".into()];
        champs.push("S".into()); // 3 state
        champs.push("473194".into()); // 4 ppid
        for i in 5..=21 {
            champs.push(i.to_string());
        }
        champs.push("80877610".into()); // 22 starttime
        let brut = champs.join(" ");
        assert_eq!(super::parse_proc_stat(&brut), Some((473_194, 80_877_610)));
    }

    #[test]
    fn un_stat_tronque_ne_fabrique_aucune_valeur() {
        assert_eq!(super::parse_proc_stat(""), None);
        assert_eq!(super::parse_proc_stat("650712 (axon) S 473194"), None);
    }

    /// Le processus courant est la seule vérité disponible sans monter de fixture :
    /// son ppid doit être celui que le noyau nous donne par ailleurs.
    #[test]
    fn le_stat_proc_du_processus_courant_concorde_avec_le_noyau() {
        let pid = std::process::id() as i64;
        let (ppid, starttime) =
            super::read_proc_ppid_and_starttime(pid).expect("/proc du processus courant");
        assert_eq!(ppid, unsafe { libc::getppid() } as i64);
        assert!(starttime > 0, "un starttime nul ne saurait ordonner deux processus");
    }

    /// REQ-AXO-902614 critère 4 — deux boots concurrents, MÊME superviseur (le
    /// processus de test) et MÊME identité déclarée : le second doit désormais
    /// RÉUSSIR par reprise, là où il bouclait à l'infini.
    #[test]
    fn deux_boots_freres_du_meme_superviseur_le_second_reprend_le_verrou() {
        let db_root = tempdir().unwrap();
        let ready_file = db_root.path().join("helper-ready");
        let exe = std::env::current_exe().unwrap();
        let helper = "runtime_writer_guard::tests::writer_guard_subprocess_helper";

        let mut holder = Command::new(&exe)
            .arg("--exact")
            .arg(helper)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "hold_ist_long")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .env("AXON_WRITER_GUARD_READY_FILE", &ready_file)
            .env("AXON_RUNTIME_IDENTITY", "axon-test-indexer")
            .spawn()
            .expect("holder");
        wait_for_ready_file(&ready_file);

        let repris = Command::new(&exe)
            .arg("--exact")
            .arg(helper)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "assert_acquires_ist")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .env("AXON_RUNTIME_IDENTITY", "axon-test-indexer")
            .status()
            .expect("repreneur");
        assert!(
            repris.success(),
            "le second boot n'a pas repris le verrou d'un frere orphelin : c'est la \
             boucle de 2 686 relances qui revient"
        );
        let _ = holder.wait();
    }

    /// L'autre moitié : identité NON déclarée ⇒ aucune reprise, comportement
    /// historique conservé. C'est la garde qui protège tout indexeur lancé
    /// autrement que par un superviseur qui déclare son identité.
    #[test]
    fn sans_identite_declaree_le_second_boot_reste_refuse() {
        let db_root = tempdir().unwrap();
        let ready_file = db_root.path().join("helper-ready");
        let exe = std::env::current_exe().unwrap();
        let helper = "runtime_writer_guard::tests::writer_guard_subprocess_helper";

        let mut holder = Command::new(&exe)
            .arg("--exact")
            .arg(helper)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "hold_ist_long")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .env("AXON_WRITER_GUARD_READY_FILE", &ready_file)
            .env_remove("AXON_RUNTIME_IDENTITY")
            .spawn()
            .expect("holder");
        wait_for_ready_file(&ready_file);

        let refuse = Command::new(&exe)
            .arg("--exact")
            .arg(helper)
            .arg("--nocapture")
            .env("AXON_WRITER_GUARD_HELPER_MODE", "assert_refused_ist")
            .env("AXON_WRITER_GUARD_DB_ROOT", db_root.path())
            .env_remove("AXON_RUNTIME_IDENTITY")
            .status()
            .expect("sonde de refus");
        assert!(refuse.success(), "une identite non declaree ne doit JAMAIS reprendre");
        let _ = holder.wait();
    }
}
