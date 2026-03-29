#!/usr/bin/env python3
import time
import os
import sys
import datetime
import json
import urllib.request

# Axon v8.1.0 - Nuclear Radar (SQL Gateway Edition)
# Queries Truth directly from the Rust SQL Gateway via HTTP.
# No more file locking or socket buffering issues.

GATEWAY_URL = "http://localhost:44129/sql"

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

def send_sql_query(sql):
    try:
        data = json.dumps({"query": sql}).encode('utf-8')
        req = urllib.request.Request(GATEWAY_URL, data=data, headers={'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=2.0) as response:
            return json.loads(response.read().decode('utf-8'))
    except Exception as e:
        # print(f"Debug Error: {e}")
        return None

def render_bar(value, maximum, width=40, color=GREEN):
    if maximum <= 0: return "[" + " " * width + "]"
    filled = int((value / maximum) * width)
    filled = max(0, min(filled, width))
    empty = width - filled
    return f"[{color}{'█' * filled}{RESET}{' ' * empty}]"

def main():
    print("🛰️ Initializing Deep Space Radar (HTTP Gateway Mode)...")
    while True:
        # Fetching IST & SOLL in parallel pulses
        ist_stats = send_sql_query("SELECT status, count(*) FROM File GROUP BY status")
        symbol_count_raw = send_sql_query("SELECT count(*) FROM Symbol")
        soll_count_raw = send_sql_query("SELECT count(*) FROM soll.Requirement")
        
        clear_screen()
        now = datetime.datetime.now().strftime("%H:%M:%S")
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}{CYAN}          ☢️  AXON NUCLEAR RADAR - V8.1 (GATEWAY) ☢️          {RESET}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}SYS_TIME:{RESET} {now}    {BOLD}LATTICE:{RESET} {MAGENTA}ACTIVE{RESET}    {BOLD}BRIDGE:{RESET} {GREEN if ist_stats is not None else RED}{'LINKED' if ist_stats is not None else 'LOST'}{RESET}")
        print("")
        
        if ist_stats is None:
            print(f"  {RED}>> CRITICAL: SQL GATEWAY CONNECTION LOST <<{RESET}")
            print(f"  Ensure Axon Core is running on port 44129.")
        else:
            # IST Analytics
            stats_dict = {row[0]: int(row[1]) for row in ist_stats if len(row) >= 2}
            total = sum(stats_dict.values())
            indexed = stats_dict.get('indexed', 0)
            pending = stats_dict.get('pending', 0)
            skipped = stats_dict.get('skipped', 0)
            
            sym_count = int(symbol_count_raw[0][0]) if symbol_count_raw and len(symbol_count_raw[0]) > 0 else 0
            req_count = int(soll_count_raw[0][0]) if soll_count_raw and len(soll_count_raw[0]) > 0 else 0
            
            print(f"{BOLD}{BLUE}--- [ IST PLANE : PHYSICAL FORGE ] ---{RESET}")
            print(f"  {BOLD}FILES DISCOVERED:{RESET} {total:>8}")
            print(f"  {BOLD}STATUS PENDING:  {RESET} {YELLOW}{pending:>8}{RESET}")
            print(f"  {BOLD}STATUS SKIPPED:  {RESET} {WHITE}{skipped:>8}{RESET}")
            print(f"  {BOLD}TRUTH INDEXED:   {RESET} {GREEN}{indexed:>8}{RESET} ({sym_count} symbols extracted)")
            print("")
            
            # Global Progress (Indexed + Skipped are work done)
            done = indexed + skipped
            prog = (done / total * 100) if total > 0 else 0
            print(f"{BOLD}GLOBAL TRUTH ALIGNMENT (Done/Total):{RESET} {prog:.2f}%")
            print(f"{render_bar(done, total, width=68, color=GREEN)}")
            
            print("")
            print(f"{BOLD}{WHITE}--- [ SOLL PLANE : INTENTIONAL SANCTUARY ] ---{RESET}")
            print(f"  {BOLD}REQUIREMENTS:{RESET} {req_count:>6} active objectives")
            print(f"  {BOLD}COMPLIANCE:  {RESET} Digital Thread Synchronized")

        print("")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(" [RADAR ACTIVE] SQL Gateway: http://localhost:44129/sql | Ctrl+C to exit")
        
        time.sleep(2)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nRadar shut down.")
        sys.exit(0)
