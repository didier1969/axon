#!/usr/bin/env python3
"""Run one promotion side effect with bounded retries and durable state.

The cutover verdict never depends on these hooks, but their outcome must not vanish.
Every transition is atomically projected below `.axon/live-release/hooks/<attempt>/`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import subprocess
import time
from typing import Sequence


def _utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def _safe_component(value: str) -> str:
    safe = "".join(c if c.isalnum() or c in "-_." else "-" for c in value)
    if not safe or safe in {".", ".."}:
        raise ValueError("hook/attempt identifier has no safe filesystem representation")
    return safe


def _atomic_json(path: pathlib.Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.tmp-{os.getpid()}")
    with tmp.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp, path)
    directory_fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def run_hook(
    *,
    state_root: pathlib.Path,
    attempt_id: str,
    hook_name: str,
    command: Sequence[str],
    max_attempts: int,
    timeout_seconds: int,
    retry_delay_seconds: float,
) -> int:
    if not command:
        raise ValueError("command required")
    if max_attempts < 1 or timeout_seconds < 1 or retry_delay_seconds < 0:
        raise ValueError("invalid retry bounds")

    path = state_root / _safe_component(attempt_id) / f"{_safe_component(hook_name)}.json"
    state = {
        "schema_version": 1,
        "release_attempt_id": attempt_id,
        "hook": hook_name,
        "command": list(command),
        "status": "running",
        "runner_pid": os.getpid(),
        "attempts_made": 0,
        "max_attempts": max_attempts,
        "timeout_seconds": timeout_seconds,
        "started_at": _utc_now(),
        "updated_at": _utc_now(),
        "history": [],
    }
    _atomic_json(path, state)
    last_rc = 1
    for number in range(1, max_attempts + 1):
        try:
            result = subprocess.run(command, timeout=timeout_seconds, check=False)
            last_rc = result.returncode
            terminal = "completed" if last_rc == 0 else "failed"
            detail = f"exit_code={last_rc}"
        except subprocess.TimeoutExpired:
            last_rc = 124
            terminal = "failed"
            detail = f"timeout_seconds={timeout_seconds}"

        state["attempts_made"] = number
        state["last_exit_code"] = last_rc
        state["history"].append(
            {"attempt": number, "status": terminal, "detail": detail, "at": _utc_now()}
        )
        if last_rc == 0:
            state["status"] = "completed"
            state["completed_at"] = _utc_now()
            state["updated_at"] = _utc_now()
            _atomic_json(path, state)
            return 0
        if number < max_attempts:
            state["status"] = "retrying"
            state["next_retry_delay_seconds"] = retry_delay_seconds
            state["updated_at"] = _utc_now()
            _atomic_json(path, state)
            if retry_delay_seconds:
                time.sleep(retry_delay_seconds)

    state["status"] = "failed"
    state["failed_at"] = _utc_now()
    state["updated_at"] = _utc_now()
    state.pop("next_retry_delay_seconds", None)
    _atomic_json(path, state)
    return last_rc


def defer_hook(state_root: pathlib.Path, attempt_id: str, hook_name: str, reason: str) -> int:
    path = state_root / _safe_component(attempt_id) / f"{_safe_component(hook_name)}.json"
    now = _utc_now()
    _atomic_json(
        path,
        {
            "schema_version": 1,
            "release_attempt_id": attempt_id,
            "hook": hook_name,
            "status": "deferred",
            "reason": reason,
            "attempts_made": 0,
            "started_at": now,
            "updated_at": now,
            "history": [],
        },
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--state-root", type=pathlib.Path, required=True)
    parser.add_argument("--attempt-id", required=True)
    parser.add_argument("--hook-name", required=True)
    parser.add_argument("--max-attempts", type=int, default=3)
    parser.add_argument("--timeout-seconds", type=int, default=120)
    parser.add_argument("--retry-delay-seconds", type=float, default=2)
    parser.add_argument("--defer-reason")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if args.defer_reason:
        return defer_hook(args.state_root, args.attempt_id, args.hook_name, args.defer_reason)
    return run_hook(
        state_root=args.state_root,
        attempt_id=args.attempt_id,
        hook_name=args.hook_name,
        command=command,
        max_attempts=args.max_attempts,
        timeout_seconds=args.timeout_seconds,
        retry_delay_seconds=args.retry_delay_seconds,
    )


if __name__ == "__main__":
    raise SystemExit(main())
