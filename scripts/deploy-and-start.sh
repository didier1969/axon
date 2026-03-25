#!/bin/bash
set -e

# Nexus Seal - Deployment & Boot Script
PROJECT_ROOT="/home/dstadel/projects/axon"
cd "$PROJECT_ROOT"

echo "🛡️  [Nexus Seal] Démarrage de la procédure de mise en service industrielle..."

# 1. Kill everything first to avoid "Text file busy"
echo "🧹 Nettoyage des processus existants..."
killall axon-core axon-mcp-tunnel 2>/dev/null || true
fuser -k 44127/tcp 44129/tcp 2>/dev/null || true
rm -f /tmp/axon-*.sock
rm -f .axon/graph_v2/lbug.db.wal

# 2. Build in Release mode (The engine of the beast)
echo "⚙️  Compilation du moteur Axon Core (Mode Production)..."
cd src/axon-core && cargo build --release
cd "$PROJECT_ROOT"

echo "⚙️  Compilation du Tunnel MCP HTTP/SSE..."
cd src/axon-mcp-tunnel && cargo build --release
cd "$PROJECT_ROOT"

# 3. Deploy binaires
echo "🚀 Déploiement des binaires de production..."
mkdir -p bin
cp -f src/axon-core/target/release/axon-core bin/
cp -f src/axon-mcp-tunnel/target/release/axon-mcp-tunnel bin/
chmod +x bin/axon-core bin/axon-mcp-tunnel

# 4. Boot via standard start script
echo "📡 Lancement de l'infrastructure (TMUX)..."
./scripts/start-v2.sh

# 5. Final 360 Certification
echo "🔍 Certification finale du canal IA (Audit 360°)..."
if ./scripts/mcp_verify_360.py; then
    echo ""
    echo "=========================================================="
    echo "✅ SYSTÈME CERTIFIÉ 'NEXUS SEAL' ET PRÊT POUR LE CLIENT"
    echo "=========================================================="
    echo "Procédure finale pour l'IA :"
    echo "Dans Gemini CLI, tapez : /mcp reload"
    echo "=========================================================="
else
    echo "❌ ÉCHEC DE LA CERTIFICATION. Consultez 'tmux attach -t axon'."
    exit 1
fi
