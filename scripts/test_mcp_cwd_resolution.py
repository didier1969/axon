#!/usr/bin/env python3
"""E2E live — le tunnel MCP porte le cwd CLIENT jusqu'au resolver du brain.

REQ-AXO-902286 (M3, transport) + REQ-AXO-902287 (M1, divulgation de provenance).
Tripwire du gap résiduel REQ-AXO-902291 (repli littéral « AXO » des handlers).

## Pourquoi ce script existe (GUI-PRO-118)

Le brain live est PARTAGÉ : son propre cwd est le dépôt Axon. Avant M3, tout
client résolvait donc `AXO`, quel que soit le projet dans lequel il travaillait
— silencieusement. Le fix fait porter `X-Axon-Client-Cwd` par le tunnel sur
chaque POST vers le brain.

La régression correspondante est INVISIBLE depuis le dépôt Axon : un test lancé
depuis `cwd=<axon>` répond `AXO` aussi bien avec le fix que sans. Seul un appel
émis depuis un AUTRE projet enregistré discrimine. C'est ce que fait ce script,
avec le binaire tunnel réellement déployé — pas une simulation curl, qui ne
prouverait que la moitié brain.

## Ce qui est vérifié

1. pair      : tunnel lancé depuis un projet enregistré ≠ AXO → ses outils
               project-scoped répondent sur CE projet (le fix M3 fonctionne).
2. soi       : tunnel lancé depuis le dépôt Axon → `AXO` (zéro régression,
               REQ-AXO-902239 intact).
3. inconnu   : cwd non enregistré → le mailbox REFUSE explicitement au lieu de
               retomber en silence sur le projet du serveur (REQ-AXO-902287).

## Known-gaps

Un outil listé dans KNOWN_GAPS est attendu ROUGE : le défaut est tracé en SOLL,
pas oublié (GUI-AXO-1023). Il ne fait pas échouer le script — mais s'il passe
au vert, le script le DIT : le REQ est livré, la déclaration doit disparaître.

Usage :
    python3 scripts/test_mcp_cwd_resolution.py [--tunnel PATH] [--peer PATH]

Prérequis : le brain live doit répondre (le wrapper `axon-mcp` le démarre au
besoin). Sortie 0 = tous les cas non-déclarés sont verts.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_TUNNEL = os.path.join(REPO_ROOT, "bin", "axon-mcp-tunnel-static")
UNREGISTERED_CWD = "/tmp"
RPC_TIMEOUT_S = 120

# Outils project-scoped qui impriment le projet résolu dans leur réponse.
# `probe` = arguments minimaux et NON destructifs.
SCOPED_TOOLS: list[tuple[str, dict]] = [
    ("mcp_inbox_read", {"mode": "all", "limit": 1}),
    ("soll_roadmap", {}),
    ("soll_query_context", {"limit": 1}),
]

# Outil -> REQ qui trace son défaut. Rouge attendu, vert = REQ livré.
#
# Vide depuis REQ-AXO-902291 (2026-08-14) : `soll_query_context` y figurait
# parce qu'il répondait `AXO` à un client d'un autre projet — son handler
# repliait sur un littéral au lieu de passer par le chokepoint. Les dix outils
# read-only de cette forme sont désormais dans `PROJECT_AUTORESOLVE_TOOLS`, et
# le cas 1 ci-dessous le vérifie de bout en bout. Une entrée ici doit rester
# l'exception tracée, jamais le rangement d'un cas gênant.
KNOWN_GAPS: dict[str, str] = {}

# Le projet résolu est imprimé sous des formes différentes selon l'outil ;
# on lit CETTE déclaration, jamais le corps de la réponse (un message d'inbox
# cite d'autres codes projet en toute légitimité).
PROJECT_PATTERNS = [
    re.compile(r"projet `([A-Z][A-Z0-9]{1,7})` déduit du cwd"),   # divulgation 902287
    re.compile(r"^`([A-Z][A-Z0-9]{1,7})` _\(déduit du cwd", re.M),  # bandeau mailbox
    re.compile(r"^Roadmap ([A-Z][A-Z0-9]{1,7})\b", re.M),
    re.compile(r"^SOLL context for ([A-Z][A-Z0-9]{1,7})\b", re.M),
]


def resolved_project(text: str) -> str | None:
    """Le code projet que la réponse DÉCLARE avoir résolu, ou None."""
    for pattern in PROJECT_PATTERNS:
        found = pattern.search(text)
        if found:
            return found.group(1)
    return None


def _rpc(rid: int, method: str, params: dict | None = None) -> str:
    msg: dict = {"jsonrpc": "2.0", "id": rid, "method": method}
    if params is not None:
        msg["params"] = params
    return json.dumps(msg)


def call_tools(tunnel: str, cwd: str, calls: list[tuple[str, dict]]) -> dict[int, str]:
    """Un seul lancement de tunnel pour N tools/call — retourne {id: texte}."""
    lines = [
        _rpc(1, "initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test_mcp_cwd_resolution", "version": "1.0"},
        }),
        json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ]
    for i, (name, args) in enumerate(calls, start=2):
        lines.append(_rpc(i, "tools/call", {"name": name, "arguments": args}))

    proc = subprocess.run(
        [tunnel],
        cwd=cwd,
        input="\n".join(lines) + "\n",
        capture_output=True,
        text=True,
        timeout=RPC_TIMEOUT_S,
    )

    out: dict[int, str] = {}
    for raw in proc.stdout.splitlines():
        raw = raw.strip()
        if not raw.startswith("{"):
            continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        rid = obj.get("id")
        if not isinstance(rid, int) or rid < 2:
            continue
        content = obj.get("result", {}).get("content", [])
        out[rid] = (content[0].get("text", "") if content
                    else json.dumps(obj.get("error", obj)))
    if not out and proc.stderr.strip():
        print(f"    [stderr] {proc.stderr.strip()[:400]}")
    return out


def discover_peer_project(tunnel: str) -> tuple[str, str] | None:
    """Un projet enregistré ≠ AXO dont la racine existe — jamais codé en dur."""
    res = call_tools(tunnel, REPO_ROOT, [("sql", {
        "sql": "SELECT code, root_path FROM axon.project "
               "WHERE code <> 'AXO' AND root_path IS NOT NULL ORDER BY code",
    })])
    try:
        rows = json.loads(res.get(2, "[]"))
    except json.JSONDecodeError:
        return None
    for row in rows:
        if len(row) >= 2 and row[1] and os.path.isdir(row[1]):
            return row[0], row[1]
    return None


class Report:
    """Compte les verdicts ; un known-gap rouge n'échoue pas, un vert alerte."""

    def __init__(self) -> None:
        self.failed = 0
        self.gaps_still_red: list[str] = []
        self.gaps_now_green: list[str] = []

    def check(self, tool: str, ok: bool, detail: str) -> None:
        req = KNOWN_GAPS.get(tool)
        if req and not ok:
            self.gaps_still_red.append(f"{tool} → {req}")
            print(f"  [GAP ] {tool} — {detail}  (attendu, tracé {req})")
            return
        if req and ok:
            self.gaps_now_green.append(f"{tool} → {req}")
            print(f"  [NEUF] {tool} — {detail}  ({req} semble livré)")
            return
        if not ok:
            self.failed += 1
        print(f"  [{'PASS' if ok else 'FAIL'}] {tool} — {detail}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tunnel", default=DEFAULT_TUNNEL)
    ap.add_argument("--peer", default=None,
                    help="Racine d'un projet enregistré ≠ AXO (sinon découvert).")
    args = ap.parse_args()

    if not os.path.isfile(args.tunnel):
        print(f"BLOQUÉ : binaire tunnel introuvable — {args.tunnel}\n"
              f"  Reconstruire : bash scripts/release/build_tunnel_static.sh")
        return 2

    if args.peer:
        peer_path, peer_code = args.peer, None
    else:
        found = discover_peer_project(args.tunnel)
        if not found:
            print("BLOQUÉ : aucun projet enregistré ≠ AXO avec une racine "
                  "existante — impossible de discriminer.")
            return 2
        peer_code, peer_path = found

    print(f"Tunnel : {args.tunnel}")
    print(f"Pair   : {peer_code} @ {peer_path}\n")
    report = Report()

    # --- Cas 1 : projet pair — LE cas discriminant -------------------------
    print(f"Cas 1 — cwd = {peer_path} (attendu : {peer_code})")
    res = call_tools(args.tunnel, peer_path, SCOPED_TOOLS)
    for i, (name, _) in enumerate(SCOPED_TOOLS, start=2):
        got = resolved_project(res.get(i, ""))
        report.check(name, got == peer_code, f"projet résolu = {got or '(non déclaré)'}")

    # --- Cas 2 : dépôt Axon — non-régression REQ-AXO-902239 ----------------
    print(f"\nCas 2 — cwd = {REPO_ROOT} (attendu : AXO, non-régression)")
    res = call_tools(args.tunnel, REPO_ROOT, SCOPED_TOOLS)
    for i, (name, _) in enumerate(SCOPED_TOOLS, start=2):
        got = resolved_project(res.get(i, ""))
        # Un known-gap répond AXO ici aussi, mais pour la mauvaise raison :
        # on ne peut pas distinguer, donc on ne compte pas ce cas pour lui.
        if name in KNOWN_GAPS:
            print(f"  [ -- ] {name} — indiscernable depuis le dépôt Axon")
            continue
        report.check(name, got == "AXO", f"projet résolu = {got or '(non déclaré)'}")

    # --- Cas 3 : cwd non enregistré — pas de default silencieux ------------
    print(f"\nCas 3 — cwd = {UNREGISTERED_CWD} (non enregistré)")
    inbox = res_unknown = call_tools(
        args.tunnel, UNREGISTERED_CWD, [SCOPED_TOOLS[0]]
    ).get(2, "")
    refuses = "unresolved" in inbox.lower()
    report.check("mcp_inbox_read",
                 refuses and resolved_project(inbox) is None,
                 f"réponse = {inbox.splitlines()[0][:80] if inbox else '(vide)'}")

    # --- Verdict -----------------------------------------------------------
    print()
    for gap in report.gaps_still_red:
        print(f"GAP DÉCLARÉ (non bloquant) : {gap}")
    for gap in report.gaps_now_green:
        print(f"À RETIRER : {gap} est passé au vert — supprimer sa ligne "
              f"de KNOWN_GAPS et laisser le test le garder.")
    print("VERDICT : PASS — le cwd client atteint le resolver du brain."
          if report.failed == 0 else
          f"VERDICT : FAIL — {report.failed} cas rouge(s) non déclaré(s).")
    return 0 if report.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
