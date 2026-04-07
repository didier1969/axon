import duckdb
import os

conn = duckdb.connect('/home/dstadel/.local/share/axon/db/soll.db')

rows = conn.execute("SELECT project_slug, project_code, project_path FROM soll.ProjectCodeRegistry").fetchall()
print(f"Trouvé {len(rows)} entrées historiques.")

conn.execute("DELETE FROM soll.ProjectCodeRegistry")
conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_slug, project_code, project_path) VALUES ('GLOBAL', 'PRO', NULL)")

base_path = "/home/dstadel/projects"

fixed_projects = [
    ("AXO", "AXO", f"{base_path}/axon"),
    ("BKS", "BKS", f"{base_path}/BookingSystem"),
    ("OPT", "OPT", f"{base_path}/OptiPlanner"),
    ("SWX", "SWX", f"{base_path}/SwarmEx"),
    ("FSC", "FSC", f"{base_path}/Fiscaly"),
    ("TE2", "TE2", f"{base_path}/trader-elixir-v2"),
    ("RMC", "RMC", f"{base_path}/roam-code"),
    ("EXA", "EXA", f"{base_path}/excel-augmented"),
    ("TRI", "TRI", f"{base_path}/triolingo"),
    ("ZCL", "ZCL", f"{base_path}/zeroclaw"),
    ("DPG", "DPG", f"{base_path}/duckdb-graph")
]

restored = set()

for slug, code, path in fixed_projects:
    if os.path.isdir(path):
        conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_slug, project_code, project_path) VALUES (?, ?, ?)", [slug, code, path])
        print(f"Restauré : {slug} -> {path}")
        restored.add(slug)
    else:
        print(f"Ignoré (chemin introuvable) : {slug} -> {path}")

for old_slug, code, old_path in rows:
    if old_slug in ["GLOBAL", "AXO", "BookingSystem", "OptiPlanner", "SwarmEx", "FSC", "TE2", "RMC", "EXA", "TRI", "ZCL", "DPG"]:
        continue
    
    guessed_path = f"{base_path}/{old_slug}"
    if os.path.isdir(guessed_path):
        canon_slug = code if code else old_slug[:3].upper()
        if canon_slug not in restored:
            conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_slug, project_code, project_path) VALUES (?, ?, ?)", [canon_slug, code, guessed_path])
            print(f"Auto-restauré : {canon_slug} -> {guessed_path}")
            restored.add(canon_slug)

print("Migration de l'Omniscience terminée.")
