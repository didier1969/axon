#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=scripts/lib/axon-instance.sh
source "$SCRIPT_DIR/lib/axon-instance.sh"

CONFIG_PATH="${CODEX_CONFIG_PATH:-$HOME/.codex/config.toml}"
APPLY=0

usage() {
  cat <<'EOF'
Usage: ./scripts/axon sync-codex-mcp-config [--apply] [--config /path/to/config.toml]

Default mode prints the advertised Axon MCP endpoints and the config changes that would be made.
Use --apply to update the Codex config explicitly.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --apply)
      APPLY=1
      ;;
    --config)
      CONFIG_PATH="${2:?missing value for --config}"
      shift
      ;;
    --config=*)
      CONFIG_PATH="${1#*=}"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
  shift
done

capture_public_url() {
  local instance_kind="$1"
  AXON_INSTANCE_KIND="$instance_kind" axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")" >/dev/null 2>&1 || true
  case "$instance_kind" in
    live)
      printf '%s\n' "${AXON_MCP_PUBLIC_URL:-}"
      ;;
    dev)
      printf '%s\n' "${AXON_MCP_PUBLIC_URL:-}"
      ;;
  esac
}

capture_public_state() {
  local instance_kind="$1"
  AXON_INSTANCE_KIND="$instance_kind" axon_resolve_instance "$ROOT_DIR" "$(basename "$ROOT_DIR")" >/dev/null 2>&1 || true
  printf '%s|%s|%s\n' \
    "${AXON_PUBLIC_ENDPOINTS_AVAILABLE:-0}" \
    "${AXON_PUBLIC_HOST:-}" \
    "${AXON_MCP_PUBLIC_URL:-}"
}

live_state="$(capture_public_state live)"
dev_state="$(capture_public_state dev)"

IFS='|' read -r live_available live_host live_url <<<"$live_state"
IFS='|' read -r dev_available dev_host dev_url <<<"$dev_state"

echo "Axon advertised MCP endpoints"
echo "  live: ${live_url:-<unresolved>} host=${live_host:-<none>} available=${live_available:-0}"
echo "  dev:  ${dev_url:-<unresolved>} host=${dev_host:-<none>} available=${dev_available:-0}"

if [[ "${live_available:-0}" != "1" || -z "${live_url:-}" || "${dev_available:-0}" != "1" || -z "${dev_url:-}" ]]; then
  echo ""
  echo "Advertised endpoints are unresolved."
  echo "Set AXON_PUBLIC_HOST to a non-loopback host address before syncing client config."
  exit 1
fi

if [[ "$APPLY" != "1" ]]; then
  echo ""
  echo "Dry run only. Re-run with --apply to update: $CONFIG_PATH"
  exit 0
fi

mkdir -p "$(dirname "$CONFIG_PATH")"
python3 "$SCRIPT_DIR/sync_codex_mcp_config.py" \
  --config "$CONFIG_PATH" \
  --live-url "$live_url" \
  --dev-url "$dev_url" \
  --apply
