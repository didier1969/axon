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

# First integer of the first dotted version found in a `--version` line.
major_of() { grep -oE '[0-9]+(\.[0-9]+){2,3}' <<<"$1" | head -1 | cut -d. -f1; }

# Sanity-check that ChromeDriver + Chromium are on PATH from devenv.nix AND
# that they actually MATCH — REQ-AXO-902569. Presence alone proves nothing:
# Wallaby resolves its own browser (`google-chrome` BEFORE `chromium`), so a
# system-wide Chrome silently wins over the one devenv provisions, and every
# WebDriver session then dies as "invalid session id" with no assertion ever
# evaluated. A preflight that cannot contradict the run is worthless.
preflight() {
  if ! command -v chromedriver >/dev/null 2>&1; then
    echo "FATAL: chromedriver not on PATH — add pkgs.chromedriver to devenv.nix" >&2
    return 1
  fi
  if ! command -v chromium >/dev/null 2>&1; then
    echo "FATAL: chromium not on PATH — add pkgs.chromium to devenv.nix" >&2
    return 1
  fi

  # Same resolution rule as config/test.exs, so check and run agree.
  local browser="${WALLABY_CHROME_BINARY:-$(command -v chromium)}"
  local driver_version browser_version driver_major browser_major
  driver_version="$(chromedriver --version 2>&1 | head -1)"
  browser_version="$("${browser}" --version 2>&1 | head -1)"
  driver_major="$(major_of "${driver_version}")"
  browser_major="$(major_of "${browser_version}")"

  echo "[preflight] chromedriver=${driver_version}"
  echo "[preflight] browser=${browser} -> ${browser_version}"

  if [[ -z "${driver_major}" || -z "${browser_major}" ]]; then
    echo "FATAL: no major version readable (driver='${driver_version}' browser='${browser_version}')" >&2
    return 1
  fi
  if [[ "${driver_major}" != "${browser_major}" ]]; then
    echo "FATAL: chromedriver major ${driver_major} != browser major ${browser_major}." >&2
    echo "       Every Wallaby session would die as 'invalid session id' (REQ-AXO-902569)." >&2
    echo "       Point WALLABY_CHROME_BINARY at a browser matching the driver." >&2
    return 1
  fi
  echo "[preflight] driver and browser agree on major ${driver_major}"
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
