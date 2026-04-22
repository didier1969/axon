use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

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

        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
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

impl Drop for WriterGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
    }
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
    use super::WriterGuard;
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

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
}
