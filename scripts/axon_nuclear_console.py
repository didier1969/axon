#!/usr/bin/env python3
import sqlite3
import time
import os
import sys
import datetime

NEXUS_DB = "/home/dstadel/projects/axon/src/dashboard/axon_nexus.db"

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

def get_oban_stats():
    """
    Récupère les vraies statistiques de files d'attente d'Oban (Elixir)
    """
    stats = {
        'available': 0,
        'executing': 0,
        'retryable': 0,
        'hot_available': 0,
        'titan_executing': 0
    }
    
    try:
        conn = sqlite3.connect(NEXUS_DB, timeout=1.0)
        c = conn.cursor()
        
        # Statut global
        c.execute("SELECT state, count(*) FROM oban_jobs GROUP BY state")
        for row in c.fetchall():
            state, count = row[0], row[1]
            if state in stats:
                stats[state] = count
                
        # Inspection fine des files
        c.execute("SELECT queue, count(*) FROM oban_jobs WHERE state = 'available' GROUP BY queue")
        for row in c.fetchall():
            if row[0] == 'indexing_hot':
                stats['hot_available'] = row[1]
                
        c.execute("SELECT count(*) FROM oban_jobs WHERE queue = 'indexing_titan' AND state = 'executing'")
        row = c.fetchone()
        if row:
            stats['titan_executing'] = row[0]

        conn.close()
    except Exception as e:
        pass
        
    return stats

def get_nexus_stats():
    """
    Récupère le statut final des fichiers du point de vue d'Elixir
    """
    stats = {
        'total': 0,
        'indexed': 0,
        'failed': 0,
        'poison': 0,
        'stale': 0,
        'ignored': 0
    }
    
    try:
        conn = sqlite3.connect(NEXUS_DB, timeout=1.0)
        c = conn.cursor()
        c.execute("SELECT count(*) FROM indexed_files")
        row = c.fetchone()
        if row: stats['total'] = row[0]
        
        c.execute("SELECT status, count(*) FROM indexed_files GROUP BY status")
        for row in c.fetchall():
            status, count = row[0], row[1]
            if status == 'indexed': stats['indexed'] = count
            elif status == 'failed': stats['failed'] = count
            elif status == 'poison': stats['poison'] = count
            elif status == 'stale': stats['stale'] = count
            elif status == 'ignored_by_rule': stats['ignored'] = count

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
        now = datetime.datetime.now().strftime("%H:%M:%S.%f")[:-3]
        
        oban = get_oban_stats()
        nexus = get_nexus_stats()
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}{CYAN}          ☢️  AXON NUCLEAR COMMAND CENTER - V2.1 (TOC) ☢️             {RESET}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}SYS_TIME:{RESET} {now}    {BOLD}STATUS:{RESET} {GREEN}ONLINE{RESET}")
        print("")
        
        print(f"{BOLD}{YELLOW}--- [ CONTROL PLANE (OBAN QUEUES) ] ---{RESET}")
        print(f"  {BOLD}BACKLOG (Available):{RESET} {oban['available']:>6} batchs (including {RED}{oban['hot_available']}{RESET} hot path)")
        print(f"  {BOLD}UDS TUNNEL (Exec):{RESET}   {oban['executing']:>6} batchs in Rust RAM (Titan: {MAGENTA}{oban['titan_executing']}{RESET})")
        print(f"  {BOLD}SMART RETRY (Wait):{RESET}  {YELLOW}{oban['retryable']:>6}{RESET} batchs backing off (DB Locked)")
        print("")
        
        print(f"{BOLD}{BLUE}--- [ KNOWLEDGE GRAPH (FILE STATUS) ] ---{RESET}")
        print(f"  {BOLD}TOTAL DISCOVERED:{RESET} {nexus['total']:>8}")
        print(f"  {BOLD}INDEXED (Green):{RESET}  {GREEN}{nexus['indexed']:>8}{RESET}")
        print(f"  {BOLD}STALE (Scanning):{RESET} {nexus['stale']:>8}")
        print(f"  {BOLD}POISON (Fatal):{RESET}   {RED}{nexus['poison']:>8}{RESET} (Dropped by parser)")
        print(f"  {BOLD}IGNORED (Rules):{RESET}  {nexus['ignored']:>8} (Vendor/Assets)")
        print("")
        
        # Calculate visual metrics for the pipeline
        q_total = oban['available'] + oban['executing'] + oban['retryable']
        if q_total == 0: q_total = 1
        
        print(f"{BOLD}OBAN PIPELINE PRESSURE:{RESET}")
        print(f"[{YELLOW}Pending{RESET}] {render_bar(oban['available'], q_total, color=YELLOW)} {oban['available']/q_total*100:.1f}%")
        print(f"[{BLUE}In UDS {RESET}] {render_bar(oban['executing'], q_total, color=BLUE)} {oban['executing']/q_total*100:.1f}%")
        print(f"[{MAGENTA}Retries{RESET}] {render_bar(oban['retryable'], q_total, color=MAGENTA)} {oban['retryable']/q_total*100:.1f}%")
        print("")
        
        # Progress Bar logic (ignoring stale items in completion)
        active_files = nexus['indexed'] + nexus['poison'] + nexus['ignored']
        if nexus['total'] > 0:
            prog = active_files / nexus['total'] * 100
        else:
            prog = 0
            
        print(f"{BOLD}GLOBAL INGESTION PROGRESS:{RESET} {prog:.2f}%")
        print(f"{render_bar(active_files, max(1, nexus['total']), width=68, color=GREEN)}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print("Press Ctrl+C to exit.")
        
        time.sleep(1)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nExiting Command Center.")
        sys.exit(0)
