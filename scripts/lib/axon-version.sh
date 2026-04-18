#!/usr/bin/env bash

# Shared version metadata resolver for Axon operator/runtime surfaces.

axon_package_version() {
    local project_root="${1:?project root required}"
    local cargo_manifest="$project_root/src/axon-core/Cargo.toml"
    local package_version=""

    if [[ -f "$cargo_manifest" ]]; then
        package_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$cargo_manifest" | head -n1)"
    fi
    printf '%s\n' "${package_version:-unknown}"
}

axon_workspace_build_id() {
    local project_root="${1:?project root required}"
    local package_version=""
    package_version="$(axon_package_version "$project_root")"

    if git -C "$project_root" rev-parse --git-dir >/dev/null 2>&1; then
        git -C "$project_root" describe --tags --always --dirty 2>/dev/null || printf '%s\n' "$package_version"
        return 0
    fi

    printf '%s\n' "$package_version"
}

axon_workspace_release_bin() {
    local project_root="${1:?project root required}"
    local cargo_target_root="${CARGO_TARGET_DIR:-$project_root/.axon/cargo-target}"
    printf '%s\n' "$cargo_target_root/release/axon-core"
}

axon_file_sha256() {
    local path="${1:?path required}"
    sha256sum "$path" | awk '{print $1}'
}

axon_write_export_file() {
    local path="$1"
    shift

    : > "$path"
    while [[ $# -gt 0 ]]; do
        local key="$1"
        local value="$2"
        local escaped=""
        printf -v escaped '%q' "$value"
        printf '%s=%s\n' "$key" "$escaped" >> "$path"
        shift 2
    done
}

axon_resolve_version() {
    local project_root="${1:?project root required}"
    local build_info_file="$project_root/bin/axon-core.build-info"
    local package_version=""
    local build_id=""
    local release_version=""
    local install_generation=""

    export AXON_BUILD_INFO_FILE="$build_info_file"

    package_version="$(axon_package_version "$project_root")"

    if [[ -f "$build_info_file" ]]; then
        # shellcheck disable=SC1090
        source "$build_info_file"
    fi

    build_id="${AXON_BUILD_ID:-$(axon_workspace_build_id "$project_root")}"
    release_version="${AXON_RELEASE_VERSION:-$package_version}"
    install_generation="${AXON_INSTALL_GENERATION:-workspace}"

    export AXON_PACKAGE_VERSION="${AXON_PACKAGE_VERSION:-$package_version}"
    export AXON_RELEASE_VERSION="$release_version"
    export AXON_BUILD_ID="$build_id"
    export AXON_INSTALL_GENERATION="$install_generation"
}
