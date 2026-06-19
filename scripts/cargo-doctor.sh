#!/usr/bin/env bash
# REQ-AXO-902010 — cargo target self-heal + size guard.
#
# RCA (GUI-PRO-106): the recurring `E0786: found invalid metadata files for
# crate tokenizers` (+ ~1600 E0282 inference cascades) is NOT a source bug. Its
# root is an INTERRUPTED build (timeout / OOM-kill / Ctrl-C) that dies while
# rustc is writing a large crate's `.rmeta`/`.rlib`, leaving a TRUNCATED file.
# The next `cargo` invocation reads that partial metadata and fails cryptically.
# tokenizers is the usual victim because it is large (long write window) and
# deep in the graph (its types feed embedder/gpu_backend → the E0282 cascade).
# A bloated target dir (the 185 GB seen 2026-06-16) widens the window further.
#
# We cannot prevent every interruption, so we make the failure self-healing
# (GUI-PRO-008 design-for-failure): detect truncated artifacts and surgically
# `cargo clean -p <crate>` only the affected crates, so the next build is
# deterministic (GUI-PRO-006) instead of red. `--gc` additionally caps the
# target dir. Run it standalone, from CI, or before a build.
#
# Usage:
#   scripts/cargo-doctor.sh                # heal corrupt artifacts, report size
#   scripts/cargo-doctor.sh --gc           # also `cargo clean` if over the cap
#   scripts/cargo-doctor.sh --gc-cap 120   # cap in GiB (default 100)
#   scripts/cargo-doctor.sh --dry-run      # report only, change nothing
# Exit: 0 = nothing to heal · 1 = healed/cleaned · 2 = usage/env error.

set -euo pipefail

MANIFEST="${CARGO_DOCTOR_MANIFEST:-src/axon-core/Cargo.toml}"
TARGET_DIR="${CARGO_TARGET_DIR:-.axon/cargo-target}"
# A valid .rmeta is a rust archive with a header; a truncated one is far smaller
# than any real crate metadata. 512 bytes is a safe floor (smallest real proc-
# macro .rmeta observed is multiple KiB) that never false-positives a good file.
TRUNCATED_BYTES="${CARGO_DOCTOR_TRUNCATED_BYTES:-512}"
GC_CAP_GIB=100
DO_GC=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --gc)      DO_GC=1; shift ;;
        --gc-cap)  GC_CAP_GIB="$2"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "cargo-doctor: unknown arg '$1'" >&2; exit 2 ;;
    esac
done

if [[ ! -d "$TARGET_DIR" ]]; then
    echo "cargo-doctor: target dir '$TARGET_DIR' absent — nothing to heal."
    exit 0
fi

# 1. Find truncated .rmeta / .rlib (zero-size or below the floor) = the
#    interrupted-build fingerprint. Map each back to its crate name from the
#    canonical `lib<crate>-<hash>.{rmeta,rlib}` artifact filename.
mapfile -t corrupt < <(
    find "$TARGET_DIR" \( -name '*.rmeta' -o -name '*.rlib' \) -size "-${TRUNCATED_BYTES}c" 2>/dev/null || true
)

declare -A crates=()
for f in "${corrupt[@]:-}"; do
    [[ -z "$f" ]] && continue
    base="$(basename "$f")"
    # lib<crate>-<hash>.rmeta → <crate> (crate may contain '-' → strip the
    # trailing -<hash> only).
    name="${base#lib}"
    name="${name%.*}"          # drop extension
    name="${name%-*}"          # drop -<hash>
    [[ -n "$name" ]] && crates["$name"]=1
done

healed=0
if [[ ${#crates[@]} -gt 0 ]]; then
    echo "cargo-doctor: ${#corrupt[@]} truncated artifact(s) → crates: ${!crates[*]}"
    for crate in "${!crates[@]}"; do
        if [[ "$DRY_RUN" == 1 ]]; then
            echo "  [dry-run] would: cargo clean -p $crate"
        else
            echo "  healing: cargo clean -p $crate"
            cargo clean --manifest-path "$MANIFEST" -p "$crate" 2>/dev/null \
                || echo "  warn: cargo clean -p $crate failed (crate name not in graph?)" >&2
        fi
        healed=1
    done
else
    echo "cargo-doctor: no truncated artifacts — metadata healthy."
fi

# 2. Size guard. Report always; GC when over the cap and --gc is set.
size_kib="$(du -sk "$TARGET_DIR" 2>/dev/null | cut -f1)"
size_gib=$(( size_kib / 1024 / 1024 ))
echo "cargo-doctor: target dir '$TARGET_DIR' = ${size_gib} GiB (cap ${GC_CAP_GIB} GiB)"
if [[ "$size_gib" -gt "$GC_CAP_GIB" ]]; then
    if [[ "$DO_GC" == 1 && "$DRY_RUN" == 0 ]]; then
        echo "  over cap → cargo clean (full); next build is a cold rebuild."
        cargo clean --manifest-path "$MANIFEST"
        healed=1
    else
        echo "  over cap → run with --gc to reclaim (or 'cargo clean')." >&2
    fi
fi

exit $(( healed ? 1 : 0 ))
