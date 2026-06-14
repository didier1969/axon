#!/usr/bin/env python3
"""Axon MCP-first PreToolUse guard (GUI-PRO-112 / CPT-PRO-100).

Environmental enforcement of MCP-first: when an Axon MCP server is available,
code search/navigation must go through Axon (query/inspect/retrieve_context/
impact/why/path), not grep/rg/find. The harness runs this hook, not the LLM,
so the model cannot drift back to grep under latency pressure.

Wire it as a Claude Code PreToolUse hook on the `Bash` tool (see
settings-snippet.json). Protocol: read the tool call as JSON on stdin; exit 0 to
allow, exit 2 with a stderr message to BLOCK (the message is fed to the model).

Design (deliberate, fail-OPEN):
- Only blocks Bash commands that SEARCH CODE: a top-level grep/egrep/rg/ag/ack
  over files, or `find … -name "*.<codeext>"`, or `ls -R`.
- Never blocks `… | grep …` (filtering another command's output, not a search).
- Escapes: `AXON_OK=1` inline (legitimate literal-string search) ; env
  `AXON_MCP_ENFORCE=0` (operator global off).
- FAIL-OPEN: if the Axon MCP endpoint is unreachable, ALLOW — never break a
  consumer when Axon is down. (Probe only when about to block; short timeout.)
"""
import json
import os
import re
import shutil
import sys
import urllib.request

CODE_SEARCH_BINS = {"grep", "egrep", "fgrep", "rg", "ag", "ack", "ack-grep"}
# Extensions that signify a code/structure search (not a log/config grep).
CODE_EXT = (
    "rs ex exs erl py js ts tsx jsx go java kt kts rb php c h cpp cc hpp cs "
    "scala clj swift m mm sql graphql proto"
).split()


def _segments(command: str):
    """Split a shell line into pipeline segments, dropping `| grep` filters.

    We only care about the FIRST binary of each segment that is NOT the
    right-hand side of a pipe — a piped grep is output filtering, not a search.
    Returns the list of leading tokens of non-piped segments.
    """
    # Split on ; && || newline into statements; within each, take the part
    # before the first pipe (the producer), whose leading bin is the "search".
    leads = []
    for stmt in re.split(r"(?:&&|\|\||;|\n)", command):
        producer = stmt.split("|", 1)[0].strip()
        if producer:
            leads.append(producer)
    return leads


def _is_code_search(command: str) -> bool:
    for lead in _segments(command):
        toks = lead.split()
        if not toks:
            continue
        # skip leading env assignments (FOO=bar grep ...) and `sudo`
        i = 0
        while i < len(toks) and ("=" in toks[i] and not toks[i].startswith("-")):
            i += 1
        if i >= len(toks):
            continue
        bin_ = os.path.basename(toks[i])
        rest = " ".join(toks[i + 1 :])
        if bin_ in CODE_SEARCH_BINS:
            # rg/ag/ack default to recursive code search; grep counts when it
            # targets files/paths or -r/-R (not reading stdin).
            if bin_ in {"rg", "ag", "ack", "ack-grep"}:
                return True
            if re.search(r"(^|\s)-[a-zA-Z]*[rR]", rest) or re.search(r"\s\S*\.\w+(\s|$)", rest) or re.search(r"\s\S+/", rest):
                return True
        if bin_ == "find" and re.search(r"-i?name\s+['\"]?\*?\.\w+", rest):
            ext = re.search(r"\.(\w+)", rest)
            if ext and ext.group(1) in CODE_EXT:
                return True
        if bin_ == "ls" and re.search(r"(^|\s)-[a-zA-Z]*R", rest):
            return True
    return False


def _axon_reachable() -> bool:
    """Fail-open probe. Returns False (=> allow grep) when Axon is unreachable."""
    url = os.environ.get("AXON_MCP_URL", "http://127.0.0.1:44129/mcp")
    try:
        req = urllib.request.Request(
            url,
            data=b'{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}',
            headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=0.6) as resp:
            return resp.status == 200
    except Exception:
        return False


def main() -> int:
    if os.environ.get("AXON_MCP_ENFORCE", "1") == "0":
        return 0
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0  # fail-open on any parse issue
    if payload.get("tool_name") != "Bash":
        return 0
    command = (payload.get("tool_input") or {}).get("command", "") or ""
    if "AXON_OK=1" in command:
        return 0
    if not _is_code_search(command):
        return 0
    # About to block — but never break the consumer if Axon is actually down.
    if not _axon_reachable():
        return 0
    sys.stderr.write(
        "Axon MCP is available — use it instead of grep/find for code search.\n"
        "  • find a symbol            -> query(\"<name>\")\n"
        "  • inspect / callers / flow -> inspect / impact / path / why\n"
        "  • evidence for a question  -> retrieve_context(\"<question>\")\n"
        "Axon returns the symbol + its callers + the governing intent in one call, "
        "which grep cannot. For a legitimate literal-string search, re-run with "
        "AXON_OK=1 prefixed. To disable this guard, set AXON_MCP_ENFORCE=0.\n"
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
