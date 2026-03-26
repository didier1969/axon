#!/usr/bin/env python3
import sqlite3
import time
import os
import sys
import datetime

QUEUE_DB = "/home/dstadel/projects/axon/.axon/run/tasks.db"
NEXUS_DB = "/home/dstadel/projects/axon/src/dashboard/axon_nexus.db"

# ANSI Colors
RED = "\033[91m"
GREEN = "\033[92m"
YELLOW = "\033[93m"
BLUE = "\033[94m"
CYAN = "\033[96m"
WHITE = "\033[97m"
BOLD = "\033[1m"
RESET = "\033[0m"

def clear_screen():
    print("\033[2J\033[H", end="")

def get_queue_stats():
    try:
        conn = sqlite3.connect(QUEUE_DB, timeout=1.0)
        c = conn.cursor()
        c.execute("SELECT count(*) FROM queue WHERE status = 'PENDING'")
        pending = c.fetchone()[0]
        c.execute("SELECT count(*) FROM queue WHERE status = 'PROCESSING'")
        processing = c.fetchone()[0]
        c.execute("SELECT count(*) FROM queue WHERE status = 'DONE'")
        done = c.fetchone()[0]
        conn.close()
        return pending, processing, done
    except:
        return 0, 0, 0

def get_nexus_stats():
    try:
        conn = sqlite3.connect(NEXUS_DB, timeout=1.0)
        c = conn.cursor()
        c.execute("SELECT count(*) FROM indexed_files")
        total = c.fetchone()[0]
        c.execute("SELECT count(*) FROM indexed_files WHERE status = 'indexed'")
        indexed = c.fetchone()[0]
        c.execute("SELECT count(*) FROM indexed_files WHERE status = 'failed'")
        failed = c.fetchone()[0]
        conn.close()
        return total, indexed, failed
    except:
        return 0, 0, 0

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
        
        pending, processing, done = get_queue_stats()
        total_discovered, indexed, failed = get_nexus_stats()
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}{CYAN}             ☢️  AXON SYSTEM COMMAND CENTER - CORE V2 ☢️             {RESET}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}SYS_TIME:{RESET} {now}    {BOLD}STATUS:{RESET} {GREEN}ONLINE{RESET}")
        print("")
        
        print(f"{BOLD}{YELLOW}--- [ RUST CORE QUEUE (Backpressure) ] ---{RESET}")
        print(f"  {BOLD}PENDING:{RESET}    {pending:>8} files waiting for CPU")
        print(f"  {BOLD}PROCESSING:{RESET} {processing:>8} files being embedded (ONNX)")
        print(f"  {BOLD}DONE:{RESET}       {done:>8} files committed to KuzuDB")
        print("")
        
        print(f"{BOLD}{BLUE}--- [ ELIXIR NEXUS (Control Plane) ] ---{RESET}")
        print(f"  {BOLD}DISCOVERED:{RESET} {total_discovered:>8} total files found by Watcher")
        print(f"  {BOLD}INDEXED:{RESET}    {indexed:>8} verified in DB")
        print(f"  {BOLD}FAILED:{RESET}     {RED}{failed:>8}{RESET} files panicked/errored")
        print("")
        
        # Calculate visual metrics
        q_total = pending + processing + done
        if q_total == 0: q_total = 1
        
        print(f"{BOLD}RUST ENGINE LOAD:{RESET}")
        print(f"[{YELLOW}Pending{RESET}] {render_bar(pending, q_total, color=YELLOW)} {pending/q_total*100:.1f}%")
        print(f"[{BLUE}Process{RESET}] {render_bar(processing, q_total, color=BLUE)} {processing/q_total*100:.1f}%")
        print(f"[{GREEN}Done   {RESET}] {render_bar(done, q_total, color=GREEN)} {done/q_total*100:.1f}%")
        print("")
        
        if total_discovered > 0:
            prog = indexed / total_discovered * 100
        else:
            prog = 0
            
        print(f"{BOLD}GLOBAL INGESTION PROGRESS:{RESET} {prog:.2f}%")
        print(f"{render_bar(indexed, max(1, total_discovered), width=68, color=GREEN)}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print("Press Ctrl+C to exit.")
        
        time.sleep(1)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nExiting Command Center.")
        sys.exit(0)
