import sqlite3
conn = sqlite3.connect("/home/dstadel/projects/axon/src/dashboard/axon_nexus.db")
conn.execute("DELETE FROM oban_jobs")
conn.commit()
print("Purged Oban Queue completely")
