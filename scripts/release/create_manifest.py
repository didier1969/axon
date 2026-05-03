#!/usr/bin/env python3
import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import sys


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_output(repo: pathlib.Path, *args: str) -> str:
    try:
        return (
            subprocess.check_output(["git", "-C", str(repo), *args], text=True)
            .strip()
        )
    except Exception:
        return ""


def default_package_version(repo: pathlib.Path) -> str:
    cargo = repo / "src" / "axon-core" / "Cargo.toml"
    if not cargo.exists():
        return "unknown"
    for line in cargo.read_text().splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    return "unknown"


def load_build_info(path: pathlib.Path) -> dict[str, str]:
    data: dict[str, str] = {}
    if not path.exists():
        return data
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or "=" not in line:
            continue
        key, value = line.split("=", 1)
        parsed = shlex.split(value.strip())
        data[key] = parsed[0] if parsed else ""
    return data


def runtime_primary_artifact(
    repo: pathlib.Path, artifact_arg: str | None, build_info_arg: str | None
) -> tuple[str, pathlib.Path, pathlib.Path]:
    artifact = pathlib.Path(artifact_arg).resolve() if artifact_arg else (repo / "bin" / "axon-brain")
    build_info = (
        pathlib.Path(build_info_arg).resolve()
        if build_info_arg
        else (repo / "bin" / "axon-brain.build-info")
    )
    return "axon-brain", artifact, build_info


def runtime_artifact_names() -> tuple[str, ...]:
    # REQ-AXO-153 — axonctl supervises both axon-brain and axon-indexer and
    # owns the status contract surfaced to `axon status` / qualify-mcp.
    # Including it in the manifest is what makes promote_live.sh's generic
    # artifact-copy loop deploy a fresh axonctl alongside the runtime
    # binaries; otherwise axonctl-side fixes never reach production.
    return ("axon-brain", "axon-indexer", "axonctl")


def main() -> int:
    repo = pathlib.Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description="Create a canonical Axon release manifest.")
    parser.add_argument("--artifact")
    parser.add_argument("--build-info")
    parser.add_argument("--state", choices=["pushed", "qualified"], default="qualified")
    parser.add_argument("--release-version")
    parser.add_argument("--install-generation")
    parser.add_argument("--evidence", action="append", default=[])
    parser.add_argument("--output")
    args = parser.parse_args()
    primary_bin_name, artifact, build_info_path = runtime_primary_artifact(
        repo, args.artifact, args.build_info
    )
    if not artifact.exists():
        print(f"Artifact not found: {artifact}", file=sys.stderr)
        return 2

    preflight = repo / "scripts" / "release" / "preflight.sh"
    preflight_cmd = ["bash", str(preflight)]
    subprocess.run(preflight_cmd, check=True)
    build_info = load_build_info(build_info_path)

    package_version = build_info.get("AXON_PACKAGE_VERSION") or default_package_version(repo)
    release_version = (
        args.release_version
        or build_info.get("AXON_RELEASE_VERSION")
        or package_version
    )
    build_id = build_info.get("AXON_BUILD_ID") or git_output(repo, "describe", "--tags", "--always", "--dirty") or package_version
    install_generation = args.install_generation or build_info.get("AXON_INSTALL_GENERATION") or "workspace"

    git_commit = git_output(repo, "rev-parse", "HEAD")
    git_describe = git_output(repo, "describe", "--tags", "--always", "--dirty")
    git_tag = git_output(repo, "describe", "--tags", "--abbrev=0")
    git_dirty = git_output(repo, "status", "--short", "--untracked-files=no")

    artifact_sha = sha256_file(artifact)
    artifacts_root = repo / ".axon" / "releases" / "artifacts" / artifact_sha[:16]
    archived_artifact = artifacts_root / primary_bin_name
    archived_build_info = artifacts_root / f"{primary_bin_name}.build-info"
    artifacts_root.mkdir(parents=True, exist_ok=True)
    if not archived_artifact.exists():
        shutil.copy2(artifact, archived_artifact)
    if build_info_path.exists() and not archived_build_info.exists():
        shutil.copy2(build_info_path, archived_build_info)

    created_at = dt.datetime.now(dt.timezone.utc).isoformat()
    evidence = []
    artifact_mtime = artifact.stat().st_mtime
    for raw in args.evidence:
        evidence_path = pathlib.Path(raw).resolve()
        if not evidence_path.exists():
            print(f"Evidence not found: {evidence_path}", file=sys.stderr)
            return 2
        if evidence_path.stat().st_mtime < artifact_mtime:
            print(
                f"Evidence appears older than artifact build: {evidence_path}",
                file=sys.stderr,
            )
            return 2
        evidence.append(str(evidence_path))

    manifest = {
        "schema_version": 1,
        "created_at": created_at,
        "state": args.state,
        "runtime_contract": "brain_mcp_indexer_ist",
        "source": {
            "repo_root": str(repo),
            "git_commit": git_commit or None,
            "git_describe": git_describe or None,
            "git_tag": git_tag or None,
            "git_dirty": bool(git_dirty),
        },
        "runtime_version": {
            "release_version": release_version,
            "package_version": package_version,
            "build_id": build_id,
            "install_generation": install_generation,
        },
        "artifact": {
            "name": primary_bin_name,
            "path": str(archived_artifact),
            "sha256": artifact_sha,
            "size_bytes": archived_artifact.stat().st_size,
            "build_info_path": str(archived_build_info) if archived_build_info.exists() else None,
            "build_info_sha256": sha256_file(archived_build_info) if archived_build_info.exists() else None,
        },
        "artifacts": {},
        "qualification": {
            "evidence": evidence,
        },
    }

    for bin_name in runtime_artifact_names():
        bin_path = repo / "bin" / bin_name
        build_info = repo / "bin" / f"{bin_name}.build-info"
        if not bin_path.exists():
            continue
        bin_sha = sha256_file(bin_path)
        bin_root = repo / ".axon" / "releases" / "artifacts" / bin_sha[:16]
        archived_bin = bin_root / bin_name
        archived_bin_build_info = bin_root / f"{bin_name}.build-info"
        bin_root.mkdir(parents=True, exist_ok=True)
        if not archived_bin.exists():
            shutil.copy2(bin_path, archived_bin)
        if build_info.exists() and not archived_bin_build_info.exists():
            shutil.copy2(build_info, archived_bin_build_info)
        manifest["artifacts"][bin_name] = {
            "path": str(archived_bin),
            "sha256": bin_sha,
            "size_bytes": archived_bin.stat().st_size,
            "build_info_path": str(archived_bin_build_info) if archived_bin_build_info.exists() else None,
            "build_info_sha256": sha256_file(archived_bin_build_info) if archived_bin_build_info.exists() else None,
        }

    build_tag = build_id.replace("/", "_").replace(" ", "_")
    default_output = repo / ".axon" / "releases" / "candidates" / f"{release_version}-{build_tag}.json"
    output = pathlib.Path(args.output).resolve() if args.output else default_output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
