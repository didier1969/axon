import duckdb
import os

conn = duckdb.connect('/home/dstadel/.local/share/axon/db/soll.db')

rows = conn.execute("SELECT project_code, project_name, project_path FROM soll.ProjectCodeRegistry").fetchall()
print(f"Trouvé {len(rows)} entrées historiques.")

conn.execute("DELETE FROM soll.ProjectCodeRegistry")
conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) VALUES ('PRO', 'System Global Namespace', NULL)")

base_path = "/home/dstadel/projects"

fixed_projects = [
    ("AXO", "axon", f"{base_path}/axon"),
    ("BKS", "BookingSystem", f"{base_path}/BookingSystem"),
    ("OPT", "OptiPlanner", f"{base_path}/OptiPlanner"),
    ("SWX", "SwarmEx", f"{base_path}/SwarmEx"),
    ("FSC", "Fiscaly", f"{base_path}/Fiscaly"),
    ("TE2", "trader-elixir-v2", f"{base_path}/trader-elixir-v2"),
    ("RMC", "roam-code", f"{base_path}/roam-code"),
    ("EXA", "excel-augmented", f"{base_path}/excel-augmented"),
    ("TRI", "triolingo", f"{base_path}/triolingo"),
    ("ZCL", "zeroclaw", f"{base_path}/zeroclaw"),
    ("DPG", "duckdb-graph", f"{base_path}/duckdb-graph")
]

restored = set()

for code, name, path in fixed_projects:
    if os.path.isdir(path):
        conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) VALUES (?, ?, ?)", [code, name, path])
        print(f"Restauré : {code} ({name}) -> {path}")
        restored.add(code)
    else:
        print(f"Ignoré (chemin introuvable) : {code} ({name}) -> {path}")

for old_code, old_name, old_path in rows:
    if old_code in ["PRO", "AXO", "BKS", "OPT", "SWX", "FSC", "TE2", "RMC", "EXA", "TRI", "ZCL", "DPG"]:
        continue

    guessed_name = old_name or (os.path.basename(old_path) if old_path else old_code)
    guessed_path = f"{base_path}/{guessed_name}"
    if os.path.isdir(guessed_path):
        canonical_code = old_code[:3].upper()
        canonical_name = os.path.basename(guessed_path)
        if canonical_code not in restored:
            conn.execute("INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) VALUES (?, ?, ?)", [canonical_code, canonical_name, guessed_path])
            print(f"Auto-restauré : {canonical_code} ({canonical_name}) -> {guessed_path}")
            restored.add(canonical_code)

print("Migration de l'Omniscience terminée.")
