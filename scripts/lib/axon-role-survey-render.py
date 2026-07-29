#!/usr/bin/env python3
"""Render the supervised-role survey for `axon status` — REQ-AXO-902264.

Reads survey rows on stdin (the pipe-separated output of `axon_role_survey`), one per
role: `name|status|is_ready|restarts|max_restarts|serving|verdict`.

Writes the operator-facing lines on stdout and exits:
    0 — nothing abandoned
    2 — at least one role is down or has been abandoned (caller degrades the runtime)
    1 — the input could not be read as a survey

It is a separate script, and not another branch inside the `status.sh` heredoc, because
these lines carry the RECOVERY COMMANDS an operator will actually run. A recovery command
that silently rots is worse than no line at all, and a heredoc inside a shell script
cannot be exercised with fixtures. `tests/shell/test_role_survey_render.sh` covers every
verdict here.

Context, measured on process-compose 1.94.0 in an isolated probe (see the REQ):
  * `max_restarts` is a HARD ceiling. Once consumed, the role stays `Completed` forever
    and the supervisor never tries again — the only trace being a log line nobody reads.
  * The counter NEVER goes back down: not after a healthy period, and not after the
    explicit `POST /process/start` used to recover. So a role can be Running with zero
    remaining safety net, which is what the `no_budget` verdict names.

REQ-AXO-902271 adds `wedged`, which is the opposite shape and needs the opposite advice:
the budget is INTACT because the supervisor never got to spend it. The role's stop never
completes (unreapable zombie on the WSL2 GPU channel), so self-healing does not give up —
it never starts. It is the only verdict here whose recovery is not a command to run now.
"""

import os
import sys

FIELDS = 7


def render(rows, pc_port, instance):
    """Yield (line, degrades) pairs. Pure: no I/O, no environment read."""
    for raw in rows:
        parts = raw.split("|")
        if len(parts) != FIELDS:
            continue
        name, status, ready, restarts, maxr, _serving, verdict = (p.strip() for p in parts)
        if not name:
            continue
        state = f"{status}/{ready}" if ready and ready != "-" else status
        start_cmd = f"curl -X POST http://127.0.0.1:{pc_port}/process/start/{name}"
        budget_cmd = (f"./scripts/axon --instance {instance} stop && "
                      f"./scripts/axon --instance {instance} start")

        if verdict == "ok":
            yield f"OK      {name:<15} {state}", False
        elif verdict == "oneshot":
            yield f"OK      {name:<15} {state} (one-shot task, exited 0)", False
        elif verdict == "no_budget":
            # Running, but with nothing left underneath it. Not a failure yet — which is
            # exactly why it needs saying now rather than after the next crash.
            yield (f"WARN    {name:<15} {state} but its restart budget is SPENT "
                   f"({restarts}/{maxr}): the counter never resets, so the next failure "
                   f"will NOT be retried. Restore the safety net with a full instance "
                   f"restart ({budget_cmd}) — that interrupts the brain, so plan it."), False
        elif verdict == "disabled":
            # Configuration, not failure: brain_only does not select the indexer.
            yield f"--      {name:<15} disabled for this runtime mode (start it: {start_cmd})", False
        elif verdict == "drift":
            # Does NOT degrade: the role answers its own health endpoint, so the runtime
            # works. What is broken is the supervisor's bookkeeping.
            yield (f"WARN    {name:<15} supervisor says '{status}' but the role IS serving "
                   f"its health endpoint — stale bookkeeping; a later crash would NOT be "
                   f"restarted"), False
        elif verdict == "not_ready":
            # Also non-degrading: this is the normal shape of a boot (the indexer spends
            # minutes loading the GPU model). Abandonment is the failure this section is
            # for; warmup is not abandonment.
            yield (f"WARN    {name:<15} Running but its readiness probe is not passing "
                   f"(boot, or degraded)"), False
        elif verdict == "wedged":
            # REQ-AXO-902271 — the one verdict whose recovery is NOT a start command.
            # `POST /process/start` is ignored while the supervisor still believes the role
            # is terminating, and `PATCH stop` answers "process is not running". Naming a
            # command that cannot work here would be the same lie as a green line.
            yield (f"FAIL    {name:<15} {state} — WEDGED mid-teardown behind an unreapable "
                   f"zombie ({restarts}/{maxr} restarts consumed: self-healing has not "
                   f"even STARTED, and will not, because the stop never completes). "
                   f"A start command will NOT work here. The blocked thread sits in "
                   f"uninterruptible D-state on the WSL2 GPU channel, usually behind an "
                   f"`nvidia-smi` from another tool: check with "
                   f"`ps -eo pid,stat,cmd | grep -E '^ *[0-9]+ +D'`. It clears on its own "
                   f"in 5-15 min once that caller releases the adapter, after which "
                   f"{start_cmd} works. `wsl --shutdown` forces it but closes every one of "
                   f"your Windows sessions — operator decision, never automatic."), True
        elif verdict == "exhausted":
            yield (f"FAIL    {name:<15} {state} — SELF-HEALING EXHAUSTED "
                   f"({restarts}/{maxr} restarts consumed): the supervisor will NEVER "
                   f"retry, and has not been trying since the last failure. "
                   f"Bring it back now: {start_cmd} — that does NOT give the budget back "
                   f"(the counter never resets); for that: {budget_cmd}"), True
        else:  # down, and any verdict a future version adds: fail loudly, never silently
            yield (f"FAIL    {name:<15} {state} — role is down "
                   f"({restarts}/{maxr} restarts consumed, retries left). "
                   f"Recover: {start_cmd}"), True


def main():
    rows = [line for line in sys.stdin.read().splitlines() if line.strip()]
    if not rows:
        return 1
    pc_port = os.environ.get("AXON_PC_PORT", "?")
    instance = os.environ.get("AXON_INSTANCE_KIND", "live")
    rendered = list(render(rows, pc_port, instance))
    if not rendered:
        return 1
    degraded = False
    for line, degrades in rendered:
        print(line)
        degraded = degraded or degrades
    return 2 if degraded else 0


if __name__ == "__main__":
    sys.exit(main())
