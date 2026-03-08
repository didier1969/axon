#!/usr/bin/env python3
import json
import subprocess
import os
from pathlib import Path
import time

def start_fleet():
    registry_root = Path.home() / ".axon" / "repos"
    watcher_dir = Path(__file__).parent.parent / "src" / "watcher"
    
    if not registry_root.exists():
        print("No registered repositories found.")
        return

    print(f"🚀 Axon Fleet: Starting v1.1 indexers for all projects...")
    
    for slug_dir in registry_root.iterdir():
        if not slug_dir.is_dir(): continue
        meta_path = slug_dir / "meta.json"
        if not meta_path.exists(): continue
        
        try:
            meta = json.loads(meta_path.read_text())
            repo_path = meta.get("path")
            repo_name = meta.get("name")
            
            if repo_path and os.path.exists(repo_path):
                print(f"  -> Launching indexer for {repo_name}...")
                # On lance mix run dans un sous-processus détaché
                # On passe le répertoire du projet via variable d'env
                env = os.environ.copy()
                env["AXON_WATCH_DIR"] = repo_path
                
                # Utilisation de Popen pour que ça tourne en tâche de fond
                subprocess.Popen(
                    ["mix", "run", "--no-halt"],
                    cwd=str(watcher_dir),
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL
                )
        except Exception as e:
            print(f"  !! Failed to launch {slug_dir.name}: {e}")

    print("✅ All indexers launched in background.")
    print("Check progress in Elixir logs or via future 'axon status' command.")

if __name__ == "__main__":
    start_fleet()
