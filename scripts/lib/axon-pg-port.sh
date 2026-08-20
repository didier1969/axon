#!/usr/bin/env bash
# REQ-AXO-902350 — THE canonical Postgres port for shell callers.
#
# Upstream truth is `services.postgres.port` in devenv.nix, which devenv exports
# as PGPORT. The literal below is only the net for scripts run outside the devenv
# shell. It used to be copy-pasted in ensure-runtime.sh, axon-supervisor.sh (x2)
# and cleanup-tmp-fixtures.sh, so a port change silently left stale fallbacks
# behind — the 2026-08-20 drift (PG on :44145, every client URL on :44144).
: "${AXON_CANONICAL_PG_PORT:=${PGPORT:-44144}}"
export AXON_CANONICAL_PG_PORT
