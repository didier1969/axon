#!/usr/bin/env bash

axon_ort_artifact_prepare() {
    local artifact_dir="${1:?artifact dir required}"
    AXON_ORT_ARTIFACT_LOG_DIR="$artifact_dir/logs"
    mkdir -p "$AXON_ORT_ARTIFACT_LOG_DIR"
    export AXON_ORT_ARTIFACT_LOG_DIR
}

axon_ort_artifact_new_build_log() {
    local log_dir="${1:?log dir required}"
    mktemp "$log_dir/build-XXXXXX.log"
}

axon_ort_artifact_write_manifest() {
    local manifest_path="${1:?manifest path required}"
    local manifest_dir
    local manifest_tmp

    manifest_dir="$(dirname "$manifest_path")"
    mkdir -p "$manifest_dir"
    manifest_tmp="$(mktemp "${AXON_ORT_ARTIFACT_LOG_DIR}/current.json.XXXXXX.tmp")"
    cat > "$manifest_tmp"
    mv "$manifest_tmp" "$manifest_path"
}
