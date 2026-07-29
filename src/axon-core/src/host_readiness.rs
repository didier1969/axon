//! REQ-AXO-902275 / REQ-AXO-902273 — is this host quiet enough for a MEASUREMENT to
//! mean anything?
//!
//! Distinct from build sizing, and the difference is the whole point: sizing answers
//! "how big a build can this host take", this answers "can I BELIEVE a number I measure
//! right now". A host can be perfectly capable of running a build while being far too
//! busy for its timings to mean anything.
//!
//! # Why it exists (session 107, and it cost most of a session)
//!
//! A full `--lib` run was timed on a host whose only checked precondition —
//! `pgrep -c rustc` — read ZERO, i.e. the documented gate said GO. It was in fact at
//! load 76 with 20 kB of 8 GB swap free, saturated by THIRD-PARTY processes (a typedb
//! server at 321 % CPU, a python3.11 at 100 %). The suite passed 594 tests in its first
//! minutes and 7 in its last ten. The conclusion nearly drawn — "my change makes the
//! suite unusable" — would have been false, and would have thrown away a correct fix.
//!
//! The lesson is not "also look at the load". It is that a precondition covering ONE
//! family of processes silently ignores every other one. `rustc` was the family we had
//! been burned by (a sibling repo's pre-push hook spawning one per test case), so it
//! became the check; nothing else did. Load average and swap are process-agnostic: they
//! cannot miss a consumer because they never enumerate consumers.
//!
//! # Why it lives in Rust rather than in the shell (REQ-AXO-902275)
//!
//! It was first written as a bash function with bash tests. That was the reflex the
//! operator called out: the scripts were supposed to carry sequence, not policy. A
//! function that can be tested WITHOUT launching a process belongs here — typed,
//! covered by the main harness, and DRY with the runtime that shares its notion of host
//! pressure. The same audit found a sibling bash policy that had drifted into computing
//! a worker cap nobody read; in Rust an unused function is a compiler warning, and
//! GUI-PRO-003 forbids warnings.

/// A host-readiness verdict. `Quiet` means a timing taken now is worth believing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Quiet,
    /// Busy, with every disqualifying reason — not just the first one found.
    Busy(Vec<String>),
}

impl Readiness {
    pub fn is_quiet(&self) -> bool {
        matches!(self, Readiness::Quiet)
    }

    /// One-line rendering, stable enough for a gate to embed in its PASS/FAIL message:
    /// `quiet` or `busy:load=76>2x16cores,swap=99%`.
    pub fn render(&self) -> String {
        match self {
            Readiness::Quiet => "quiet".to_string(),
            Readiness::Busy(reasons) => format!("busy:{}", reasons.join(",")),
        }
    }
}

/// Host snapshot, taken by the caller so the decision below stays pure.
#[derive(Debug, Clone, Copy)]
pub struct HostSample {
    pub load_1m: u64,
    pub cores: usize,
    pub swap_used_pct: u8,
    pub foreign_rustc: usize,
}

/// PURE decision — no `/proc`, no env, no clock. Every threshold is justified:
///
/// * **load > 2 × cores** — the run queue is more than double what the machine can
///   serve, so wall-clock timings measure the queue, not the code. Two-times rather
///   than one-times because a healthy build legitimately saturates every core, and a
///   check that fires on every honest full-load run is a check people learn to ignore.
/// * **swap used ≥ 90 %** — the kernel has nowhere left to evict to; the next
///   allocation stalls on I/O. Observed at 99.9 % while `MemAvailable` still read
///   18 GB, which is why free RAM alone is NOT a sufficient signal.
/// * **any foreign rustc** — kept from the original gate: a sibling repo's pre-push
///   hook spawns one per test case (29 observed simultaneously during a real promote).
pub fn assess(sample: HostSample) -> Readiness {
    let cores = sample.cores.max(1);
    let mut reasons = Vec::new();

    if sample.load_1m > (cores as u64) * 2 {
        reasons.push(format!("load={}>2x{}cores", sample.load_1m, cores));
    }
    if sample.swap_used_pct >= 90 {
        reasons.push(format!("swap={}%", sample.swap_used_pct));
    }
    if sample.foreign_rustc > 0 {
        reasons.push(format!("rustc={}", sample.foreign_rustc));
    }

    if reasons.is_empty() {
        Readiness::Quiet
    } else {
        Readiness::Busy(reasons)
    }
}

/// 1-minute load average, floored to an integer. 0 when `/proc/loadavg` is unreadable —
/// degrading to "quiet" rather than blocking work on a parse error is deliberate: this
/// signal is advisory, and a check that refuses to run gets bypassed.
pub fn read_load_1m() -> u64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|first| first.split('.').next().and_then(|i| i.parse().ok()))
        .unwrap_or(0)
}

/// Percentage of swap in use; 0 when there is no swap or `/proc/meminfo` is unreadable.
pub fn read_swap_used_pct() -> u8 {
    let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    let field = |name: &str| -> Option<u64> {
        meminfo
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    match (field("SwapTotal:"), field("SwapFree:")) {
        (Some(total), Some(free)) if total > 0 => ((total - free) * 100 / total) as u8,
        _ => 0,
    }
}

#[cfg(test)]
#[path = "host_readiness_tests.rs"]
mod host_readiness_tests;
