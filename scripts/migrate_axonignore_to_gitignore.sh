#!/usr/bin/env bash
set -euo pipefail

ROOT="/home/dstadel/projects"
APPLY=0

usage() {
  cat <<'USAGE'
Usage: scripts/migrate_axonignore_to_gitignore.sh [--root <dir>] [--apply]

Default mode is dry-run. The script prints:
- patterns auto-migrated
- patterns requiring manual review

Notes:
- Negation rules (`!pattern`) are NOT auto-migrated.
- Anchored path rules may need manual placement review.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      ROOT="${2:-}"
      shift 2
      ;;
    --apply)
      APPLY=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

echo "Migration root: $ROOT"
echo "Mode: $([[ "$APPLY" -eq 1 ]] && echo APPLY || echo DRY-RUN)"
echo ""

mapfile -t AXONIGNORE_FILES < <(find "$ROOT" -type f -name ".axonignore" 2>/dev/null | sort)

if [[ ${#AXONIGNORE_FILES[@]} -eq 0 ]]; then
  echo "No .axonignore files found under $ROOT"
  exit 0
fi

for axon_file in "${AXONIGNORE_FILES[@]}"; do
  dir="$(dirname "$axon_file")"
  git_file="$dir/.gitignore"
  echo "== $axon_file =="

  auto_patterns=()
  manual_patterns=()

  while IFS= read -r line || [[ -n "$line" ]]; do
    trimmed="$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
    [[ -z "$trimmed" ]] && continue
    [[ "${trimmed:0:1}" == "#" ]] && continue

    if [[ "${trimmed:0:1}" == "!" ]]; then
      manual_patterns+=("$trimmed  # manual: negation rule")
      continue
    fi
    if [[ "$trimmed" == *"**"* ]]; then
      manual_patterns+=("$trimmed  # manual: recursive glob review")
      continue
    fi

    auto_patterns+=("$trimmed")
  done < "$axon_file"

  if [[ ${#auto_patterns[@]} -eq 0 && ${#manual_patterns[@]} -eq 0 ]]; then
    echo "  no actionable patterns"
    echo ""
    continue
  fi

  if [[ ${#auto_patterns[@]} -gt 0 ]]; then
    echo "  auto-migrated patterns:"
    for p in "${auto_patterns[@]}"; do
      echo "    - $p"
    done
    if [[ "$APPLY" -eq 1 ]]; then
      touch "$git_file"
      {
        echo ""
        echo "# migrated from .axonignore ($(date -Iseconds))"
        for p in "${auto_patterns[@]}"; do
          echo "$p"
        done
      } >> "$git_file"
      echo "  applied to: $git_file"
    fi
  fi

  if [[ ${#manual_patterns[@]} -gt 0 ]]; then
    echo "  manual review required:"
    for p in "${manual_patterns[@]}"; do
      echo "    - $p"
    done
  fi
  echo ""
done

