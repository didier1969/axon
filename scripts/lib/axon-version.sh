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

# REQ-AXO-902464 — le seul contrôle d'artefact qui ne soit pas auto-référentiel.
#
# Toutes les autres vérifications de release comparent deux dérivés du MÊME
# fichier : le sha du build-info au sha réel, l'étiquette `AXON_BUILD_ID` à
# `git describe`, l'artefact à la cible dont il dit sortir. Le 2026-08-23 elles
# étaient TOUTES vraies sur un binaire vieux d'un jour, parce qu'aucune ne lisait
# le binaire. Celle-ci le lit : le linker réserve `.axon_build_id`, puis le
# packaging y estampille l'identité sans recompiler, et on vérifie qu'elle y est.
#
# Renvoie 0 si l'artefact PORTE cette identité, non-zéro sinon — y compris quand
# l'artefact est absent ou l'identité vide (`grep -F ''` matcherait tout : un
# contrôle qui valide n'importe quoi se lit comme un verdict, et c'est pire que
# pas de contrôle du tout).
axon_artifact_carries_build_id() {
    local artifact="${1:-}"
    local build_id="${2:-}"

    [[ -n "$artifact" && -f "$artifact" ]] || return 1
    [[ -n "$build_id" ]] || return 1

    # REQ-AXO-902464 — `grep -q` sort a la PREMIERE correspondance et ferme le
    # tube ; `strings`, qui a encore des dizaines de Mo a produire, recoit
    # SIGPIPE et meurt avec 141. Sous `set -o pipefail` (preflight.sh l'active),
    # le pipeline herite de ce 141 : la sonde repondait « ne porte pas »
    # PRECISEMENT quand le binaire portait l'identite. Une garde inversee, qui
    # echoue sur le succes et ne peut jamais valider un promote correct.
    #
    # Mesure : bin/axon-brain fait 66 Mo, les fixtures du test 8 Ko — `strings`
    # les epuise avant que `grep` ne sorte, donc pas de SIGPIPE et le test
    # restait vert. La discipline etait la (`set -euo pipefail` ligne 25), c'est
    # l'ECHELLE qui manquait.
    #
    # `grep -c` lit tout le flux : pas de fermeture prematuree, donc pas de
    # SIGPIPE, et le compte reste utile au diagnostic.
    local hits
    hits="$(strings -a "$artifact" 2>/dev/null | grep -cF -- "$build_id" || true)"
    [[ "${hits:-0}" -gt 0 ]]
}

# REQ-AXO-902543 — stamp the fixed ELF identity slot after linking. The slot is
# deliberately constant while rustc runs, so a new release label cannot
# invalidate every axon-core codegen unit. Provenance remains inside the binary
# and is verified by axon_artifact_carries_build_id exactly as before.
axon_stamp_artifact_build_id() {
    local artifact="${1:?artifact required}"
    local build_id="${2:?build id required}"
    local section_file=""
    local objcopy_bin=""
    local readelf_bin=""
    local section_table=""

    [[ -f "$artifact" ]] || return 1
    [[ "$build_id" != *$'\n'* && "$build_id" != *$'\r'* ]] || return 1
    (( ${#build_id} < 128 )) || return 1
    objcopy_bin="$(command -v objcopy || command -v llvm-objcopy || true)"
    [[ -n "$objcopy_bin" ]] || return 1
    readelf_bin="$(command -v readelf || command -v llvm-readelf || true)"
    [[ -n "$readelf_bin" ]] || return 1

    section_file="$(mktemp -t axon-build-id-section.XXXXXX)"
    printf '%s' "$build_id" > "$section_file"
    truncate -s 128 "$section_file"
    section_table="$("$readelf_bin" -S -W "$artifact" 2>/dev/null || true)"
    if [[ "$section_table" == *".axon_build_id"* ]]; then
        "$objcopy_bin" --update-section ".axon_build_id=$section_file" "$artifact"
    else
        # Small utility binaries may not retain the library's reserved slot.
        # They still carry a file-backed section that the runtime/provenance
        # probes read from ELF; it does not need to enter a load segment.
        "$objcopy_bin" \
            --add-section ".axon_build_id=$section_file" \
            --set-section-flags .axon_build_id=readonly,data \
            "$artifact"
    fi
    local status=$?
    rm -f "$section_file"
    return "$status"
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
