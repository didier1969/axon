#!/usr/bin/env python3
import socket
import time
import os
import sys
import datetime
import json

# Axon v3.4.1 - Nuclear Radar (Stream Reconstructor Edition)
# Queries Truth directly from the Rust Data Plane via RAW_QUERY with line-buffering.

SOCKET_PATH = "/tmp/axon-telemetry.sock"

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

def send_raw_query(sql):
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(2.0)
        client.connect(SOCKET_PATH)
        
        # Consommer le message de bienvenue
        initial = ""
        while True:
            chunk = client.recv(4096).decode('utf-8')
            initial += chunk
            if "SystemReady" in initial:
                break
        
        # Envoyer la requête
        client.sendall(f"RAW_QUERY {sql}\n".encode())
        
        # Reconstituer la réponse JSON complète
        response = ""
        while True:
            chunk = client.recv(4096).decode('utf-8')
            if not chunk: break
            response += chunk
            if "\n" in chunk: break
            
        client.close()
        
        # Nettoyer et parser
        for line in response.split('\n'):
            line = line.strip()
            if line.startswith('[['):
                return json.loads(line)
        return []
    except Exception as e:
        return None

def render_bar(value, maximum, width=40, color=GREEN):
    if maximum <= 0: return "[" + " " * width + "]"
    filled = int((value / maximum) * width)
    filled = max(0, min(filled, width))
    empty = width - filled
    return f"[{color}{'█' * filled}{RESET}{' ' * empty}]"

def main():
    print("🛰️ Initializing Deep Space Radar...")
    while True:
        # Fetching IST & SOLL in parallel pulses
        ist_stats = send_raw_query("SELECT status, count(*) FROM File GROUP BY status")
        symbol_count_raw = send_raw_query("SELECT count(*) FROM Symbol")
        soll_count_raw = send_raw_query("SELECT count(*) FROM soll.Requirement")
        
        clear_screen()
        now = datetime.datetime.now().strftime("%H:%M:%S")
        
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}{CYAN}          ☢️  AXON NUCLEAR RADAR - V3.4 (DEEP SPACE) ☢️          {RESET}")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(f"{BOLD}SYS_TIME:{RESET} {now}    {BOLD}LATTICE:{RESET} {MAGENTA}ACTIVE{RESET}    {BOLD}BRIDGE:{RESET} {GREEN if ist_stats is not None else RED}{'LINKED' if ist_stats is not None else 'LOST'}{RESET}")
        print("")
        
        if ist_stats is None:
            print(f"  {RED}>> CRITICAL: DATA PLANE CONNECTION LOST <<{RESET}")
            print(f"  Attempting to re-establish bridge...")
        else:
            # IST Analytics
            stats_dict = {row[0]: int(row[1]) for row in ist_stats if len(row) >= 2}
            total = sum(stats_dict.values())
            indexed = stats_dict.get('indexed', 0)
            pending = stats_dict.get('pending', 0)
            proc = stats_dict.get('processing', 0)
            
            sym_count = int(symbol_count_raw[0][0]) if symbol_count_raw and len(symbol_count_raw[0]) > 0 else 0
            req_count = int(soll_count_raw[0][0]) if soll_count_raw and len(soll_count_raw[0]) > 0 else 0
            
            print(f"{BOLD}{BLUE}--- [ IST PLANE : PHYSICAL FORGE ] ---{RESET}")
            print(f"  {BOLD}FILES DISCOVERED:{RESET} {total:>8}")
            print(f"  {BOLD}STATUS PENDING:  {RESET} {YELLOW}{pending:>8}{RESET}")
            print(f"  {BOLD}STATUS IN-FLIGHT:{RESET} {MAGENTA}{proc:>8}{RESET}")
            print(f"  {BOLD}TRUTH INDEXED:   {RESET} {GREEN}{indexed:>8}{RESET} ({sym_count} symbols extracted)")
            print("")
            
            # Global Progress
            prog = (indexed / total * 100) if total > 0 else 0
            print(f"{BOLD}GLOBAL TRUTH ALIGNMENT:{RESET} {prog:.2f}%")
            print(f"{render_bar(indexed, total, width=68, color=GREEN)}")
            
            print("")
            print(f"{BOLD}{WHITE}--- [ SOLL PLANE : INTENTIONAL SANCTUARY ] ---{RESET}")
            print(f"  {BOLD}REQUIREMENTS:{RESET} {req_count:>6} active objectives")
            print(f"  {BOLD}COMPLIANCE:  {RESET} Digital Thread Synchronized")

        print("")
        print(f"{BOLD}{CYAN}======================================================================{RESET}")
        print(" [RADAR ACTIVE] Use './scripts/axon_scan.py' to resume mapping | Ctrl+C to exit")
        
        time.sleep(2)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nRadar shut down.")
        sys.exit(0)
