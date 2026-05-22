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

axon_workspace_release_bin_for() {
    local project_root="${1:?project root required}"
    local bin_name="${2:?bin name required}"
    local cargo_target_root="${CARGO_TARGET_DIR:-$project_root/.axon/cargo-target}"
    printf '%s\n' "$cargo_target_root/release/$bin_name"
}

axon_build_info_path_for() {
    local project_root="${1:?project root required}"
    local bin_name="${2:?bin name required}"
    printf '%s\n' "$project_root/bin/$bin_name.build-info"
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

    # REQ-AXO-901661 — Source `bin/*.build-info` ONLY for the live instance.
    #
    # `bin/axon-core.build-info` is stamped by `axon setup --artifact-only`
    # during a live promote. Sourcing it from a DEV start.sh leaks the live
    # promote's `AXON_BUILD_ID` / `AXON_RELEASE_VERSION` /
    # `AXON_INSTALL_GENERATION` into the dev brain — so MCP `status` reports
    # the live's `runtime_version.build_id` instead of dev's actual git
    # describe at start time. That falsifies the `feedback_dev_first_no_exception`
    # gate (REQ-AXO-901659 / 901660) which compares dev brain build_id to
    # the candidate HEAD short-sha : with the leak, dev appears to "already
    # run the candidate" indefinitely and the gate is effectively bypassed.
    #
    # Fix : restrict the source to `AXON_INSTANCE_KIND == "live"`. Dev,
    # test, and other instances fall through to `axon_workspace_build_id`
    # which runs `git describe --tags --always --dirty` at start time —
    # giving each instance a build_id that reflects its actual code state.
    if [[ "${AXON_INSTANCE_KIND:-live}" == "live" && -f "$build_info_file" ]]; then
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
