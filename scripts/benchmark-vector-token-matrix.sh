#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

duration="20"
interval="5"
label_prefix="vector-token-matrix"
tokens_csv="4096,8192,16384,32768"
max_items="128"
max_batch_bytes="$((8 * 1024 * 1024))"
max_vram_used_mb="2048"
gpu_primary_worker_max_used_mb=""
graph_workers="2"
run_mode="cold"
warmup_timeout="120"
warmup_poll_interval="2"
warmup_settle_seconds="0"
prepare_workers=""
ready_depth=""
pipeline_depth=""
gpu_backend="cuda"
artifact_manifest=""

usage() {
    cat <<'EOF'
Usage: bash scripts/benchmark-vector-token-matrix.sh [options]

Run an indexer-only token-cap benchmark matrix by overriding runtime env vars,
resetting the dev indexer baseline, and qualifying each run.

Options:
  --tokens CSV           Comma-separated token caps (default: 4096,8192,16384,32768)
  --duration N           Qualification duration per step (default: 20)
  --interval N           Qualification interval per step (default: 5)
  --label-prefix NAME    Qualification label prefix (default: vector-token-matrix)
  --max-items N          AXON_EMBED_MICRO_BATCH_MAX_ITEMS override (default: 128)
  --max-batch-bytes N    AXON_MAX_EMBED_BATCH_BYTES override (default: 8388608)
  --graph-workers N      AXON_GRAPH_WORKERS override (default: 2)
  --max-vram-used-mb N   VRAM operator budget in MB (default: 2048)
  --gpu-admission-vram-used-mb N  Max VRAM already used before launching a GPU batch
  --mode MODE            Benchmark mode: cold|warm (default: cold)
  --warmup-timeout N     Warm-mode timeout in seconds waiting for vector activity (default: 120)
  --warmup-poll N        Warm-mode poll interval seconds (default: 2)
  --warmup-settle N      Extra settle seconds after warm signal (default: 0)
  --prepare-workers N    Optional AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR override
  --ready-depth N        Optional AXON_VECTOR_READY_QUEUE_DEPTH override
  --pipeline-depth N     Optional AXON_VECTOR_PREPARE_PIPELINE_DEPTH override
  --gpu-backend NAME     GPU backend: cuda|tensorrt (default: cuda)
  --manifest PATH        Optional explicit ORT artifact manifest path
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tokens) tokens_csv="$2"; shift 2 ;;
        --tokens=*) tokens_csv="${1#*=}"; shift ;;
        --duration) duration="$2"; shift 2 ;;
        --duration=*) duration="${1#*=}"; shift ;;
        --interval) interval="$2"; shift 2 ;;
        --interval=*) interval="${1#*=}"; shift ;;
        --label-prefix) label_prefix="$2"; shift 2 ;;
        --label-prefix=*) label_prefix="${1#*=}"; shift ;;
        --max-items) max_items="$2"; shift 2 ;;
        --max-items=*) max_items="${1#*=}"; shift ;;
        --max-batch-bytes) max_batch_bytes="$2"; shift 2 ;;
        --max-batch-bytes=*) max_batch_bytes="${1#*=}"; shift ;;
        --graph-workers) graph_workers="$2"; shift 2 ;;
        --graph-workers=*) graph_workers="${1#*=}"; shift ;;
        --max-vram-used-mb) max_vram_used_mb="$2"; shift 2 ;;
        --max-vram-used-mb=*) max_vram_used_mb="${1#*=}"; shift ;;
        --gpu-admission-vram-used-mb) gpu_primary_worker_max_used_mb="$2"; shift 2 ;;
        --gpu-admission-vram-used-mb=*) gpu_primary_worker_max_used_mb="${1#*=}"; shift ;;
        --mode) run_mode="$2"; shift 2 ;;
        --mode=*) run_mode="${1#*=}"; shift ;;
        --warmup-timeout) warmup_timeout="$2"; shift 2 ;;
        --warmup-timeout=*) warmup_timeout="${1#*=}"; shift ;;
        --warmup-poll) warmup_poll_interval="$2"; shift 2 ;;
        --warmup-poll=*) warmup_poll_interval="${1#*=}"; shift ;;
        --warmup-settle) warmup_settle_seconds="$2"; shift 2 ;;
        --warmup-settle=*) warmup_settle_seconds="${1#*=}"; shift ;;
        --prepare-workers) prepare_workers="$2"; shift 2 ;;
        --prepare-workers=*) prepare_workers="${1#*=}"; shift ;;
        --ready-depth) ready_depth="$2"; shift 2 ;;
        --ready-depth=*) ready_depth="${1#*=}"; shift ;;
        --pipeline-depth) pipeline_depth="$2"; shift 2 ;;
        --pipeline-depth=*) pipeline_depth="${1#*=}"; shift ;;
        --gpu-backend) gpu_backend="$2"; shift 2 ;;
        --gpu-backend=*) gpu_backend="${1#*=}"; shift ;;
        --manifest) artifact_manifest="$2"; shift 2 ;;
        --manifest=*) artifact_manifest="${1#*=}"; shift ;;
        --help|-h) usage; exit 0 ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

IFS=',' read -r -a token_caps <<<"$tokens_csv"
if [[ "${#token_caps[@]}" -eq 0 ]]; then
    echo "At least one token cap is required" >&2
    exit 1
fi

if [[ -z "$gpu_primary_worker_max_used_mb" ]]; then
    gpu_primary_worker_max_used_mb="$(( max_vram_used_mb - (( max_vram_used_mb / 10 > 512 ? max_vram_used_mb / 10 : 512 )) ))"
fi

case "$run_mode" in
    cold|warm) ;;
    *)
        echo "Unsupported --mode: $run_mode (expected cold or warm)" >&2
        exit 1
        ;;
esac

case "$gpu_backend" in
    cuda|tensorrt) ;;
    *)
        echo "Unsupported --gpu-backend: $gpu_backend (expected cuda or tensorrt)" >&2
        exit 1
        ;;
esac

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
result_dir="$PROJECT_ROOT/.axon/benchmarks/${timestamp}-${label_prefix}"
mkdir -p "$result_dir"
results_tsv="$result_dir/results.tsv"

printf 'label\tmode\tgpu_backend\ttokens\tready_depth\tpipeline_depth\tprepare_workers\tmax_items\tmax_batch_bytes\tmax_vram_used_mb_budget\tgpu_admission_vram_used_mb\tgraph_workers\tmax_chunk_embeddings_per_second\twindow_chunk_delta\twindow_chunks_per_second\tavg_ready_queue_chunks_at_gpu_start\tavg_prepare_inflight_chunks_at_gpu_start\tmax_gpu_used_mb\tavg_gpu_used_mb\tvram_budget_exceeded\tmax_graph_projection_queue_runtime_inflight\tmax_graph_workers_active_current\tdominant_bottleneck\tsummary_path\n' > "$results_tsv"

wait_for_warm_vector_window() {
    local timeout_s="$1"
    local poll_s="$2"
    local settle_s="$3"
    local heartbeat_path="$PROJECT_ROOT/.axon-dev/run-indexer/runtime-heartbeat.json"
    local deadline=$((SECONDS + timeout_s))
    local state=""

    echo "[benchmark-vector-token-matrix] waiting for warm vector activity (timeout=${timeout_s}s)"
    while (( SECONDS < deadline )); do
        state="$(
            python3 - "$heartbeat_path" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
if not path.exists():
    print("missing\t0.0\t0\t0\t0")
    raise SystemExit(1)

try:
    payload = json.loads(path.read_text())
except Exception:
    print("invalid\t0.0\t0\t0\t0")
    raise SystemExit(1)

telemetry = payload.get("runtime_telemetry", {})
if not isinstance(telemetry, dict):
    telemetry = {}
graph_queue = telemetry.get("graph_projection_queue")
if not isinstance(graph_queue, dict):
    graph_queue = {}

chunk_rate = float(telemetry.get("chunk_embeddings_per_second") or 0.0)
ready_chunks = int(telemetry.get("ready_queue_chunks_current") or 0)
prepare_chunks = int(telemetry.get("prepare_inflight_chunks_current") or 0)
graph_workers = int(telemetry.get("graph_workers_active_current") or 0)
graph_inflight = int(graph_queue.get("inflight") or 0)
ready = graph_workers > 0 and (chunk_rate > 0.0 or ready_chunks > 0 or prepare_chunks > 0)
print(
    f"{'ready' if ready else 'cold'}\t{chunk_rate}\t{ready_chunks + prepare_chunks}\t{graph_workers}\t{graph_inflight}"
)
raise SystemExit(0 if ready else 1)
PY
        )" || true

        if [[ -n "$state" ]]; then
            IFS=$'\t' read -r warm_state chunk_rate vector_total graph_workers_active graph_inflight <<<"$state"
            echo "[benchmark-vector-token-matrix] warm-state=${warm_state} chunk_rate=${chunk_rate} vector_total=${vector_total} graph_workers=${graph_workers_active} graph_inflight=${graph_inflight}"
            if [[ "$warm_state" == "ready" ]]; then
                if (( settle_s > 0 )); then
                    sleep "$settle_s"
                fi
                return 0
            fi
        fi
        sleep "$poll_s"
    done

    echo "Warm vector activity not observed within ${timeout_s}s" >&2
    return 1
}

run_step() {
    local tokens="$1"
    local label="${label_prefix}-t${tokens}"
    local summary_path=""
    local latest_run_dir=""
    echo "[benchmark-vector-token-matrix] running ${label}"
    (
        export AXON_INSTANCE_KIND=dev
        export AXON_BENCHMARK_ACTIVE=1
        export AXON_GPU_EMBED_SERVICE_ENABLED=1
        export AXON_GPU_TELEMETRY_BACKEND="${AXON_GPU_TELEMETRY_BACKEND:-nvml}"
        export AXON_NVML_LIBRARY_PATH="${AXON_NVML_LIBRARY_PATH:-/usr/lib/wsl/lib/libnvidia-ml.so.1}"
        export AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS="$tokens"
        export AXON_EMBED_MICRO_BATCH_MAX_ITEMS="$max_items"
        export AXON_MAX_EMBED_BATCH_BYTES="$max_batch_bytes"
        export AXON_OPT_MAX_VRAM_USED_MB="$max_vram_used_mb"
        export AXON_CUDA_MEMORY_SOFT_LIMIT_MB="$max_vram_used_mb"
        export AXON_CUDA_MEMORY_LIMIT_MB="$(( max_vram_used_mb > 1024 ? max_vram_used_mb - 1024 : max_vram_used_mb ))"
        export AXON_GPU_PRIMARY_WORKER_MAX_USED_MB="$gpu_primary_worker_max_used_mb"
        export AXON_GPU_TELEMETRY_CACHE_TTL_MS="250"
        export AXON_TENSORRT_OVERSHOOT_MB="${AXON_TENSORRT_OVERSHOOT_MB:-7900}"
        export AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT="${AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT:-1}"
        export AXON_GRAPH_WORKERS="$graph_workers"
        if [[ "$gpu_backend" == "tensorrt" ]]; then
            export AXON_GPU_EMBED_SERVICE_TENSORRT=1
            if [[ -n "$artifact_manifest" ]]; then
                export AXON_ORT_ARTIFACT_MANIFEST="$artifact_manifest"
            fi
        else
            export AXON_GPU_EMBED_SERVICE_TENSORRT=0
            unset AXON_ORT_ARTIFACT_MANIFEST || true
        fi
        if [[ -n "$prepare_workers" ]]; then
            export AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR="$prepare_workers"
        fi
        if [[ -n "$ready_depth" ]]; then
            export AXON_VECTOR_READY_QUEUE_DEPTH="$ready_depth"
        fi
        if [[ -n "$pipeline_depth" ]]; then
            export AXON_VECTOR_PREPARE_PIPELINE_DEPTH="$pipeline_depth"
        fi
        if [[ "$run_mode" == "cold" ]]; then
            # REQ-AXO-113 — fold into unified qualify entry
            python3 "$SCRIPT_DIR/qualify_runtime.py" \
                --instance dev \
                --profile ingestion \
                --mode indexer_full \
                --cold \
                --duration "$duration" \
                --interval "$interval" \
                --label "$label"
        else
            # Warm path: reuse the already-running indexer baseline. The reset
            # was previously delegated to scripts/reset-dev-indexer-baseline.sh
            # (deleted by REQ-AXO-113 Phase 4); we now invoke the dev-baseline
            # library functions directly since the warm path also needs the
            # subsequent wait_for_warm_vector_window step before qualify.
            (
                # shellcheck source=scripts/lib/dev-baseline.sh
                source "$SCRIPT_DIR/lib/dev-baseline.sh"
                dev_baseline_require_dev_instance
                dev_baseline_stop_split
                dev_baseline_clean_state
                AXON_INSTANCE_KIND=dev bash "$SCRIPT_DIR/lib/start-indexer.sh"
                dev_baseline_wait_for_indexer_measurement_window 240
            )
            wait_for_warm_vector_window "$warmup_timeout" "$warmup_poll_interval" "$warmup_settle_seconds"
            python3 "$SCRIPT_DIR/qualify_ingestion_run.py" \
                --reuse-runtime \
                --no-reset-ist \
                --mode indexer_full \
                --duration "$duration" \
                --interval "$interval" \
                --label "$label"
        fi
    )

    latest_run_dir="$(find "$PROJECT_ROOT/.axon/qualification-runs" -maxdepth 1 -type d -name "*-${label}" | sort | tail -n 1)"
    if [[ -z "$latest_run_dir" ]]; then
        echo "Failed to locate run directory for label ${label}" >&2
        return 1
    fi
    summary_path="$latest_run_dir/summary.json"
    if [[ ! -f "$summary_path" ]]; then
        echo "Missing summary: $summary_path" >&2
        return 1
    fi

    python3 - "$summary_path" "$latest_run_dir/samples.ndjson" "$label" "$run_mode" "$gpu_backend" "$tokens" "${ready_depth:-default}" "${pipeline_depth:-default}" "${prepare_workers:-default}" "$max_items" "$max_batch_bytes" "$max_vram_used_mb" "$gpu_primary_worker_max_used_mb" "$graph_workers" "$PROJECT_ROOT/.axon-dev/run/benchmark.sqlite3" >> "$results_tsv" <<'PY'
import json
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path

summary_path = Path(sys.argv[1])
samples_path = Path(sys.argv[2])
label = sys.argv[3]
mode = sys.argv[4]
gpu_backend = sys.argv[5]
tokens = sys.argv[6]
ready_depth = sys.argv[7]
pipeline_depth = sys.argv[8]
prepare_workers = sys.argv[9]
max_items = sys.argv[10]
max_batch_bytes = sys.argv[11]
max_vram_used_mb_budget = int(sys.argv[12])
gpu_admission_vram_used_mb = int(sys.argv[13])
graph_workers = sys.argv[14]
benchmark_db_path = Path(sys.argv[15])
summary = json.loads(summary_path.read_text())

def parse_timestamp_ms(value: str | None) -> int:
    if not value:
        return 0
    normalized = value.replace("Z", "+00:00")
    return int(datetime.fromisoformat(normalized).timestamp() * 1000)

window_chunk_delta = 0
window_chunks_per_second = 0.0
samples = []
if samples_path.exists():
    for line in samples_path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            samples.append(json.loads(line))
        except json.JSONDecodeError:
            continue

vector_points = []
sample_window_start_ms = 0
sample_window_end_ms = 0
for sample in samples:
    cockpit = sample.get("cockpit", {})
    if not isinstance(cockpit, dict):
        continue
    sample_timestamp_ms = parse_timestamp_ms(sample.get("timestamp"))
    if sample_timestamp_ms > 0:
        if sample_window_start_ms == 0:
            sample_window_start_ms = sample_timestamp_ms
        sample_window_end_ms = sample_timestamp_ms
    elapsed = sample.get("elapsed_seconds")
    total = cockpit.get("vector_chunks_embedded_total")
    if isinstance(elapsed, (int, float)) and isinstance(total, (int, float)):
        vector_points.append((float(elapsed), int(total)))

if len(vector_points) >= 2:
    start_elapsed, start_total = vector_points[0]
    end_elapsed, end_total = vector_points[-1]
    elapsed_window = max(0.0, end_elapsed - start_elapsed)
    window_chunk_delta = max(0, end_total - start_total)
    if elapsed_window > 0:
        window_chunks_per_second = window_chunk_delta / elapsed_window

if sample_window_start_ms == 0:
    sample_window_start_ms = parse_timestamp_ms(summary.get("created_at"))
if sample_window_end_ms == 0:
    sample_window_end_ms = sample_window_start_ms

max_gpu_used_mb = 0
avg_gpu_used_mb = 0.0
avg_ready_queue_chunks_at_gpu_start = 0.0
avg_prepare_inflight_chunks_at_gpu_start = 0.0
vram_budget_exceeded = 0
if benchmark_db_path.exists():
    con = sqlite3.connect(benchmark_db_path)
    row = con.execute(
        """
        select
            coalesce(max(gpu_used_mb), 0) as max_gpu_used_mb,
            coalesce(avg(gpu_used_mb), 0.0) as avg_gpu_used_mb,
            coalesce(avg(ready_queue_chunks_at_gpu_start), 0.0) as avg_ready_queue_chunks_at_gpu_start,
            coalesce(avg(prepare_inflight_chunks_at_gpu_start), 0.0) as avg_prepare_inflight_chunks_at_gpu_start
        from vector_batch_run
        where finished_at_ms >= ? and finished_at_ms <= ? and gpu_used_mb is not null
        """,
        (sample_window_start_ms, sample_window_end_ms),
    ).fetchone()
    con.close()
    if row is not None:
        max_gpu_used_mb = int(row[0] or 0)
        avg_gpu_used_mb = float(row[1] or 0.0)
        avg_ready_queue_chunks_at_gpu_start = float(row[2] or 0.0)
        avg_prepare_inflight_chunks_at_gpu_start = float(row[3] or 0.0)
        vram_budget_exceeded = int(max_gpu_used_mb > max_vram_used_mb_budget)

summary.update(
    {
        "window_chunk_delta": window_chunk_delta,
        "window_chunks_per_second": window_chunks_per_second,
        "avg_ready_queue_chunks_at_gpu_start": avg_ready_queue_chunks_at_gpu_start,
        "avg_prepare_inflight_chunks_at_gpu_start": avg_prepare_inflight_chunks_at_gpu_start,
        "max_gpu_used_mb": max_gpu_used_mb,
        "avg_gpu_used_mb": avg_gpu_used_mb,
        "vram_budget_exceeded": bool(vram_budget_exceeded),
        "benchmark_sample_window_start_ms": sample_window_start_ms,
        "benchmark_sample_window_end_ms": sample_window_end_ms,
        "benchmark_ready_depth": ready_depth,
        "benchmark_pipeline_depth": pipeline_depth,
        "benchmark_prepare_workers": prepare_workers,
    }
)
summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")

print(
    "\t".join(
            [
                label,
                mode,
                gpu_backend,
                tokens,
                ready_depth,
                pipeline_depth,
            prepare_workers,
            max_items,
            max_batch_bytes,
            str(max_vram_used_mb_budget),
            str(gpu_admission_vram_used_mb),
            graph_workers,
            str(summary.get("max_chunk_embeddings_per_second", 0.0)),
            str(window_chunk_delta),
            str(window_chunks_per_second),
            str(avg_ready_queue_chunks_at_gpu_start),
            str(avg_prepare_inflight_chunks_at_gpu_start),
            str(max_gpu_used_mb),
            str(avg_gpu_used_mb),
            str(vram_budget_exceeded),
            str(summary.get("max_graph_projection_queue_runtime_inflight", 0)),
            str(summary.get("max_graph_workers_active_current", 0)),
            str(summary.get("dominant_bottleneck", "")),
            str(summary_path),
        ]
    )
)
PY
}

for tokens in "${token_caps[@]}"; do
    [[ -n "$tokens" ]] || continue
    run_step "$tokens"
done

python3 - "$results_tsv" <<'PY'
import csv
import sys
from pathlib import Path

path = Path(sys.argv[1])
rows = list(csv.DictReader(path.read_text().splitlines(), delimiter="\t"))
print("label\tmode\tgpu_backend\ttokens\tready_depth\tpipeline_depth\tprepare_workers\titems\tbytes\tvram_budget_mb\tgpu_admission_vram_used_mb\tgraph_workers\tinstant_chunks_per_second\twindow_chunk_delta\twindow_chunks_per_second\tavg_ready_chunks_at_gpu_start\tavg_prepare_chunks_at_gpu_start\tmax_gpu_used_mb\tavg_gpu_used_mb\tvram_budget_exceeded\tgraph_inflight\tgraph_workers_active\tbottleneck")
for row in rows:
    print(
        "\t".join(
            [
                row["label"],
                row["mode"],
                row["gpu_backend"],
                row["tokens"],
                row["ready_depth"],
                row["pipeline_depth"],
                row["prepare_workers"],
                row["max_items"],
                row["max_batch_bytes"],
                row["max_vram_used_mb_budget"],
                row["gpu_admission_vram_used_mb"],
                row["graph_workers"],
                row["max_chunk_embeddings_per_second"],
                row["window_chunk_delta"],
                row["window_chunks_per_second"],
                row["avg_ready_queue_chunks_at_gpu_start"],
                row["avg_prepare_inflight_chunks_at_gpu_start"],
                row["max_gpu_used_mb"],
                row["avg_gpu_used_mb"],
                row["vram_budget_exceeded"],
                row["max_graph_projection_queue_runtime_inflight"],
                row["max_graph_workers_active_current"],
                row["dominant_bottleneck"],
            ]
        )
    )
PY

echo "[benchmark-vector-token-matrix] results saved to $results_tsv"
