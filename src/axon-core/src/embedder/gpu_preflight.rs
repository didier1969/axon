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

/// How strictly a library's dlopen failure is judged (REQ-AXO-902345).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeMode {
    /// The ORT CORE loads standalone, so ANY dlopen failure is a real defect.
    Strict,
    /// An ORT PROVIDER (`libonnxruntime_providers_{cuda,tensorrt}.so`)
    /// legitimately carries undefined symbols (`Provider_GetHost`) that the core
    /// resolves at runtime through ORT's provider bridge, so that ONE failure
    /// class is expected and benign. Every other failure — above all a missing
    /// `DT_NEEDED` dependency such as the driver's `libcuda.so.1` — is real.
    TolerateUndefinedSymbols,
}

/// Why a probe failed. TYPED rather than a bare string so a crash can never be
/// softened: only [`ProbeFailure::Dlopen`] is ever eligible for tolerance.
#[derive(Debug)]
enum ProbeFailure {
    /// The probe child could not even be spawned.
    Spawn(String),
    /// The child died on a signal — corrupt / ABI-incompatible library.
    Crashed(String),
    /// The child reported a catchable load error.
    Dlopen(String),
}

impl ProbeFailure {
    fn reason(&self) -> &str {
        match self {
            Self::Spawn(r) | Self::Crashed(r) | Self::Dlopen(r) => r,
        }
    }
}

/// PURE — does this dlopen reason denote the benign "the ORT provider bridge
/// resolves it at runtime" case?
///
/// Kept as a separate string-level predicate precisely so it can be falsified in
/// unit tests against the EXACT messages observed in the field, rather than
/// trusted by inspection. The two real messages from 2026-08-17, same machine,
/// same provider, only `LD_LIBRARY_PATH` differing:
///   defect : `libcuda.so.1: cannot open shared object file: No such file or directory`
///   benign : `…/libonnxruntime_providers_cuda.so: undefined symbol: Provider_GetHost`
fn dlopen_reason_is_undefined_symbol(reason: &str) -> bool {
    reason.contains("undefined symbol")
}

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
/// The probe loads the library with LAZY binding (`RTLD_LAZY`): the loader still
/// maps the `.so` AND eagerly loads its `DT_NEEDED` dependencies (so a corrupt
/// `libnvinfer` pulled in by the TensorRT provider still faults HERE, in the
/// throwaway child), but it does NOT eagerly resolve every undefined symbol.
/// That matters because the ONNX Runtime provider libs
/// (`libonnxruntime_providers_{cuda,tensorrt}.so`) legitimately carry undefined
/// symbols (e.g. `Provider_GetHost`) that the ORT CORE lib resolves at runtime
/// through its provider bridge — `RTLD_NOW` would FALSE-POSITIVE on those and
/// wrongly refuse a perfectly healthy GPU stack (REQ-AXO-902027 regression
/// caught in dev: the indexer never spawned its pipeline).
pub(crate) fn run_dlopen_probe_if_requested() -> Option<i32> {
    let path = parse_probe_arg(std::env::args())?;
    // SAFETY: loading an arbitrary shared object can run initialisers; that is
    // the whole point — we WANT a corrupt lib to fault this throwaway process.
    let result = unsafe {
        #[cfg(unix)]
        {
            use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_LAZY, RTLD_LOCAL};
            // The ORT provider libs (cuda/tensorrt) carry undefined symbols
            // (e.g. `Provider_GetHost`) that are EXPORTED by the ORT CORE lib
            // and resolved at runtime through ORT's provider bridge — and the
            // providers are linked BIND_NOW, so RTLD_LAZY alone still
            // false-positives ("undefined symbol"). Load the CORE globally
            // first (its exports enter the global scope) and keep it resident,
            // THEN load the target so the provider's symbols resolve exactly as
            // they do at runtime. A corrupt core / provider / DT_NEEDED dep
            // (libnvinfer) still faults HERE in the throwaway child. When the
            // target IS the core, the preload is skipped (would be redundant).
            let _core_guard = std::env::var("ORT_DYLIB_PATH")
                .ok()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty() && std::path::Path::new(c) != path.as_path())
                .and_then(|c| Library::open(Some(c.as_str()), RTLD_GLOBAL | RTLD_LAZY).ok());
            Library::open(Some(&path), RTLD_LAZY | RTLD_LOCAL).map(|_| ())
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
fn probe_in_subprocess(self_exe: &Path, lib: &Path) -> Result<(), ProbeFailure> {
    let output = Command::new(self_exe)
        .arg(GPU_LIB_PROBE_FLAG)
        .arg(lib)
        .output()
        .map_err(|err| ProbeFailure::Spawn(format!("could not spawn dlopen probe: {err}")))?;
    if output.status.success() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = output.status.signal() {
            return Err(ProbeFailure::Crashed(format!(
                "dlopen crashed the probe with signal {sig} — library is corrupt or \
                 ABI-incompatible (would have SIGSEGV'd the indexer)"
            )));
        }
    }
    // Non-zero exit: surface the child's reported libloading error if present.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .find_map(|l| l.trim().strip_prefix(PROBE_ERROR_MARKER))
        .map(|r| r.trim().to_string())
        .unwrap_or_else(|| format!("probe exited with {}", output.status));
    Err(ProbeFailure::Dlopen(format!("dlopen failed: {reason}")))
}

/// The libraries to vet before a CUDA/TensorRT embedder session is built.
/// Returns `(label, path, ProbeMode)` — every library IS dlopen-probed; the mode
/// says how its failure is judged.
///
/// REQ-AXO-902345 — the provider libs used to be static-checked ONLY, on the
/// grounds that probing them in isolation false-positives with
/// `undefined symbol: Provider_GetHost` (REQ-AXO-902040). That reasoning was
/// sound but the remedy was too blunt, and it cost a real outage on 2026-08-17:
/// after the WSL->Ubuntu migration the driver's `libcuda.so.1` was no longer on
/// `LD_LIBRARY_PATH`, so `libonnxruntime_providers_cuda.so` could not be loaded
/// at all. The FILE was present and well-formed, so the static check passed, the
/// CUDA EP failed at session build, the embedder fell back to CPU — and the
/// instance still reported HEALTHY while every semantic query was degraded.
///
/// Both failures are dlopen failures, but they are DISTINGUISHABLE, and the
/// distinction was verified on the real binaries before this change (same host,
/// same provider, only `LD_LIBRARY_PATH` differing):
///   without the driver dir → `libcuda.so.1: cannot open shared object file`
///   with    the driver dir → `undefined symbol: Provider_GetHost`
/// So the providers are probed with [`ProbeMode::TolerateUndefinedSymbols`]:
/// the benign class is ignored, the missing-dependency class fails loud.
fn gpu_libraries_to_check() -> Vec<(&'static str, PathBuf, ProbeMode)> {
    let mut out = Vec::new();
    if let Some(core) = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        out.push(("onnxruntime core", PathBuf::from(core), ProbeMode::Strict));
    }
    if let Some(cuda) = super::ort_cuda_provider_library_path() {
        out.push((
            "onnxruntime CUDA provider",
            cuda,
            ProbeMode::TolerateUndefinedSymbols,
        ));
    }
    if let Some(trt) = super::gpu_backend::ort_tensorrt_provider_library_path() {
        out.push((
            "onnxruntime TensorRT provider",
            trt,
            ProbeMode::TolerateUndefinedSymbols,
        ));
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
    for (label, path, mode) in libs {
        if let Err(reason) = check_static(&path) {
            let msg = format!("{label} at {}: {reason}", path.display());
            tracing::error!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), reason = %reason, "GPU library pre-flight FAILED (static)");
            return Err(msg);
        }
        // REQ-AXO-902345 — every configured lib is dlopen-probed. Only ONE
        // failure class is softened, and only for the provider libs: an
        // `undefined symbol` they legitimately carry (resolved at runtime by
        // ORT's provider bridge). A missing DT_NEEDED dependency, a corrupt lib
        // or a crashed probe still fails loud — the whole point, since a
        // silently unloadable CUDA provider is what let the embedder degrade to
        // CPU under a HEALTHY banner on 2026-08-17.
        if let Err(failure) = probe_in_subprocess(&self_exe, &path) {
            let tolerated = mode == ProbeMode::TolerateUndefinedSymbols
                && matches!(&failure, ProbeFailure::Dlopen(r) if dlopen_reason_is_undefined_symbol(r));
            if tolerated {
                tracing::debug!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), reason = %failure.reason(), "GPU provider carries runtime-resolved symbols — expected, not a defect");
            } else {
                let reason = failure.reason();
                let msg = format!("{label} at {}: {reason}", path.display());
                tracing::error!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), reason = %reason, "GPU library pre-flight FAILED (dlopen probe)");
                return Err(msg);
            }
        }
        tracing::debug!(target: "embedder::gpu_preflight", lib = label, path = %path.display(), "GPU library pre-flight ok");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// REQ-AXO-902345 — the two messages below are VERBATIM from the real
    /// binaries on 2026-08-17: same host, same `libonnxruntime_providers_cuda.so`,
    /// the ONLY difference being whether the NVIDIA driver directory was on
    /// `LD_LIBRARY_PATH`. They are the ground truth this classifier exists to
    /// separate, which is why they are pinned here rather than paraphrased.
    const REAL_MISSING_DEP: &str =
        "dlopen failed: libcuda.so.1: cannot open shared object file: No such file or directory";
    const REAL_RUNTIME_RESOLVED: &str = "dlopen failed: /nix/store/x-onnxruntime-1.27.1/lib/\
         libonnxruntime_providers_cuda.so: undefined symbol: Provider_GetHost";

    #[test]
    fn missing_dependency_is_never_treated_as_a_runtime_resolved_symbol() {
        // The 2026-08-17 outage. If this ever returns true the pre-flight goes
        // blind again to exactly the defect it was extended to catch.
        assert!(!dlopen_reason_is_undefined_symbol(REAL_MISSING_DEP));
    }

    #[test]
    fn runtime_resolved_symbol_is_recognised_so_a_healthy_provider_is_not_refused() {
        assert!(dlopen_reason_is_undefined_symbol(REAL_RUNTIME_RESOLVED));
    }

    #[test]
    fn only_the_provider_libs_tolerate_an_undefined_symbol() {
        // The core loads standalone: an undefined symbol there IS a defect.
        // Mirrors the `tolerated` predicate in preflight_gpu_libraries.
        let tolerated = |mode: ProbeMode, reason: &str| {
            mode == ProbeMode::TolerateUndefinedSymbols && dlopen_reason_is_undefined_symbol(reason)
        };
        assert!(tolerated(
            ProbeMode::TolerateUndefinedSymbols,
            REAL_RUNTIME_RESOLVED
        ));
        assert!(!tolerated(ProbeMode::Strict, REAL_RUNTIME_RESOLVED));
        assert!(!tolerated(
            ProbeMode::TolerateUndefinedSymbols,
            REAL_MISSING_DEP
        ));
    }

    #[test]
    fn a_crash_is_never_tolerated_even_if_its_text_mentions_undefined_symbol() {
        // Guards the reason the failure type is an enum and not a bare string:
        // a SIGSEGV must stay fatal no matter what the message happens to read.
        let crashed = ProbeFailure::Crashed("undefined symbol".to_string());
        assert!(
            !matches!(&crashed, ProbeFailure::Dlopen(r) if dlopen_reason_is_undefined_symbol(r)),
            "a crashed probe must never match the tolerated branch"
        );
    }

    #[test]
    fn every_configured_gpu_library_is_dlopen_probed() {
        // REQ-AXO-902345 — the providers were static-checked ONLY, which is how
        // an unloadable CUDA provider reached production. No entry may be
        // probe-exempt: the mode says how a failure is judged, never whether the
        // probe runs at all.
        for (label, _path, mode) in gpu_libraries_to_check() {
            assert!(
                matches!(
                    mode,
                    ProbeMode::Strict | ProbeMode::TolerateUndefinedSymbols
                ),
                "{label} must be dlopen-probed"
            );
        }
    }

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
