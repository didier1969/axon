// REQ-AXO-902275 — the bash assertions this replaces, ported to the main harness.
//
// They are the same cases, and deliberately so: the migration must be provably
// behaviour-preserving, not an excuse to redesign the policy mid-move. What changes is
// where they run — `cargo test` instead of a hand-rolled shell PASS/FAIL loop.

use super::{assess, HostSample, Readiness};

fn sample(load_1m: u64, cores: usize, swap_used_pct: u8, foreign_rustc: usize) -> HostSample {
    HostSample {
        load_1m,
        cores,
        swap_used_pct,
        foreign_rustc,
    }
}

/// THE case this exists for. Zero rustc — the only precondition the project had — while
/// the host sat at load 76 with swap essentially full, saturated by non-Rust third-party
/// processes. The old gate said GO and the measurement taken was meaningless.
#[test]
fn zero_rustc_does_not_mean_quiet_when_load_and_swap_say_otherwise() {
    assert_eq!(
        assess(sample(76, 16, 99, 0)).render(),
        "busy:load=76>2x16cores,swap=99%"
    );
}

/// Each signal must be able to fire ALONE, or a single blind spot reopens — which is
/// exactly how the rustc-only precondition failed.
#[test]
fn each_signal_disqualifies_on_its_own() {
    assert_eq!(
        assess(sample(40, 16, 10, 0)).render(),
        "busy:load=40>2x16cores"
    );
    assert_eq!(assess(sample(4, 16, 95, 0)).render(), "busy:swap=95%");
    assert_eq!(assess(sample(4, 16, 10, 29)).render(), "busy:rustc=29");
}

/// A healthy build legitimately saturates every core, so the bar is 2x cores, not 1x:
/// otherwise every honest full-load measurement would be refused and the check ignored.
#[test]
fn load_bar_is_two_times_cores_not_one() {
    assert!(assess(sample(16, 16, 10, 0)).is_quiet());
    assert!(assess(sample(32, 16, 10, 0)).is_quiet(), "exactly 2x is accepted");
    assert_eq!(
        assess(sample(33, 16, 10, 0)).render(),
        "busy:load=33>2x16cores",
        "one over 2x tips it"
    );
}

/// 89 % is pressure; 90 % is the floor where the kernel has nowhere left to evict.
#[test]
fn swap_threshold_is_ninety_percent() {
    assert!(assess(sample(4, 16, 89, 0)).is_quiet());
    assert_eq!(assess(sample(4, 16, 90, 0)).render(), "busy:swap=90%");
}

/// Naming one cause when there are three sends the operator to fix the wrong thing.
#[test]
fn every_reason_is_reported_not_just_the_first() {
    assert_eq!(
        assess(sample(80, 8, 99, 12)).render(),
        "busy:load=80>2x8cores,swap=99%,rustc=12"
    );
}

#[test]
fn an_idle_host_is_quiet() {
    assert_eq!(assess(sample(0, 16, 0, 0)).render(), "quiet");
}

/// A zero core count must not divide-by-zero nor disqualify every host by accident.
#[test]
fn zero_cores_is_coerced_to_one() {
    assert_eq!(assess(sample(3, 0, 0, 0)).render(), "busy:load=3>2x1cores");
    assert!(assess(sample(2, 0, 0, 0)).is_quiet());
}

/// The readers degrade to a neutral value rather than panicking: this signal is
/// advisory, and a check that crashes the caller is worse than one that says "quiet".
#[test]
fn proc_readers_never_panic_and_return_plausible_values() {
    let load = super::read_load_1m();
    let swap = super::read_swap_used_pct();
    assert!(swap <= 100, "swap percentage must be a percentage, got {swap}");
    // `load` is unbounded by nature; the contract is only that reading it cannot panic
    // and yields something usable by `assess`.
    let _ = assess(sample(load, 16, swap, 0));
}

#[test]
fn quiet_and_busy_render_round_trip() {
    assert_eq!(Readiness::Quiet.render(), "quiet");
    assert_eq!(
        Readiness::Busy(vec!["a".into(), "b".into()]).render(),
        "busy:a,b"
    );
    assert!(Readiness::Quiet.is_quiet());
    assert!(!Readiness::Busy(vec!["x".into()]).is_quiet());
}
