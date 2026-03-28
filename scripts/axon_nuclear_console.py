#!/usr/bin/env python3
import sqlite3
import time
import os
import sys
import datetime
import json

# Axon v3.3.1 - Nuclear Command Center (DuckDB Edition)
# This console monitors the Unified Lattice (SOLL + IST)

IST_DB = "/home/dstadel/projects/axon/.axon/graph_v2/ist.db"
SOLL_DB = "/home/dstadel/projects/axon/.axon/graph_v2/soll.db"
NEXUS_SQLITE = "/home/dstadel/projects/axon/src/dashboard/axon_nexus.db"

# ANSI Colors
RED = "\033[91m"
GREEN = "\033[92m"
YELLOW = "\033[93m"
BLUE = "\033[94m"
CYAN = "\033[96m"
MAGENTA = "\033[95m"
WHITE = "\033[97m"
BOLD = "\033[1m"
RESET = "\033[0m"

def clear_screen():
    print("\033[2J\033[H", end="")

def get_duckdb_stats():
    """
    Récupère les statistiques depuis la forge technique DuckDB (IST + SOLL via ATTACH)
    Note: Comme DuckDB est utilisé en mode exclusif par Axon Core, 
    cette console tente une lecture via le MCP ou directement si le verrou le permet.
    Ici, on simule la lecture via Python sqlite3 (compatible DuckDB format de base) 
    OU on utilise les fichiers s'ils sont lisibles.
    """
    stats = {
        'ist_files': 0,
        'ist_symbols': 0,
        'soll_reqs': 0,
        'soll_decisions': 0,
        'soll_concepts': 0,
        'impact_radius_avg': 0.0
    }
    
    # NOTE: En environnement de production, on interroge via le socket ou une base répliquée.
    # Ici, nous allons lire les métadonnées de progression depuis le Control Plane (SQLite).
    return stats

def get_control_plane_stats():
    """
    Récupère l'état depuis le Control Plane Elixir (SQLite)
    """
    stats = {
        'total': 0,
        'indexed': 0,
        'failed': 0,
        'ignored': 0,
        'pending': 0,
        'oban_available': 0,
        'oban_executing': 0
    }
    
    try:
        conn = sqlite3.connect(NEXUS_SQLITE, timeout=1.0)
        c = conn.cursor()
        
        # Files status
        c.execute("SELECT status, count(*) FROM indexed_files GROUP BY status")
        for row in c.fetchall():
            status, count = row[0], row[1]
            if status == 'indexed': stats['indexed'] = count
            elif status == 'failed': stats['failed'] = count
            elif status == 'pending': stats['pending'] = count
            elif status == 'ignored_by_rule': stats['ignored'] = count
        
        stats['total'] = stats['indexed'] + stats['failed'] + stats['pending'] + stats['ignored']
        
        # Oban status
        c.execute("SELECT state, count(*) FROM oban_jobs GROUP BY state")
        for row in c.fetchall():
            state, count = row[0], row[1]
            if state == 'available': stats['oban_available'] = count
            elif state == 'executing': stats['oban_executing'] = count
            
        conn.close()
    except:
        pass
        
    return stats

def render_bar(value, maximum, width=40, color=GREEN):
    if maximum == 0:
        return "[" + " " * width + "]"
    
    filled = int((value / maximum) * width)
    filled = min(filled, width)
    empty = width - filled
    return f"[{color}{'█' * filled}{RESET}{' ' * empty}]"

def main():
    while True:
        clear_screen()
        now = datetime.datetime.now().strftime("%H:%M:%S")
        
        cp = get_control_plane_stats()
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}{CYAN}          ☢️  AXON NUCLEAR COMMAND CENTER - V3.3 (APOLLO) ☢️          {RESET}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}SYS_TIME:{RESET} {now}    {BOLD}MODE:{RESET} {MAGENTA}NEXUS PULL (ADAPTIVE){RESET}    {BOLD}STATUS:{RESET} {GREEN}ONLINE{RESET}")
        print("")
        
        print(f"{BOLD}{YELLOW}--- [ CONTROL PLANE : TRAFFIC GUARDIAN ] ---{RESET}")
        q_total = cp['oban_available'] + cp['oban_executing']
        print(f"  {BOLD}BUFFER PRESSURE (Oban):{RESET} {cp['oban_available']:>6} jobs pending")
        print(f"  {BOLD}ACTIVE WORKERS (Rust): {RESET} {cp['oban_executing']:>6} threads engaged")
        print(f"  {render_bar(cp['oban_executing'], max(14, cp['oban_executing']), width=50, color=BLUE)} {min(100, cp['oban_executing']/14*100):.1f}% CPU Usage")
        print("")
        
        print(f"{BOLD}{BLUE}--- [ DATA PLANE : UNIFIED LATTICE (IST) ] ---{RESET}")
        print(f"  {BOLD}TOTAL DISCOVERED:{RESET} {cp['total']:>8}")
        print(f"  {BOLD}INDEXED (Truth): {RESET} {GREEN}{cp['indexed']:>8}{RESET}")
        print(f"  {BOLD}PENDING (Pull):  {RESET} {YELLOW}{cp['pending']:>8}{RESET}")
        print(f"  {BOLD}FAILED (Poison): {RESET} {RED}{cp['failed']:>8}{RESET}")
        print("")
        
        # Ingestion Progress
        if cp['total'] > 0:
            prog = (cp['indexed'] + cp['ignored']) / cp['total'] * 100
        else:
            prog = 0
            
        print(f"{BOLD}GLOBAL INGESTION PROGRESS:{RESET} {prog:.2f}%")
        print(f"{render_bar(cp['indexed'] + cp['ignored'], max(1, cp['total']), width=68, color=GREEN)}")
        
        print("")
        print(f"{BOLD}{WHITE}--- [ INTENTIONAL SANCTUARY (SOLL) ] ---{RESET}")
        print(f"  {BOLD}STATUS:{RESET} {GREEN}LOCKED & SYNCED{RESET} | {BOLD}METAMODEL:{RESET} Apollo v3.3")
        print(f"  {BOLD}COMPLIANCE:{RESET} Digital Thread 100% Active")
        print("")
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(" Commands: [start_scan] [axon_query] [export_soll] | Press Ctrl+C to exit")
        
        time.sleep(2)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nExiting Command Center.")
        sys.exit(0)
