#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
TERM = "project_slug"

# Only these files may still mention the legacy term while the migration
# path exists. Everything else in active code/docs should fail.
ALLOWED_EXACT = {
    Path("ADR-2026-04-18-project-registry-runtime-authority.md"),
    Path("src/axon-core/src/graph_bootstrap.rs"),
    Path("scripts/check_no_project_slug.py"),
    Path("scripts/axon"),
}

SCAN_ROOTS = [
    Path("src/axon-core"),
    Path("scripts"),
    Path("docs/skills"),
    Path("docs/working-notes/2026-04-05-soll-canonical-ids-and-project-scope.md"),
    Path("docs/working-notes/2026-04-01-reprise-handoff.md"),
    Path("docs/working-notes/2026-04-01-wsl-install-runtime-notes.md"),
    Path("docs/plans/2026-04-03-reliability-callgraph-execution-plan.md"),
    Path("docs/plans/2026-04-07-omniscience-federation-design.md"),
    Path("docs/archive/root-docs/expert_prompt.md"),
    Path("ADR-2026-04-18-project-registry-runtime-authority.md"),
]

TEXT_EXTENSIONS = {
    ".md",
    ".rs",
    ".py",
    ".sh",
    ".json",
    ".yaml",
    ".yml",
    ".toml",
}

SKIP_DIR_NAMES = {
    "target",
    "deps",
    "node_modules",
    "_build",
    ".git",
    ".devenv",
    "__pycache__",
}


def should_scan(path: Path) -> bool:
    if path.name.startswith("."):
        return False
    if path.suffix and path.suffix not in TEXT_EXTENSIONS:
        return False
    return path.is_file()


def iter_scan_files() -> list[Path]:
    files: list[Path] = []
    for root in SCAN_ROOTS:
        abs_root = REPO_ROOT / root
        if not abs_root.exists():
            continue
        if abs_root.is_file():
            files.append(abs_root)
            continue
        for path in abs_root.rglob("*"):
            if any(part in SKIP_DIR_NAMES for part in path.parts):
                continue
            if should_scan(path):
                files.append(path)
    return sorted(set(files))


def main() -> int:
    violations: list[tuple[str, int, str]] = []
    for path in iter_scan_files():
        rel = path.relative_to(REPO_ROOT)
        if rel in ALLOWED_EXACT:
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for lineno, line in enumerate(content.splitlines(), start=1):
            if TERM in line:
                violations.append((str(rel), lineno, line.strip()))

    if violations:
        print("Forbidden legacy term found outside allowlist:\n", file=sys.stderr)
        for rel, lineno, line in violations:
            print(f"{rel}:{lineno}: {line}", file=sys.stderr)
        return 1

    print("OK: no forbidden 'project_slug' occurrences found in active surfaces.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
