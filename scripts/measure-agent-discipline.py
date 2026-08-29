#!/usr/bin/env python3
"""Mesure la discipline méthodologique d'un agent LLM sur un dépôt Axon.

RÉUTILISE : néant — vérifié via axon query "mcp telemetry tool usage report" et
"transcript session jsonl parser tool_use". Le tool MCP `mcp_telemetry_report`
(src/axon-core/src/mcp/tools_friction.rs) couvre les compteurs par outil côté
SERVEUR, mais ni l'attribution par client (Claude vs Codex), ni le périmètre par
dépôt, ni la normalisation par commit — les trois dimensions qui rendent la
comparaison possible. Ces données ne sont lisibles que côté client, dans les
journaux de session, auxquels le serveur n'a pas accès. Voir REQ-AXO-902555 :
la bonne cible à terme est d'ajouter l'attribution client à la télémétrie
serveur et de retirer ce script.

Compte les appels d'outils Axon MCP par commit livré (`axon_commit_work`).
Le dénominateur est le commit, pas le temps ni le volume brut : deux agents qui
ne travaillent ni les mêmes jours ni sur les mêmes tâches restent comparables
sur « combien de cérémonie par unité de travail livré ».

Les deux formats de journal diffèrent et sont parsés séparément :
  - Claude : ~/.claude/projects/**/*.jsonl, message.content[].type == "tool_use"
  - Codex  : ~/.codex/sessions/**/*.jsonl, payload.item.type == "McpToolCall"

Codex travaille sur tout le parc : ses appels sont filtrés sur le `cwd` courant,
suivi au fil du rollout. Sans ce filtre la comparaison est fausse — mesuré le
2026-08-29 : 8 671 appels Codex répartis sur neuf dépôts, dont 1 078 sur axon.

Usage:
    python3 scripts/measure-agent-discipline.py [--since YYYY-MM-DD] [--repo axon]
"""

import argparse
import collections
import datetime as dt
import glob
import json
import os
import sys

CLAUDE_LOGS = os.path.expanduser("~/.claude/projects/**/*.jsonl")
CODEX_LOGS = os.path.expanduser("~/.codex/sessions/**/*.jsonl")

# Outils suivis : cérémonie, preuve et navigation. L'ordre est celui du rapport.
TRACKED = [
    "soll_query_context",
    "soll_attach_evidence",
    "axon_pre_flight_check",
    "axon_handoff_check",
    "axon_init_project",
    "practice_recall",
    "soll_work_plan",
    "soll_validate",
    "soll_get",
    "query",
    "inspect",
    "impact",
    "skill_invoke",
    "prompt_template_get",
    "sql",
]
DENOMINATOR = "axon_commit_work"


def _recent(pattern, since):
    for path in glob.glob(pattern, recursive=True):
        try:
            if dt.datetime.fromtimestamp(os.path.getmtime(path)) < since:
                continue
        except OSError:
            continue
        yield path


def scan_claude(since, repo):
    """Transcripts Claude Code. Le dépôt est encodé dans le chemin du répertoire."""
    counts = collections.Counter()
    for path in _recent(CLAUDE_LOGS, since):
        if repo and repo not in path:
            continue
        with open(path, errors="replace") as fh:
            for line in fh:
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                content = (entry.get("message") or {}).get("content")
                if not isinstance(content, list):
                    continue
                for block in content:
                    if isinstance(block, dict) and block.get("type") == "tool_use":
                        name = block.get("name", "")
                        if name.startswith("mcp__axon__"):
                            counts[name[len("mcp__axon__"):]] += 1
    return counts


def scan_codex(since, repo):
    """Rollouts Codex. Le cwd change en cours de session : on le suit."""
    counts = collections.Counter()
    for path in _recent(CODEX_LOGS, since):
        cwd = None
        with open(path, errors="replace") as fh:
            for line in fh:
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                payload = entry.get("payload") or {}
                envs = ((payload.get("state") or {}).get("environments") or {}).get(
                    "environments"
                ) or {}
                for env in envs.values():
                    if isinstance(env, dict) and env.get("cwd"):
                        cwd = env["cwd"]
                item = payload.get("item") or {}
                if item.get("type") != "McpToolCall":
                    continue
                if repo and not (cwd or "").rstrip("/").endswith("/" + repo):
                    continue
                counts[item.get("tool") or item.get("name") or "?"] += 1
    return counts


def report(label, counts):
    commits = counts.get(DENOMINATOR, 0)
    print(f"\n### {label} — {commits} commit(s), {sum(counts.values())} appel(s) Axon")
    if not commits:
        print("  (aucun commit sur la fenêtre — ratio non calculable)")
        return {}
    ratios = {}
    for tool in TRACKED:
        ratios[tool] = counts.get(tool, 0) / commits
        print(f"  {tool:24s} {ratios[tool]:6.2f}   (brut {counts.get(tool, 0)})")
    return ratios


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--since", default="2026-08-01", help="date plancher YYYY-MM-DD")
    parser.add_argument("--repo", default="axon", help="nom du dépôt à isoler ('' = tous)")
    args = parser.parse_args()

    try:
        since = dt.datetime.strptime(args.since, "%Y-%m-%d")
    except ValueError:
        sys.exit(f"date invalide : {args.since}")

    print(f"Fenêtre : depuis {args.since} · dépôt : {args.repo or 'tous'}")
    print(f"Dénominateur : {DENOMINATOR}")

    claude = report("CLAUDE", scan_claude(since, args.repo))
    codex = report("CODEX", scan_codex(since, args.repo))

    if claude and codex:
        print("\n### Écart (Codex / Claude) — >1 signifie que Codex en fait plus")
        for tool in TRACKED:
            c, x = claude.get(tool, 0), codex.get(tool, 0)
            gap = "n/a" if not c else f"{x / c:.1f}x"
            print(f"  {tool:24s} claude {c:5.2f}   codex {x:5.2f}   {gap}")


if __name__ == "__main__":
    main()
