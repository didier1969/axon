//! REQ-AXO-902027 — fail-loud GPU shared-library pre-flight.
//!
//! When a native GPU library is corrupt/incompatible (the canonical incident:
//! a corrupted `libnvinfer` in the nix-store → deterministic SIGSEGV), the
//! crash happens DEEP inside `GpuB2Embedder::try_new_cuda` (ORT session commit
//! / TensorRT engine build), so it never returns an `Err` — the indexer dies
//! with a native segfault that appears NOWHERE in the application log, only in
//! `dmesg`. REQ-AXO-902021 split this off (AC#2): turn that silent native crash
//! into an EXPLICIT application-log line (lib + path + reason) + a clean
//! signalled exit consumable by the `indexer_lifecycle` verdict.
//!
//! In-process `dlopen` is NOT an option: a corrupt lib would segfault the
//! indexer itself — the very thing we are trying to avoid. So we probe each
//! library in a THROWAWAY SUBPROCESS (`axon-indexer --__gpu-lib-probe <path>`):
//! the corrupt lib crashes the probe, and the parent observes the signal and
//! logs it. A corrupt `libnvinfer` is a load-time (`DT_NEEDED`) dependency of
//! the TensorRT provider `.so`, so probing the provider lib also exercises it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Hidden CLI flag that turns the indexer binary into a one-shot dlopen probe.
pub(crate) const GPU_LIB_PROBE_FLAG: &str = "--__gpu-lib-probe";

/// Stderr marker the probe child prints on a (catchable) libloading failure, so
/// the parent can surface the real reason rather than a bare exit code.
const PROBE_ERROR_MARKER: &str = "GPU_LIB_PROBE_ERROR:";

/// Minimum plausible size for a real shared object — anything smaller is a
/// truncated / placeholder file, not a usable `.so`.
const MIN_PLAUSIBLE_SO_BYTES: u64 = 4096;

/// Parse the probe target out of an argv iterator. Pure → unit-testable.
/// Returns the path that follows [`GPU_LIB_PROBE_FLAG`], if present.
pub(crate) fn parse_probe_arg<I: IntoIterator<Item = String>>(args: I) -> Option<PathBuf> {
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == GPU_LIB_PROBE_FLAG {
            return it.next().map(PathBuf::from);
        }
    }
    None
}

/// If the current process was launched as a dlopen probe, perform the probe and
/// return the exit code to use. Returns `None` for a normal indexer launch.
///
/// The probe loads the library with eager symbol resolution (`RTLD_NOW`) so a
/// corrupt `.so` (or a corrupt `DT_NEEDED` dependency) faults HERE, in the
/// throwaway child, instead of inside the real indexer.
pub(crate) fn run_dlopen_probe_if_requested() -> Option<i32> {
    let path = parse_probe_arg(std::env::args())?;
    // SAFETY: loading an arbitrary shared object can run initialisers; that is
    // the whole point — we WANT a corrupt lib to fault this throwaway process.
    let result = unsafe {
        #[cfg(unix)]
        {
            use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
            Library::open(Some(&path), RTLD_NOW | RTLD_LOCAL).map(|_| ())
        }
        #[cfg(not(unix))]
        {
            libloading::Library::new(&path).map(|_| ())
        }
    };
    match result {
        Ok(()) => Some(0),
        Err(err) => {
            eprintln!("{PROBE_ERROR_MARKER} {err}");
            Some(1)
        }
    }
}

/// Cheap static integrity check: the file must exist, be a regular file of
/// plausible size, and start with the ELF magic. Catches the absent / truncated
/// / non-ELF cases without spawning anything.
fn check_static(path: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(path)
        .map_err(|err| format!("not readable ({err})"))?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() < MIN_PLAUSIBLE_SO_BYTES {
        return Err(format!(
            "implausibly small ({} bytes) — truncated/placeholder",
            meta.len()
        ));
    }
    let mut magic = [0u8; 4];
    use std::io::Read;
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|err| format!("header unreadable ({err})"))?;
    if magic != [0x7f, b'E', b'L', b'F'] {
        return Err("not an ELF shared object (bad magic)".to_string());
    }
    Ok(())
}

/// Probe one library in a throwaway subprocess. `Ok(())` when the child loaded
/// it cleanly; `Err(reason)` when the child crashed (signal — corrupt/
/// incompatible) or reported a load error.
fn probe_in_subprocess(self_exe: &Path, lib: &Path) -> Result<(), String> {
    let output = Command::new(self_exe)
        .arg(GPU_LIB_PROBE_FLAG)
        .arg(lib)
        .output()
        .map_err(|err| format!("could not spawn dlopen probe: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = output.status.signal() {
            return Err(format!(
                "dlopen crashed the probe with signal {sig} — library is corrupt or \
                 ABI-incompatible (would have SIGSEGV'd the indexer)"
            ));
        }
    }
    // Non-zero exit: surface the child's reported libloading error if present.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .find_map(|l| l.trim().strip_prefix(PROBE_ERROR_MARKER))
        .map(|r| r.trim().to_string())
        .unwrap_or_else(|| format!("probe exited with {}", output.status));
    Err(format!("dlopen failed: {reason}"))
}

/// The libraries to vet before a CUDA/TensorRT embedder session is built.
/// Returns `(label, path)` pairs for the ones that are configured.
fn gpu_libraries_to_check() -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    if let Some(core) = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        out.push(("onnxruntime core", PathBuf::from(core)));
    }
    if let Some(cuda) = super::ort_cuda_provider_library_path() {
        out.push(("onnxruntime CUDA provider", cuda));
    }
    if let Some(trt) = super::gpu_backend::ort_tensorrt_provider_library_path() {
        out.push(("onnxruntime TensorRT provider", trt));
    }
    out
}

/// REQ-AXO-902027 — vet every configured GPU shared library BEFORE the embedder
/// session is built. `Ok(())` when all load cleanly. `Err(reason)` names the
/// exact lib + path + failure so the caller can log it explicitly and exit
/// cleanly instead of dying on a silent native SIGSEGV. Each failing lib is
/// also logged via `tracing::error!` as it is found.
pub(crate) fn preflight_gpu_libraries() -> Result<(), String> {
    let self_exe = std::env::current_exe()
        .map_err(|err| format!("cannot resolve own exe for the dlopen probe: {err}"))?;
    let libs = gpu_libraries_to_check();
    if libs.is_empty() {
        return Ok(()); // no GPU libs configured → nothing to vet
    }
    for (label, path) in libs {
        if let Err(reason) = check_static(&path) {
            let msg = format!("{label} at {}: {reason}", path.display());
            tracing::error!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), reason = %reason, "GPU library pre-flight FAILED (static)");
            return Err(msg);
        }
        if let Err(reason) = probe_in_subprocess(&self_exe, &path) {
            let msg = format!("{label} at {}: {reason}", path.display());
            tracing::error!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), reason = %reason, "GPU library pre-flight FAILED (dlopen probe)");
            return Err(msg);
        }
        tracing::debug!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), "GPU library pre-flight ok");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_probe_arg_extracts_path() {
        let args = vec![
            "axon-indexer".to_string(),
            GPU_LIB_PROBE_FLAG.to_string(),
            "/lib/foo.so".to_string(),
        ];
        assert_eq!(parse_probe_arg(args), Some(PathBuf::from("/lib/foo.so")));
    }

    #[test]
    fn parse_probe_arg_none_for_normal_launch() {
        let args = vec!["axon-indexer".to_string(), "--indexer".to_string()];
        assert_eq!(parse_probe_arg(args), None);
    }

    #[test]
    fn parse_probe_arg_none_when_flag_has_no_value() {
        let args = vec!["axon-indexer".to_string(), GPU_LIB_PROBE_FLAG.to_string()];
        assert_eq!(parse_probe_arg(args), None);
    }

    #[test]
    fn check_static_rejects_missing_file() {
        let err = check_static(Path::new("/nonexistent/libfoo.so")).unwrap_err();
        assert!(err.contains("not readable"), "got: {err}");
    }

    #[test]
    fn check_static_rejects_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("trunc.so");
        std::fs::write(&p, b"\x7fELF").unwrap(); // 4 bytes, below the floor
        let err = check_static(&p).unwrap_err();
        assert!(err.contains("implausibly small"), "got: {err}");
    }

    #[test]
    fn check_static_rejects_non_elf() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notelf.so");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&vec![b'M'; MIN_PLAUSIBLE_SO_BYTES as usize + 16])
            .unwrap();
        let err = check_static(&p).unwrap_err();
        assert!(err.contains("not an ELF"), "got: {err}");
    }

    #[test]
    fn check_static_accepts_elf_shaped_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ok.so");
        let mut buf = vec![0x7f, b'E', b'L', b'F'];
        buf.extend(std::iter::repeat(0u8).take(MIN_PLAUSIBLE_SO_BYTES as usize));
        std::fs::write(&p, &buf).unwrap();
        assert!(check_static(&p).is_ok());
    }
}
