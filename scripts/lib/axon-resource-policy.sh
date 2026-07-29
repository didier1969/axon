#!/usr/bin/env bash

# Shared resource-policy resolver for Axon live/dev dual-instance operation.
# Policy decisions live here, then project onto existing runtime knobs.

axon_detect_host_cpu_cores() {
    if command -v nproc >/dev/null 2>&1; then
        nproc
        return 0
    fi

    getconf _NPROCESSORS_ONLN 2>/dev/null || printf '4\n'
}

axon_detect_host_ram_gb() {
    local kb=""
    kb="$(sed -n 's/^MemTotal:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*kB$/\1/p' /proc/meminfo 2>/dev/null | head -n1)"
    if [[ -n "$kb" ]]; then
        printf '%s\n' "$(( kb / 1024 / 1024 ))"
        return 0
    fi

    printf '16\n'
}

axon_normalize_resource_priority() {
    case "${1:-}" in
        critical|CRITICAL) printf 'critical\n' ;;
        best_effort|BEST_EFFORT|besteffort|BESTEFFORT) printf 'best_effort\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_background_budget_class() {
    case "${1:-}" in
        conservative|CONSERVATIVE) printf 'conservative\n' ;;
        balanced|BALANCED) printf 'balanced\n' ;;
        aggressive|AGGRESSIVE) printf 'aggressive\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_gpu_access_policy() {
    case "${1:-}" in
        preferred|PREFERRED) printf 'preferred\n' ;;
        shared|SHARED) printf 'shared\n' ;;
        avoid|AVOID) printf 'avoid\n' ;;
        *) return 1 ;;
    esac
}

axon_normalize_watcher_policy() {
    case "${1:-}" in
        full|FULL) printf 'full\n' ;;
        bounded|BOUNDED) printf 'bounded\n' ;;
        off|OFF) printf 'off\n' ;;
        *) return 1 ;;
    esac
}

axon_default_resource_priority() {
    case "${1:?instance kind required}" in
        live) printf 'critical\n' ;;
        *) printf 'best_effort\n' ;;
    esac
}

axon_default_background_budget_class() {
    case "${1:?instance kind required}" in
        live) printf 'balanced\n' ;;
        *) printf 'conservative\n' ;;
    esac
}

axon_default_gpu_access_policy() {
    # Dev defaults to `preferred` so that local benchmarks (REQ-AXO-221)
    # measure the GPU path that production runs on. The single-GPU
    # exclusion contract (DEC-AXO-067) is now AUTOMATED (REQ-AXO-234): a dev
    # `--indexer-full`/`-vector` start pauses the live indexer (marker
    # .axon/live-paused-by-dev) and the dev stop resumes it — see
    # axon_auto_pause_live_indexer_for_dev / axon_resume_live_indexer_after_dev
    # in axon-supervisor.sh. Opt out with `--no-auto-pause`.
    case "${1:?instance kind required}" in
        live) printf 'preferred\n' ;;
        *) printf 'preferred\n' ;;
    esac
}

axon_default_watcher_policy() {
    case "${1:?instance kind required}" in
        live) printf 'full\n' ;;
        *) printf 'bounded\n' ;;
    esac
}

# REQ-AXO-902275 — `axon_compute_worker_cap` SUPPRIMÉE. Elle calculait un plafond de
# workers (aggressive/balanced/conservative, bornes dev 6 / live 2-12) exporté dans
# `MAX_AXON_WORKERS` — variable qui n'avait **aucun consommateur** : ni Rust, ni Elixir,
# ni YAML, ni Nix, ni aucun autre script. Ses seules occurrences étaient son producteur
# et ses propres tests.
#
# Le dimensionnement réel des workers est décidé côté Rust par
# `runtime_capacity_profile::recommend_sizing()`, avec une formule DIFFÉRENTE. Deux
# politiques pour une même question, dont une inerte : le genre d'écart qui ne se voit
# qu'en remontant les consommateurs un par un.
#
# Écrite en Rust, une fonction sans appelant aurait déclenché un warning, et
# GUI-PRO-003 impose zéro warning. En bash rien ne le signale — d'où la règle posée par
# ce REQ : toute fonction PURE (testable sans lancer un processus) appartient au Rust.

axon_compute_queue_memory_budget_bytes() {
    local budget_class="${1:?budget class required}"
    local ram_gb="${2:?ram_gb required}"
    local budget_gb=1

    case "$budget_class" in
        aggressive)
            budget_gb=$(( ram_gb / 3 ))
            ;;
        balanced)
            budget_gb=$(( ram_gb / 4 ))
            ;;
        conservative)
            budget_gb=$(( ram_gb / 8 ))
            ;;
    esac

    if [[ "$budget_class" == "balanced" || "$budget_class" == "aggressive" ]]; then
        if [[ "$budget_gb" -lt 2 ]]; then
            budget_gb=2
        fi
    elif [[ "$budget_gb" -lt 1 ]]; then
        budget_gb=1
    fi

    if [[ "$budget_class" == "conservative" && "$budget_gb" -gt 4 ]]; then
        budget_gb=4
    fi
    if [[ "$budget_gb" -gt 8 ]]; then
        budget_gb=8
    fi

    printf '%s\n' "$(( budget_gb * 1024 * 1024 * 1024 ))"
}

# REQ-AXO-902275 — `axon_compute_watcher_subtree_hint_budget` SUPPRIMÉE, même motif :
# elle alimentait `AXON_WATCHER_SUBTREE_HINT_BUDGET`, sans aucun consommateur dans le
# dépôt. `AXON_WATCHER_POLICY`, dont elle dérivait, reste exportée et vivante.

axon_resolve_resource_policy() {
    local instance_kind="${1:?instance kind required}"
    local cpu_cores=""
    local ram_gb=""

    if [[ -n "${AXON_RESOURCE_POLICY_COMPUTED_INSTANCE:-}" && "$AXON_RESOURCE_POLICY_COMPUTED_INSTANCE" != "$instance_kind" ]]; then
        for scoped_var in \
            AXON_RESOURCE_PRIORITY \
            AXON_BACKGROUND_BUDGET_CLASS \
            AXON_GPU_ACCESS_POLICY \
            AXON_WATCHER_POLICY \
            AXON_QUEUE_MEMORY_BUDGET_BYTES \
            AXON_EMBEDDING_PROVIDER
        do
            local source_var="AXON_POLICY_SOURCE_${scoped_var}"
            if [[ "${!source_var:-}" == "policy_default" ]]; then
                unset "$scoped_var"
            fi
        done
    fi

    cpu_cores="$(axon_detect_host_cpu_cores)"
    ram_gb="$(axon_detect_host_ram_gb)"

    if axon_normalize_resource_priority "${AXON_RESOURCE_PRIORITY:-}" >/dev/null 2>&1; then
        export AXON_RESOURCE_PRIORITY
        AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY="explicit"
    else
        export AXON_RESOURCE_PRIORITY="$(axon_default_resource_priority "$instance_kind")"
        AXON_POLICY_SOURCE_AXON_RESOURCE_PRIORITY="policy_default"
    fi
    if axon_normalize_background_budget_class "${AXON_BACKGROUND_BUDGET_CLASS:-}" >/dev/null 2>&1; then
        export AXON_BACKGROUND_BUDGET_CLASS
        AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS="explicit"
    else
        export AXON_BACKGROUND_BUDGET_CLASS="$(axon_default_background_budget_class "$instance_kind")"
        AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS="policy_default"
    fi
    if axon_normalize_gpu_access_policy "${AXON_GPU_ACCESS_POLICY:-}" >/dev/null 2>&1; then
        export AXON_GPU_ACCESS_POLICY
        AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY="explicit"
    else
        export AXON_GPU_ACCESS_POLICY="$(axon_default_gpu_access_policy "$instance_kind")"
        AXON_POLICY_SOURCE_AXON_GPU_ACCESS_POLICY="policy_default"
    fi
    if axon_normalize_watcher_policy "${AXON_WATCHER_POLICY:-}" >/dev/null 2>&1; then
        export AXON_WATCHER_POLICY
        AXON_POLICY_SOURCE_AXON_WATCHER_POLICY="explicit"
    else
        export AXON_WATCHER_POLICY="$(axon_default_watcher_policy "$instance_kind")"
        AXON_POLICY_SOURCE_AXON_WATCHER_POLICY="policy_default"
    fi

    export AXON_RESOURCE_POLICY_CPU_CORES="$cpu_cores"
    export AXON_RESOURCE_POLICY_RAM_GB="$ram_gb"
    # REQ-AXO-902275 — `AXON_EFFECTIVE_MAX_AXON_WORKERS` et
    # `AXON_EFFECTIVE_WATCHER_SUBTREE_HINT_BUDGET` retirées avec les deux `MAX_*` /
    # `*_SUBTREE_HINT_BUDGET` qu'elles alimentaient : aucune n'avait de consommateur.
    # Le doublet EFFECTIVE/canonique reste pour le budget mémoire, où il a un sens —
    # il distingue ce que la politique RECOMMANDE de ce qui est EN VIGUEUR après
    # override opérateur.
    export AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES="$(
        axon_compute_queue_memory_budget_bytes "$AXON_BACKGROUND_BUDGET_CLASS" "$ram_gb"
    )"

    if [[ -z "${AXON_QUEUE_MEMORY_BUDGET_BYTES:-}" ]]; then
        export AXON_QUEUE_MEMORY_BUDGET_BYTES="$AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES"
        AXON_POLICY_SOURCE_AXON_QUEUE_MEMORY_BUDGET_BYTES="policy_default"
    fi

    # REQ-AXO-184 #1: avoid → cpu auto-coercion deleted. AXON_EMBEDDING_PROVIDER
    # is the canonical knob; the runtime (canonical_embedding_provider_request)
    # decides cpu vs cuda vs tensorrt based on detected GPU + runtime mode.
    # Operators wanting cpu must export AXON_EMBEDDING_PROVIDER=cpu explicitly.

    export AXON_RESOURCE_POLICY_COMPUTED_INSTANCE="$instance_kind"
}

# REQ-AXO-902267 — cargo build parallelism, bounded by AVAILABLE memory.
#
# Why this exists. On 2026-07-27 a promote made the whole laptop unresponsive: two global
# OOM kills (both took Chrome down) inside a single promote window. The primary cause was a
# fork storm since fixed (REQ-AXO-902266), but the host is structurally tight — 47 GB total
# yet swap sitting at 7/8 GB — so the release build is the remaining large consumer on the
# promote path. `cargo` defaults to one rustc per core (16 here); a release rustc on this
# crate is GB-scale, so an unbounded build can commit far more than the machine has free
# and push a busy host into swap thrashing.
#
# The existing helpers read TOTAL ram (`axon_detect_host_ram_gb`), which is the wrong
# quantity: what matters is what is free WHILE the promote runs, alongside the live runtime,
# Postgres, the indexer's GPU session and the operator's browser.
#
# Deliberately a floor of 1 and a cap of the core count: this may only ever REDUCE
# parallelism, never raise it above cargo's own default.

# axon_available_ram_gb — MemAvailable in GiB (the kernel's own estimate of what can be
# allocated without swapping), 0 when unreadable.
axon_available_ram_gb() {
    local kb
    kb="$(sed -n 's/^MemAvailable:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*kB$/\1/p' /proc/meminfo 2>/dev/null | head -n1)"
    [[ -n "$kb" ]] && printf '%s\n' "$(( kb / 1024 / 1024 ))" || printf '0\n'
}

# axon_swap_used_pct — percentage of swap in use, 0 when there is no swap or it is
# unreadable. Heavy swap use means the host is ALREADY struggling; adding 16 rustc
# processes to that is how a build takes the desktop down with it.
axon_swap_used_pct() {
    local total free
    total="$(sed -n 's/^SwapTotal:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*kB$/\1/p' /proc/meminfo 2>/dev/null | head -n1)"
    free="$(sed -n 's/^SwapFree:[[:space:]]*\([0-9][0-9]*\)[[:space:]]*kB$/\1/p' /proc/meminfo 2>/dev/null | head -n1)"
    [[ -n "$total" && -n "$free" && "$total" -gt 0 ]] || { printf '0\n'; return 0; }
    printf '%s\n' "$(( (total - free) * 100 / total ))"
}

# axon_compute_cargo_jobs <available_gb> <swap_used_pct> <cores> [gb_per_job]
#
# PURE (no /proc, no env) so the policy is unit-testable on any host.
#
# `gb_per_job` defaults to 3, and that 3 is MEASURED, not guessed. Sampling
# /proc/<pid>/status VmRSS once a second across a real `cargo build --release -j6` of this
# crate (154 samples):  peak 2.13 GB · mean 1.04 GB.
#
# The budget tracks the PEAK rounded up, not the mean, because an OOM is caused by
# SIMULTANEOUS peaks — betting on the mean assumes rustc processes never peak together, and
# losing that bet costs the operator's whole session (two global OOM kills on 2026-07-27,
# Chrome killed twice). Erring high only costs a slower build.
#
# The first version of this used 2 as an ADMITTED ESTIMATE; the measurement showed 2 sat
# BELOW the observed peak, i.e. it under-budgeted. Re-sample if the crate or the toolchain
# changes materially.
#
# Halve again past 50 % swap: at that point the kernel is already evicting, and the next
# allocation storm is what triggers the OOM killer.
axon_compute_cargo_jobs() {
    local available_gb="${1:-0}" swap_pct="${2:-0}" cores="${3:-4}" gb_per_job="${4:-3}"
    [[ "$gb_per_job" -ge 1 ]] || gb_per_job=1
    local jobs=$(( available_gb / gb_per_job ))
    (( swap_pct >= 50 )) && jobs=$(( jobs / 2 ))
    (( jobs < 1 )) && jobs=1
    (( jobs > cores )) && jobs="$cores"
    printf '%s\n' "$jobs"
}

# axon_resolve_cargo_jobs — the host-reading wrapper. Honours an explicit
# AXON_BUILD_JOBS override (operator wins, always).
axon_resolve_cargo_jobs() {
    if [[ -n "${AXON_BUILD_JOBS:-}" && "${AXON_BUILD_JOBS}" =~ ^[0-9]+$ && "${AXON_BUILD_JOBS}" -ge 1 ]]; then
        printf '%s\n' "$AXON_BUILD_JOBS"
        return 0
    fi
    axon_compute_cargo_jobs \
        "$(axon_available_ram_gb)" \
        "$(axon_swap_used_pct)" \
        "$(axon_detect_host_cpu_cores)" \
        "${AXON_BUILD_GB_PER_JOB:-3}"
}

# ---------------------------------------------------------------------------
# REQ-AXO-902273 — is this host quiet enough for a MEASUREMENT to mean anything?
#
# Distinct from the sizing policy above, and the difference is the whole point. The
# functions above answer "how big a build can this host take"; this one answers "can I
# BELIEVE a number I measure right now". A host can be perfectly capable of running a
# build while being far too busy for its timings to mean anything.
#
# Why it exists (session 107, and it cost most of a session). A full `--lib` run was
# timed on a host whose only checked precondition — `pgrep -c rustc` — read ZERO, i.e.
# the documented gate said GO. It was in fact at load 76 with 20 kB of 8 GB swap free,
# saturated by THIRD-PARTY processes (a typedb server at 321 % CPU, a python3.11 at
# 100 %). The suite passed 594 tests in its first minutes and 7 in its last ten. The
# conclusion nearly drawn from that — "my change makes the suite unusable" — would have
# been false, and would have thrown away a correct fix.
#
# The lesson is not "also look at the load". It is that a precondition covering ONE
# family of processes silently ignores every other one. `rustc` was the family we had
# been burned by (a sibling repo's pre-push hook), so it became the check; nothing else
# did. Load average and swap are process-agnostic: they cannot miss a consumer because
# they never enumerate consumers.

# axon_load_avg_1m — 1-minute load average, integer (rounded down). 0 when unreadable.
axon_load_avg_1m() {
    local raw
    raw="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null)"
    [[ "$raw" =~ ^([0-9]+) ]] && printf '%s\n' "${BASH_REMATCH[1]}" || printf '0\n'
}

# axon_measurement_readiness <load1> <cores> <swap_used_pct> <rustc_count>
#
# PURE (no /proc, no env) so the policy is unit-testable on any host. Prints one line:
#   quiet
#   busy:<reason>[,<reason>...]
#
# Thresholds, and why each is where it is:
#   * load > 2 x cores — the run queue is more than double what the machine can serve, so
#     wall-clock timings measure the queue, not the code. Two-times rather than one-times
#     because a healthy build legitimately saturates every core.
#   * swap used >= 90 % — the kernel is out of room to evict into; the next allocation
#     stalls on I/O. Observed at 99.9 % while `MemAvailable` still read 18 GB, which is
#     why free RAM alone is NOT a sufficient signal.
#   * any foreign rustc — kept from the original gate: a sibling repo's pre-push hook
#     spawns one rustc per test case (29 observed simultaneously).
axon_measurement_readiness() {
    local load1="${1:-0}" cores="${2:-1}" swap_pct="${3:-0}" rustc="${4:-0}"
    [[ "$load1" =~ ^[0-9]+$ ]] || load1=0
    [[ "$cores" =~ ^[0-9]+$ ]] && (( cores >= 1 )) || cores=1
    [[ "$swap_pct" =~ ^[0-9]+$ ]] || swap_pct=0
    [[ "$rustc" =~ ^[0-9]+$ ]] || rustc=0

    local -a reasons=()
    (( load1 > cores * 2 )) && reasons+=( "load=${load1}>2x${cores}cores" )
    (( swap_pct >= 90 ))    && reasons+=( "swap=${swap_pct}%" )
    (( rustc > 0 ))         && reasons+=( "rustc=${rustc}" )

    if (( ${#reasons[@]} == 0 )); then
        printf 'quiet\n'
        return 0
    fi
    local IFS=,
    printf 'busy:%s\n' "${reasons[*]}"
}

# axon_host_measurement_verdict — the host-reading wrapper. Prints the same one-line
# verdict; returns 0 when quiet, 1 when busy, so a caller can gate on it directly.
axon_host_measurement_verdict() {
    local verdict
    verdict="$(axon_measurement_readiness \
        "$(axon_load_avg_1m)" \
        "$(axon_detect_host_cpu_cores)" \
        "$(axon_swap_used_pct)" \
        "$(pgrep -c rustc 2>/dev/null || printf '0')")"
    printf '%s\n' "$verdict"
    [[ "$verdict" == "quiet" ]]
}

# axon_warn_unless_host_quiet <what> — NOISY advisory, never a blocker.
#
# Deliberately does not exit: refusing to run would be its own failure mode (an operator
# who cannot measure when they need to will simply bypass the check, and then it protects
# nothing). It states the verdict so a number produced on a busy host is never read later
# as if it had been taken on a quiet one.
axon_warn_unless_host_quiet() {
    local what="${1:-this measurement}" verdict
    verdict="$(axon_host_measurement_verdict)" && return 0
    printf '⚠️  HOST NOT QUIET (%s) — %s may be unreliable.\n' "$verdict" "$what" >&2
    printf '    Timings taken now measure host contention as much as the code.\n' >&2
    printf '    Top consumers: %s\n' \
        "$(ps -eo pcpu,comm --sort=-pcpu 2>/dev/null | sed -n '2,4p' | awk '{printf "%s(%s%%) ", $2, $1}')" >&2
    return 1
}
