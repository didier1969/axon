import time
import socket
import sqlite3
import os
import sys

TELEMETRY_SOCK = "/tmp/axon-telemetry.sock"
DB_PATH = "/home/dstadel/projects/axon/.axon/run/tasks.db"

def benchmark():
    print("🚀 Démarrage du Benchmark de Résilience (Axon v2 SQLite Edition)")
    
    # 1. Vérification de l'accès à la DB
    if not os.path.exists(DB_PATH):
        print(f"❌ Erreur: Base de données introuvable à {DB_PATH}")
        sys.exit(1)
        
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    
    # Reset queue
    cursor.execute("DELETE FROM queue")
    conn.commit()

    # 2. Déclenchement du Scan via UDS
    print("📡 Envoi de la commande SCAN_ALL...")
    t_start_scan = time.time()
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(TELEMETRY_SOCK)
        sock.sendall(b"SCAN_ALL\n")
        sock.close()
    except Exception as e:
        print(f"❌ Erreur de connexion au socket: {e}")
        sys.exit(1)

    # 3. Mesure du temps de Scan (Insertion SQLite)
    max_files = 0
    print("⏳ Attente de la fin du scan...")
    while True:
        time.sleep(1)
        cursor.execute("SELECT count(*) FROM queue")
        count = cursor.fetchone()[0]
        if count == max_files and count > 0:
            # Si le compte n'augmente plus pendant 1s, le scan est fini
            break
        max_files = max(max_files, count)
        print(f"  ... {count} fichiers trouvés dans la queue")

    t_end_scan = time.time()
    scan_duration = t_end_scan - t_start_scan
    
    print(f"\n✅ [ÉTAPE 1] SCAN TERMINE : {max_files} fichiers découverts et stockés dans SQLite en {scan_duration:.2f} secondes.")
    print(f"⚡ Vitesse d'ingestion (SQLite WAL) : {max_files / scan_duration:.2f} fichiers/seconde.")

    # 4. Mesure du temps de Traitement (Parsing + Embeddings)
    print("\n⏳ Suivi de la consommation par les Workers...")
    t_start_process = time.time()
    
    last_pending = max_files
    while True:
        time.sleep(2)
        cursor.execute("SELECT count(*) FROM queue WHERE status = 'PENDING'")
        pending = cursor.fetchone()[0]
        
        cursor.execute("SELECT count(*) FROM queue WHERE status = 'PROCESSING'")
        processing = cursor.fetchone()[0]
        
        cursor.execute("SELECT count(*) FROM queue WHERE status = 'DONE'")
        done = cursor.fetchone()[0]

        print(f"  📊 ÉTAT : {pending} En attente | {processing} En cours | {done} Terminés")
        
        if pending == 0 and processing == 0:
            break
        
        # Security to avoid infinite loop in benchmark
        if pending == last_pending and processing == 0:
             print("⚠️ Attention: Les workers semblent bloqués.")
             break
        last_pending = pending

    t_end_process = time.time()
    process_duration = t_end_process - t_start_process
    
    print(f"\n✅ [ÉTAPE 2] TRAITEMENT TERMINÉ en {process_duration:.2f} secondes.")
    if process_duration > 0:
        print(f"⚡ Vitesse de traitement IA/AST : {max_files / process_duration:.2f} fichiers/seconde.")
    
    print(f"\n🏆 TEMPS TOTAL (Scan + Traitement) : {scan_duration + process_duration:.2f} secondes.")

if __name__ == '__main__':
    benchmark()
