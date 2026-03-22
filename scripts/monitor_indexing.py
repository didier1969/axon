import sqlite3
import time
import sys
import os
from datetime import datetime

# Chemin absolu garanti pour que le script marche depuis n'importe où dans WSL
db_path = "/home/dstadel/projects/axon/src/dashboard/axon_nexus.db"

def get_stats():
    try:
        conn = sqlite3.connect(db_path)
        cursor = conn.cursor()
        cursor.execute("SELECT count(*) FROM indexed_files")
        total = cursor.fetchone()[0]
        cursor.execute("SELECT count(*) FROM indexed_files WHERE status = 'indexed'")
        indexed = cursor.fetchone()[0]
        conn.close()
        return total, indexed
    except Exception as e:
        return 0, 0

if not os.path.exists(db_path):
    print(f"❌ Erreur : La base de données n'a pas été trouvée à {db_path}")
    print("Assurez-vous qu'Axon V2 est en cours d'exécution.")
    sys.exit(1)

print(f"[{datetime.now().strftime('%H:%M:%S')}] Démarrage du monitoring de l'indexation Axon...")
print("Pressez Ctrl+C pour arrêter le monitoring.")
print("-" * 80)

start_time = time.time()
initial_total, initial_indexed = get_stats()
last_indexed = initial_indexed
i = 0

try:
    while True:
        i += 1
        time.sleep(20)
        current_time = time.time()
        
        total, indexed = get_stats()
        
        # Calcul de la vitesse sur les 20 dernières secondes
        speed_last_20s = (indexed - last_indexed) / 20.0
        
        # Calcul de la vitesse moyenne depuis le début
        elapsed_total = current_time - start_time
        avg_speed = (indexed - initial_indexed) / elapsed_total if elapsed_total > 0 else 0
        
        # Estimation du temps restant (sur base de la vitesse moyenne)
        remaining = total - indexed
        eta = (remaining / avg_speed) if avg_speed > 0 else 0
        eta_str = f"{int(eta // 60)}m {int(eta % 60)}s" if avg_speed > 0 else "N/A"
        
        # Barre de progression simple
        percent = (indexed / total * 100) if total > 0 else 0
        bar_len = 20
        filled = int(percent / 100 * bar_len)
        bar = "█" * filled + "-" * (bar_len - filled)
        
        print(f"[{datetime.now().strftime('%H:%M:%S')}] T+{i*20}s | Progression: {percent:05.1f}% [{bar}]")
        print(f"   => Fichiers : {indexed} indexés / {total} découverts")
        print(f"   => Vitesse instantanée : {speed_last_20s:04.1f} f/s | Vitesse moyenne : {avg_speed:04.1f} f/s | ETA : {eta_str}")
        print("-" * 80)
        
        last_indexed = indexed
        sys.stdout.flush()

        if total > 0 and indexed >= total and speed_last_20s == 0:
             print(f"[{datetime.now().strftime('%H:%M:%S')}] Indexation terminée ou en veille prolongée.")
             break

except KeyboardInterrupt:
    print(f"\n[{datetime.now().strftime('%H:%M:%S')}] Arrêt manuel du monitoring.")
    sys.exit(0)
