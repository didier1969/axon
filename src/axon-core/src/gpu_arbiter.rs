//! SOTA Tri-State GPU Arbiter (REQ-AXO-902680 / DEC-AXO-901713).
//!
//! Enforces strict mutual exclusion between Brain models and Indexer models.
//!
//! Invariants:
//! 1. State 0: Idle (0 models in GPU, Axon VRAM footprint = 0 MiB)
//! 2. State 1: BrainExclusive (Query/NLI active, Indexer yields/CPU)
//! 3. State 2: IndexerExclusive (B2 batch vectorization active, Brain on CPU fallback)
//! 4. (BrainGPU AND IndexerGPU) is strictly impossible (mutual exclusion).
//!
//! Cross-process synchronization is backed by kernel-level POSIX flock + atomic JSON state.
//! If a holder process crashes or dies, the kernel immediately releases the file lock,
//! allowing deadlock-free reclamation.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_LOCK_PATH: &str = "/tmp/axon-gpu-arbiter.lock";
const DEFAULT_STATE_PATH: &str = "/tmp/axon-gpu-arbiter-state.json";
const DEFAULT_LEASE_TTL_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuRole {
    Brain,
    Indexer,
}

impl GpuRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Brain => "brain",
            Self::Indexer => "indexer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GpuLeaseState {
    Idle,
    BrainExclusive {
        pid: u32,
        acquired_epoch_ms: u64,
        ttl_ms: u64,
    },
    IndexerExclusive {
        pid: u32,
        acquired_epoch_ms: u64,
        ttl_ms: u64,
    },
}

impl GpuLeaseState {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn models_in_gpu(&self) -> usize {
        match self {
            Self::Idle => 0,
            Self::BrainExclusive { .. } | Self::IndexerExclusive { .. } => 1,
        }
    }

    pub fn holder_role(&self) -> Option<GpuRole> {
        match self {
            Self::Idle => None,
            Self::BrainExclusive { .. } => Some(GpuRole::Brain),
            Self::IndexerExclusive { .. } => Some(GpuRole::Indexer),
        }
    }

    pub fn holder_pid(&self) -> Option<u32> {
        match self {
            Self::Idle => None,
            Self::BrainExclusive { pid, .. } | Self::IndexerExclusive { pid, .. } => Some(*pid),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuArbiterSnapshot {
    pub state: GpuLeaseState,
    pub models_in_gpu: usize,
    pub active_role: Option<String>,
    pub active_pid: Option<u32>,
    pub is_stale: bool,
}

/// RAII Lease guard ensuring the arbiter returns to Idle upon release or drop.
pub struct GpuLease {
    role: GpuRole,
    released: AtomicBool,
    custom_lock_path: Option<PathBuf>,
    custom_state_path: Option<PathBuf>,
}

impl GpuLease {
    fn new(
        role: GpuRole,
        custom_lock_path: Option<PathBuf>,
        custom_state_path: Option<PathBuf>,
    ) -> Self {
        Self {
            role,
            released: AtomicBool::new(false),
            custom_lock_path,
            custom_state_path,
        }
    }

    pub fn role(&self) -> GpuRole {
        self.role
    }

    /// Explicitly releases the lease ahead of drop.
    pub fn release(&self) -> io::Result<()> {
        if !self.released.swap(true, Ordering::SeqCst) {
            GpuArbiter::release_lease_internal(
                self.role,
                self.custom_lock_path.as_deref(),
                self.custom_state_path.as_deref(),
            )?;
        }
        Ok(())
    }
}

impl Drop for GpuLease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub struct GpuArbiter;

impl GpuArbiter {
    /// Attempts non-blocking acquisition of the GPU lease for the given role.
    /// Returns `Ok(Some(GpuLease))` if acquired or renewed, `Ok(None)` if held by the other role.
    pub fn try_acquire(role: GpuRole, ttl: Duration) -> io::Result<Option<GpuLease>> {
        Self::try_acquire_with_paths(role, ttl, None, None)
    }

    pub fn try_acquire_with_paths(
        role: GpuRole,
        ttl: Duration,
        lock_path: Option<&Path>,
        state_path: Option<&Path>,
    ) -> io::Result<Option<GpuLease>> {
        let lock_file = open_lock_file(lock_path)?;
        let _flock_guard = FlockGuard::acquire_exclusive(&lock_file)?;

        let state_p = resolve_state_path(state_path);
        let mut state = read_state(&state_p)?;
        let now_ms = current_epoch_ms();
        let my_pid = std::process::id();

        // 1. Check if current lease is stale (expired or holder process dead)
        if let Some(pid) = state.holder_pid() {
            let is_expired = match &state {
                GpuLeaseState::BrainExclusive {
                    acquired_epoch_ms,
                    ttl_ms,
                    ..
                }
                | GpuLeaseState::IndexerExclusive {
                    acquired_epoch_ms,
                    ttl_ms,
                    ..
                } => now_ms.saturating_sub(*acquired_epoch_ms) > *ttl_ms,
                GpuLeaseState::Idle => false,
            };
            if is_expired || !is_process_alive(pid) {
                tracing::info!(
                    role = role.as_str(),
                    stale_pid = pid,
                    is_expired,
                    "Reclaiming stale GPU arbiter lease"
                );
                state = GpuLeaseState::Idle;
            }
        }

        // 2. Evaluate lease grant
        match state {
            GpuLeaseState::Idle => {
                let ttl_ms = if ttl.is_zero() {
                    DEFAULT_LEASE_TTL_SECS * 1000
                } else {
                    ttl.as_millis() as u64
                };
                let next_state = match role {
                    GpuRole::Brain => GpuLeaseState::BrainExclusive {
                        pid: my_pid,
                        acquired_epoch_ms: now_ms,
                        ttl_ms,
                    },
                    GpuRole::Indexer => GpuLeaseState::IndexerExclusive {
                        pid: my_pid,
                        acquired_epoch_ms: now_ms,
                        ttl_ms,
                    },
                };
                write_state(&state_p, &next_state)?;
                Ok(Some(GpuLease::new(
                    role,
                    lock_path.map(Path::to_path_buf),
                    state_path.map(Path::to_path_buf),
                )))
            }
            GpuLeaseState::BrainExclusive { pid, .. } if role == GpuRole::Brain => {
                // Heartbeat / renewal by same role
                let ttl_ms = if ttl.is_zero() {
                    DEFAULT_LEASE_TTL_SECS * 1000
                } else {
                    ttl.as_millis() as u64
                };
                let next_state = GpuLeaseState::BrainExclusive {
                    pid: my_pid.min(pid),
                    acquired_epoch_ms: now_ms,
                    ttl_ms,
                };
                write_state(&state_p, &next_state)?;
                Ok(Some(GpuLease::new(
                    role,
                    lock_path.map(Path::to_path_buf),
                    state_path.map(Path::to_path_buf),
                )))
            }
            GpuLeaseState::IndexerExclusive { pid, .. } if role == GpuRole::Indexer => {
                // Heartbeat / renewal by same role
                let ttl_ms = if ttl.is_zero() {
                    DEFAULT_LEASE_TTL_SECS * 1000
                } else {
                    ttl.as_millis() as u64
                };
                let next_state = GpuLeaseState::IndexerExclusive {
                    pid: my_pid.min(pid),
                    acquired_epoch_ms: now_ms,
                    ttl_ms,
                };
                write_state(&state_p, &next_state)?;
                Ok(Some(GpuLease::new(
                    role,
                    lock_path.map(Path::to_path_buf),
                    state_path.map(Path::to_path_buf),
                )))
            }
            _ => {
                // Held by the other role
                Ok(None)
            }
        }
    }

    /// Read the current snapshot without changing state.
    pub fn current_state() -> GpuArbiterSnapshot {
        Self::current_state_with_paths(None, None)
    }

    pub fn current_state_with_paths(
        lock_path: Option<&Path>,
        state_path: Option<&Path>,
    ) -> GpuArbiterSnapshot {
        let lock_file = match open_lock_file(lock_path) {
            Ok(f) => f,
            Err(_) => {
                return GpuArbiterSnapshot {
                    state: GpuLeaseState::Idle,
                    models_in_gpu: 0,
                    active_role: None,
                    active_pid: None,
                    is_stale: false,
                }
            }
        };
        let _flock = match FlockGuard::acquire_shared(&lock_file) {
            Ok(g) => g,
            Err(_) => {
                return GpuArbiterSnapshot {
                    state: GpuLeaseState::Idle,
                    models_in_gpu: 0,
                    active_role: None,
                    active_pid: None,
                    is_stale: false,
                }
            }
        };

        let state_p = resolve_state_path(state_path);
        let state = read_state(&state_p).unwrap_or(GpuLeaseState::Idle);
        let now_ms = current_epoch_ms();
        let (is_stale, active_pid) = match &state {
            GpuLeaseState::Idle => (false, None),
            GpuLeaseState::BrainExclusive {
                pid,
                acquired_epoch_ms,
                ttl_ms,
            }
            | GpuLeaseState::IndexerExclusive {
                pid,
                acquired_epoch_ms,
                ttl_ms,
            } => {
                let expired = now_ms.saturating_sub(*acquired_epoch_ms) > *ttl_ms;
                (expired || !is_process_alive(*pid), Some(*pid))
            }
        };

        GpuArbiterSnapshot {
            models_in_gpu: if is_stale { 0 } else { state.models_in_gpu() },
            active_role: state.holder_role().map(|r| r.as_str().to_string()),
            active_pid,
            is_stale,
            state,
        }
    }

    /// Returns the exact count of models active in GPU according to arbiter (0 or 1).
    pub fn models_in_gpu() -> usize {
        Self::current_state().models_in_gpu
    }

    /// Forcefully resets arbiter state to Idle (e.g. for test cleanup or admin override).
    pub fn force_idle() -> io::Result<()> {
        Self::force_idle_with_paths(None, None)
    }

    pub fn force_idle_with_paths(
        lock_path: Option<&Path>,
        state_path: Option<&Path>,
    ) -> io::Result<()> {
        let lock_file = open_lock_file(lock_path)?;
        let _flock = FlockGuard::acquire_exclusive(&lock_file)?;
        let state_p = resolve_state_path(state_path);
        write_state(&state_p, &GpuLeaseState::Idle)?;
        Ok(())
    }

    /// Releases any lease held by the specified role, resetting to Idle if currently held.
    pub fn release_lease(role: GpuRole) -> io::Result<()> {
        Self::release_lease_internal(role, None, None)
    }

    fn release_lease_internal(
        role: GpuRole,
        lock_path: Option<&Path>,
        state_path: Option<&Path>,
    ) -> io::Result<()> {
        let lock_file = open_lock_file(lock_path)?;
        let _flock = FlockGuard::acquire_exclusive(&lock_file)?;
        let state_p = resolve_state_path(state_path);
        let current = read_state(&state_p)?;
        if current.holder_role() == Some(role) {
            write_state(&state_p, &GpuLeaseState::Idle)?;
            tracing::info!(
                role = role.as_str(),
                "GPU arbiter lease released -> Idle (0 models in GPU)"
            );
        }
        Ok(())
    }
}

struct FlockGuard<'a> {
    file: &'a File,
}

impl<'a> FlockGuard<'a> {
    fn acquire_exclusive(file: &'a File) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    fn acquire_shared(file: &'a File) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_SH) };
        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }
}

impl<'a> Drop for FlockGuard<'a> {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
    }
}

fn resolve_lock_path(custom: Option<&Path>) -> PathBuf {
    if let Some(p) = custom {
        return p.to_path_buf();
    }
    if let Ok(env_val) = std::env::var("AXON_GPU_ARBITER_LOCK_PATH") {
        if !env_val.trim().is_empty() {
            return PathBuf::from(env_val.trim());
        }
    }
    PathBuf::from(DEFAULT_LOCK_PATH)
}

fn resolve_state_path(custom: Option<&Path>) -> PathBuf {
    if let Some(p) = custom {
        return p.to_path_buf();
    }
    if let Ok(env_val) = std::env::var("AXON_GPU_ARBITER_STATE_PATH") {
        if !env_val.trim().is_empty() {
            return PathBuf::from(env_val.trim());
        }
    }
    PathBuf::from(DEFAULT_STATE_PATH)
}

fn open_lock_file(custom: Option<&Path>) -> io::Result<File> {
    let path = resolve_lock_path(custom);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o666)
        .open(&path)
}

fn read_state(path: &Path) -> io::Result<GpuLeaseState> {
    if !path.exists() {
        return Ok(GpuLeaseState::Idle);
    }
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(GpuLeaseState::Idle),
        Err(e) => return Err(e),
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    if buf.trim().is_empty() {
        return Ok(GpuLeaseState::Idle);
    }
    serde_json::from_str(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

fn write_state(path: &Path, state: &GpuLeaseState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(state)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(data.as_bytes())?;
    file.sync_data()?;
    Ok(())
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_paths() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().expect("tempdir");
        let lock = tmp.path().join("gpu.lock");
        let state = tmp.path().join("gpu-state.json");
        (tmp, lock, state)
    }

    #[test]
    fn test_arbiter_initial_state_is_idle() {
        let (_tmp, lock, state) = fixture_paths();
        let snap = GpuArbiter::current_state_with_paths(Some(&lock), Some(&state));
        assert!(snap.state.is_idle());
        assert_eq!(snap.models_in_gpu, 0);
        assert_eq!(snap.active_role, None);
    }

    #[test]
    fn test_arbiter_brain_acquisition_and_release() {
        let (_tmp, lock, state) = fixture_paths();
        let lease = GpuArbiter::try_acquire_with_paths(
            GpuRole::Brain,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("acquire")
        .expect("granted");

        let snap = GpuArbiter::current_state_with_paths(Some(&lock), Some(&state));
        assert_eq!(snap.models_in_gpu, 1);
        assert_eq!(snap.active_role.as_deref(), Some("brain"));

        drop(lease);

        let snap_after = GpuArbiter::current_state_with_paths(Some(&lock), Some(&state));
        assert!(snap_after.state.is_idle());
        assert_eq!(snap_after.models_in_gpu, 0);
    }

    #[test]
    fn test_arbiter_mutual_exclusion_strict() {
        let (_tmp, lock, state) = fixture_paths();
        let brain_lease = GpuArbiter::try_acquire_with_paths(
            GpuRole::Brain,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("acquire")
        .expect("granted");

        // Indexer must be refused while Brain holds lease
        let indexer_res = GpuArbiter::try_acquire_with_paths(
            GpuRole::Indexer,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("indexer try");
        assert!(
            indexer_res.is_none(),
            "Indexer must NOT get lease when Brain holds it"
        );

        drop(brain_lease);

        // After release, Indexer can acquire
        let indexer_lease = GpuArbiter::try_acquire_with_paths(
            GpuRole::Indexer,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("indexer retry")
        .expect("granted now");

        let snap = GpuArbiter::current_state_with_paths(Some(&lock), Some(&state));
        assert_eq!(snap.models_in_gpu, 1);
        assert_eq!(snap.active_role.as_deref(), Some("indexer"));
        drop(indexer_lease);
    }

    #[test]
    fn test_arbiter_same_role_heartbeat_renewal() {
        let (_tmp, lock, state) = fixture_paths();
        let lease1 = GpuArbiter::try_acquire_with_paths(
            GpuRole::Brain,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("acquire 1")
        .expect("granted");

        // Same role renewing succeeds
        let lease2 = GpuArbiter::try_acquire_with_paths(
            GpuRole::Brain,
            Duration::from_secs(20),
            Some(&lock),
            Some(&state),
        )
        .expect("acquire 2")
        .expect("renewed");

        assert_eq!(lease1.role(), GpuRole::Brain);
        assert_eq!(lease2.role(), GpuRole::Brain);
    }

    #[test]
    fn test_arbiter_stale_lease_dead_pid_recovery() {
        let (_tmp, lock, state) = fixture_paths();
        // Write a state with an obviously dead PID (e.g. 9999999)
        let dead_state = GpuLeaseState::BrainExclusive {
            pid: 9999999,
            acquired_epoch_ms: current_epoch_ms(),
            ttl_ms: 100000,
        };
        write_state(&state, &dead_state).expect("write dead state");

        // Indexer should be able to acquire by reclaiming the dead PID's lease
        let indexer_lease = GpuArbiter::try_acquire_with_paths(
            GpuRole::Indexer,
            Duration::from_secs(10),
            Some(&lock),
            Some(&state),
        )
        .expect("acquire")
        .expect("reclaimed dead lease");

        let snap = GpuArbiter::current_state_with_paths(Some(&lock), Some(&state));
        assert_eq!(snap.active_role.as_deref(), Some("indexer"));
        drop(indexer_lease);
    }
}
