#!/usr/bin/env bash
# REQ-AXO-901649 — Wallaby E2E suite runner for the dashboard.
#
# Boots `mix test --only feature` inside the devenv shell so that the
# Nix-provisioned ChromeDriver + Chromium binaries are on PATH (and the
# Elixir/Phoenix/Bandit toolchain is the canonical one). Exits non-zero
# on the first failing test.
#
# Usage:
#   bash scripts/test-dashboard-e2e.sh                # run full feature suite
#   bash scripts/test-dashboard-e2e.sh --file <path>  # run a single feature file
#   bash scripts/test-dashboard-e2e.sh --trace        # pass --trace through to mix
#
# Returns:
#   0  → 100% green
#   ≠0 → at least one feature failed (mix test exit code propagated)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DASH_DIR="${REPO_ROOT}/src/dashboard"

cd "${REPO_ROOT}"

# Forward any caller args after our own — most common: `--trace` or a
# specific feature file path. Default: run every feature.
EXTRA_ARGS=("--only" "feature")
if [[ $# -gt 0 ]]; then
  EXTRA_ARGS=("$@")
fi

# Sanity-check that ChromeDriver + Chromium are on PATH from devenv.nix.
preflight() {
  if ! command -v chromedriver >/dev/null 2>&1; then
    echo "FATAL: chromedriver not on PATH — add pkgs.chromedriver to devenv.nix" >&2
    return 1
  fi
  if ! command -v chromium >/dev/null 2>&1; then
    echo "FATAL: chromium not on PATH — add pkgs.chromium to devenv.nix" >&2
    return 1
  fi
  echo "[preflight] chromedriver=$(chromedriver --version | head -1)"
  echo "[preflight] chromium=$(chromium --version 2>&1 | head -1)"
}

run_suite() {
  cd "${DASH_DIR}"
  echo "[suite] cwd=$(pwd)"
  echo "[suite] mix test ${EXTRA_ARGS[*]}"
  mix test "${EXTRA_ARGS[@]}"
}

# Always run inside devenv shell so PATH is canonical.
if [[ "${IN_DEVENV_SHELL:-0}" = "1" ]]; then
  preflight
  run_suite
else
  exec devenv shell --no-reload --no-tui -- bash -lc \
    "export IN_DEVENV_SHELL=1; bash '$0' ${EXTRA_ARGS[*]}"
fi
