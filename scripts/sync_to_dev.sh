#!/bin/bash
set -euo pipefail

PROD_DIR="/home/dstadel/projects/axon"
DEV_WORKTREE="$PROD_DIR/.worktrees/dev/feat-workflow-hardening"

echo "🔄 Syncing Production Database to Dev Environment..."

if [ ! -d "$DEV_WORKTREE" ]; then
    echo "❌ Dev worktree not found at $DEV_WORKTREE"
    exit 1
fi

mkdir -p "$DEV_WORKTREE/.axon/graph_v2"

# Copy the DuckDB Database files
if [ -f "$PROD_DIR/.axon/graph_v2/ist.db" ]; then
    echo "  -> Copying ist.db..."
    # Since DuckDB locks the file if running, we can do a standard copy,
    # but the safest is to ensure we copy the WAL too.
    cp "$PROD_DIR/.axon/graph_v2/ist.db" "$DEV_WORKTREE/.axon/graph_v2/"
    if [ -f "$PROD_DIR/.axon/graph_v2/ist.db.wal" ]; then
        cp "$PROD_DIR/.axon/graph_v2/ist.db.wal" "$DEV_WORKTREE/.axon/graph_v2/"
    fi
else
    echo "⚠️ Prod ist.db not found!"
fi

# Copy capabilities and meta.json if they exist
cp -n "$PROD_DIR/.axon/meta.json" "$DEV_WORKTREE/.axon/" 2>/dev/null || true
cp -n "$PROD_DIR/.axon/capabilities.toml" "$DEV_WORKTREE/.axon/" 2>/dev/null || true

echo "✅ Sync complete. You can now start the dev server."
