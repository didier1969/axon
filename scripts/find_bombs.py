import sqlite3
import os

db_path = "/home/dstadel/projects/axon/src/dashboard/axon_nexus.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT path FROM indexed_files WHERE status = 'pending'")
paths = [row[0] for row in cursor.fetchall()]

sizes = []
for p in paths:
    try:
        size = os.path.getsize(p)
        sizes.append((size, p))
    except Exception:
        pass

sizes.sort(reverse=True)
for s, p in sizes[:20]:
    print(f"{s / (1024*1024):.2f} MB : {p}")
