#!/usr/bin/env bash
# Monotonic time primitives for lifecycle deadlines. /proc/uptime is immune to NTP and
# wall-clock jumps and is available on every supported Linux/WSL runtime.

axon_monotonic_ms() {
  awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime
}

axon_deadline_after_seconds() {
  local seconds="${1:?seconds required}"
  printf '%s\n' "$(( $(axon_monotonic_ms) + seconds * 1000 ))"
}
