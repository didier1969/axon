#!/usr/bin/env python3
"""REQ-AXO-902041 — backfill soll.Traceability for delivered nodes lacking
evidence, replaying REQ-AXO-159's auto_attach_commit_evidence over historical
commits. Deterministic trace id (node+sha) + ON CONFLICT DO NOTHING => idempotent.

Usage: backfill_traceability_902041.py [--apply]   (default: dry-run, counts only)
"""
import subprocess, re, json, time, sys

URL = "postgres://axon@127.0.0.1:44144/axon_live"
PSQL = "/nix/store/yap66ka10pgkjlgaws8zk73lj3gnj1bl-postgresql-and-plugins-17.9/bin/psql"
REPO = "/home/dstadel/projects/axon"
APPLY = "--apply" in sys.argv
ID_RE = re.compile(r'\b(?:REQ|MIL|DEC|CPT|GUI|VAL|PIL|VIS|STK|SKI|PRT)-AXO-\d+\b')


def psql(sql, capture=True):
    r = subprocess.run([PSQL, URL, "-tAF\x1f", "-c", sql],
                       capture_output=True, text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stderr)
        raise SystemExit(f"psql failed: {r.stderr[:200]}")
    return r.stdout


def sql_lit(s):
    return "'" + s.replace("'", "''") + "'"


# 1. target nodes: delivered/completed/accepted AXO nodes with NO traceability row
rows = psql(
    "SELECT id, type FROM soll.Node WHERE project_code='AXO' "
    "AND status IN ('delivered','completed','accepted') "
    "AND NOT EXISTS (SELECT 1 FROM soll.Traceability t WHERE t.soll_entity_id=soll.Node.id)")
targets = {}
for line in rows.splitlines():
    if not line.strip():
        continue
    nid, ntype = line.split("\x1f")
    targets[nid] = ntype.lower()
print(f"target delivered nodes without traceability: {len(targets)}")

# 2. walk full git history, map each referenced target id -> its commit shas
log = subprocess.run(
    ["git", "-C", REPO, "log", "--format=%H%x1f%B%x1e"],
    capture_output=True, text=True).stdout
now = int(time.time() * 1000)
inserts = []
seen = set()
for chunk in log.split("\x1e"):
    if "\x1f" not in chunk:
        continue
    sha, body = chunk.split("\x1f", 1)
    sha = sha.strip()
    if not sha:
        continue
    subject = (body.strip().splitlines() or [""])[0][:200]
    for nid in set(ID_RE.findall(body)):
        if nid in targets and (nid, sha) not in seen:
            seen.add((nid, sha))
            trace_id = f"TRC-{nid}-{sha[:12]}-bf"
            meta = json.dumps({"source": "backfill_902041", "subject": subject})
            inserts.append((trace_id, targets[nid], nid, sha, meta))

nodes_covered = len({nid for (_, _, nid, _, _) in inserts})
print(f"candidate trace rows: {len(inserts)} covering {nodes_covered} distinct nodes")
print(f"nodes still without any commit evidence: {len(targets) - nodes_covered}")

if not APPLY:
    print("DRY-RUN — pass --apply to insert.")
    raise SystemExit(0)

# 3. apply in batches
BATCH = 200
applied = 0
for i in range(0, len(inserts), BATCH):
    batch = inserts[i:i + BATCH]
    values = ",".join(
        f"({sql_lit(tid)},{sql_lit(etype)},{sql_lit(nid)},'Commit',{sql_lit(sha)},"
        f"0.6,{sql_lit(meta)}::jsonb,{now})"
        for (tid, etype, nid, sha, meta) in batch)
    psql(
        "INSERT INTO soll.Traceability "
        "(id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, "
        "confidence, metadata, created_at) VALUES " + values +
        " ON CONFLICT (id) DO NOTHING", capture=False)
    applied += len(batch)
print(f"applied {applied} INSERT rows (ON CONFLICT DO NOTHING).")
