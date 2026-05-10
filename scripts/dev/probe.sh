#!/usr/bin/env bash
# REQ-AXO-175 — Dev probe harness for fast Pipeline 2 measurement.
#
# Replaces the stop/start/observe/parse loop (~5 min/iteration) with a
# single command that auto-rebuilds the debug binary if stale, sets the
# isolation scope via AXON_WATCH_DIR (REQ-AXO-172), samples the heartbeat
# every 5s, and emits a CSV trace + a 3-line summary.
#
# Targets ~30s smoke iterations and ~90s medium-corpus measurements
# without polluting the LLM context with verbose tmux logs.
#
# Usage:
#   scripts/dev/probe.sh --scope <path> --duration <sec> [--fresh]
#                        [--workers <n>] [--tag <name>] [--no-stop]
#
# REQ-AXO-271 slice 6 (2026-05-10): PG is the only supported backend.
# --postgres flag is accepted as a no-op for backwards compatibility
# and will be deleted once existing CI invocations stop passing it.
#
# Output: dev-probe-<tag>-<UTC>.csv with columns
#   t_seconds,files,chunks_total,chunks_per_sec,ready_queue,
#   claim_mode,gpu_mb,zombies,provider

set -euo pipefail

SCOPE=""
DURATION=60
FRESH=0
WORKERS=""
TAG=""
NO_STOP=0
SAMPLE_INTERVAL=5

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scope) SCOPE="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --fresh) FRESH=1; shift ;;
        --workers) WORKERS="$2"; shift 2 ;;
        --tag) TAG="$2"; shift 2 ;;
        --no-stop) NO_STOP=1; shift ;;
        --interval) SAMPLE_INTERVAL="$2"; shift 2 ;;
        # REQ-AXO-271 slice 6 (2026-05-10): --postgres flag retired —
        # PG is the only supported backend. The flag is accepted as a
        # no-op for one release window so existing CI invocations don't
        # break; it will be deleted entirely once that window closes.
        --postgres) shift ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -25
            exit 0
            ;;
        *) echo "❌ unknown arg: $1" >&2; exit 2 ;;
    esac
done

if [[ -z "$SCOPE" ]]; then
    echo "❌ missing --scope <path>" >&2; exit 2
fi
if [[ -z "$TAG" ]]; then
    TAG="$(basename "$SCOPE")"
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# Locate duckdb once (Nix-store path may move across rebuilds — try $PATH first)
DUCKDB="$(command -v duckdb 2>/dev/null || true)"
if [[ -z "$DUCKDB" ]]; then
    DUCKDB="$(find /nix/store -maxdepth 3 -name duckdb -type f 2>/dev/null | head -1 || true)"
fi

# Auto-rebuild debug binary if any .rs is newer than the binary
BIN=".axon/cargo-target/debug/axon-indexer"
NEEDS_REBUILD=0
if [[ ! -f "$BIN" ]]; then
    NEEDS_REBUILD=1
elif find src/axon-core/src -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null | grep -q .; then
    NEEDS_REBUILD=1
fi
if [[ "$NEEDS_REBUILD" == "1" ]]; then
    echo "🔨 Rebuilding debug binary (stale or missing)..."
    CARGO_TARGET_DIR=.axon/cargo-target cargo build \
        --manifest-path src/axon-core/Cargo.toml --bin axon-indexer
fi

# Wipe IST if --fresh (must stop dev first to release locks)
if [[ "$FRESH" == "1" ]]; then
    ./scripts/axon-dev stop --hard >/dev/null 2>&1 || true
    rm -rf .axon-dev/graph_v2
    # Also wipe the run-indexer dir so the heartbeat and traces from a
    # previous probe don't pollute the early-window samples (chunks_total
    # would otherwise read the previous run's accumulator until the new
    # indexer overwrites the heartbeat file).
    rm -rf .axon-dev/run-indexer
    echo "🧹 Wiped dev IST and run-indexer"
fi

# Start dev only if not already running with the desired scope
DEV_PID="$(pgrep -f '[c]argo-target/debug/axon-indexer' | head -1 || true)"
if [[ -z "$DEV_PID" ]]; then
    # REQ-AXO-271 slice 6 (2026-05-10): PG is the only supported backend.
    # AXON_DB_BACKEND=postgres is always set; AXON_LIVE_DATABASE_URL
    # (or AXON_DEV_DATABASE_URL) must be exported by the shell — devenv
    # exports both by default.
    if [[ -z "${AXON_LIVE_DATABASE_URL:-}" && -z "${AXON_DEV_DATABASE_URL:-}" ]]; then
        echo "❌ probe.sh requires AXON_LIVE_DATABASE_URL or AXON_DEV_DATABASE_URL exported (run inside devenv shell)" >&2
        exit 2
    fi
    EXPORTS=(
        "AXON_WATCH_DIR=$SCOPE"
        "AXON_PROJECTS_ROOT=$SCOPE"
        "AXON_DB_BACKEND=postgres"
    )
    if [[ -n "$WORKERS" ]]; then
        EXPORTS+=("AXON_VECTOR_WORKERS=$WORKERS")
    fi
    # Default to --tensorrt: the CUDA EP path falls back to a 30+ min
    # nixpkgs build if the local manifest is stale, which silently kills
    # bench runs (operator burned a session on this 2026-05-08). The
    # TensorRT artifact is materialised once via scripts/lib/axon-ort-runtime.sh
    # and reused. Set AXON_GPU_EMBED_SERVICE_TENSORRT=0 explicitly to
    # opt out (e.g. to compare CUDA EP vs TensorRT EP).
    # Skip the Elixir prewarm + dashboard for indexer-only throughput
    # benches: nothing in probe.sh consumes them, and prewarm/dashboard
    # boot can take minutes that count against the 90s heartbeat wait.
    START_FLAGS=("--indexer-full" "--skip-elixir-prewarm" "--no-dashboard")
    if [[ "${AXON_GPU_EMBED_SERVICE_TENSORRT:-1}" =~ ^(1|true|yes|on)$ ]]; then
        START_FLAGS+=("--tensorrt")
    fi
    echo "🚀 Starting dev with scope=$SCOPE${WORKERS:+ workers=$WORKERS} flags=${START_FLAGS[*]}..."
    env "${EXPORTS[@]}" ./scripts/axon-dev start "${START_FLAGS[@]}" \
        > /tmp/probe-start.log 2>&1
fi

# Wait for heartbeat (max 90s)
HB_PATH=".axon-dev/run-indexer/runtime-heartbeat.json"
for _ in $(seq 1 90); do
    [[ -f "$HB_PATH" ]] && break
    sleep 1
done
if [[ ! -f "$HB_PATH" ]]; then
    echo "❌ Heartbeat never appeared at $HB_PATH" >&2
    cat /tmp/probe-start.log >&2 || true
    exit 1
fi

DEV_PID="$(pgrep -f '[c]argo-target/debug/axon-indexer' | head -1)"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="dev-probe-${TAG}-${TS}.csv"
echo "t_seconds,files,chunks_total,chunks_per_sec,ready_queue,claim_mode,gpu_mb,zombies,provider" > "$OUT"

START="$(date +%s)"
while :; do
    NOW="$(date +%s)"
    T=$((NOW - START))
    [[ "$T" -ge "$DURATION" ]] && break

    # Heartbeat extract via python3 for robustness against missing keys.
    # REQ-AXO-184 #3 / REQ-AXO-185 #3: provider column reads embedder_provider.effective
    # from the heartbeat (top-level object) and surfaces "<requested>->fallback:<effective>"
    # when the worker silently fell back from the requested provider, instead of "unknown".
    HB="$(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
    rtt = d.get("runtime_telemetry", {})
    embedder = d.get("embedder_provider") or {}
    requested = embedder.get("requested")
    effective = embedder.get("effective")
    if effective is None:
        provider = "unknown"
    elif requested and requested != effective:
        provider = f"{requested}->fallback:{effective}"
    else:
        provider = effective
    print(",".join([
        str(rtt.get("vector_chunks_embedded_total", 0)),
        str(rtt.get("chunk_embeddings_per_second", 0)),
        str(rtt.get("ready_queue_chunks_current", 0)),
        str(rtt.get("claim_mode", "unknown")),
        provider,
    ]))
except Exception:
    print("0,0,0,err,err")
' "$HB_PATH")"

    GPU="$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits 2>/dev/null | head -1 | tr -d ' ' || echo 0)"
    ZOMBIES="$(ps -ef | awk -v p="$DEV_PID" '$3 == p && /<defunct>/' | wc -l | tr -d ' ')"

    FILES=0
    if [[ -n "$DUCKDB" ]] && [[ -f .axon-dev/graph_v2/ist-reader.db ]]; then
        FILES="$("$DUCKDB" .axon-dev/graph_v2/ist-reader.db -noheader -csv \
            'SELECT COUNT(*) FROM main.File' 2>/dev/null | tail -1 || echo 0)"
    fi

    # Split HB into total,rate,queue,claim,provider
    IFS=',' read -r CHUNKS RATE QUEUE CLAIM PROVIDER <<< "$HB"

    echo "$T,$FILES,$CHUNKS,$RATE,$QUEUE,$CLAIM,$GPU,$ZOMBIES,$PROVIDER" >> "$OUT"
    sleep "$SAMPLE_INTERVAL"
done

# Compact summary: first / mid / last sample + delta
SUMMARY="$(awk -F',' 'NR==1 {next} {n=NR-1; if (n==1) first=$0; mid=$0; last=$0} END {print first; print last}' "$OUT")"

echo
echo "📊 Probe: $OUT (scope=$SCOPE, duration=${DURATION}s)"
echo "   first sample:"
echo "   $(echo "$SUMMARY" | head -1)"
echo "   final sample:"
echo "   $(echo "$SUMMARY" | tail -1)"
echo "   columns: t_seconds,files,chunks_total,chunks_per_sec,ready_queue,claim_mode,gpu_mb,zombies,provider"

if [[ "$NO_STOP" != "1" ]]; then
    ./scripts/axon-dev stop --hard >/dev/null 2>&1 || true
    echo "🛑 Dev stopped"
else
    echo "ℹ️  Dev left running (--no-stop)"
fi
