#!/bin/bash
# Axon Service Installer - Industrial v2
# Installe Axon comme un service persistant avec logs tournants (48h)

PROJECT_ROOT="/home/dstadel/projects/axon"
LOG_DIR="$PROJECT_ROOT/logs"
RETENTION_HOURS=48

echo "🚀 Installing Axon System Service (Rust Core)..."

# 1. Création des répertoires de logs
mkdir -p "$LOG_DIR/watchers"
mkdir -p "$LOG_DIR/archive"

# 2. Script de rotation des logs (sera appelé par cron ou le daemon)
cat <<EOF > "$PROJECT_ROOT/scripts/rotate_logs.sh"
#!/bin/bash
# Rotation des logs Axon - Rétention 48h
find "$LOG_DIR" -name "*.log.*" -mmin +$((RETENTION_HOURS * 60)) -delete
for f in "$LOG_DIR"/*.log "$LOG_DIR/watchers"/*.log; do
    if [ -f "\$f" ] && [ \$(stat -c%s "\$f") -gt 10485760 ]; then # 10Mo max par segment
        mv "\$f" "\$f.\$(date +%Y%m%d%H%M)"
        touch "\$f"
    fi
done
EOF
chmod +x "$PROJECT_ROOT/scripts/rotate_logs.sh"

# 3. Création du service Systemd pour Axon Core (Data Plane)
cat <<EOF | sudo tee /etc/systemd/system/axon-core.service
[Unit]
Description=Axon Code Intelligence Daemon (Rust Core)
After=network.target

[Service]
Type=simple
User=dstadel
WorkingDirectory=$PROJECT_ROOT
ExecStart=$PROJECT_ROOT/bin/axon-core
ExecReload=/bin/kill -HUP \$MAINPID
Restart=always
RestartSec=3
StandardOutput=append:$LOG_DIR/axon-core.log
StandardError=append:$LOG_DIR/axon-core.log

[Install]
WantedBy=multi-user.target
EOF

# 4. Création du service Systemd pour le Dashboard (Control Plane)
cat <<EOF | sudo tee /etc/systemd/system/axon-dashboard.service
[Unit]
Description=Axon Dashboard (Elixir/Phoenix)
After=network.target axon-core.service

[Service]
Type=simple
User=dstadel
WorkingDirectory=$PROJECT_ROOT/src/dashboard
Environment=MIX_ENV=prod
Environment=PORT=44921
ExecStart=/usr/bin/env mix phx.server
Restart=always
RestartSec=5
StandardOutput=append:$LOG_DIR/axon-dashboard.log
StandardError=append:$LOG_DIR/axon-dashboard.log

[Install]
WantedBy=multi-user.target
EOF

# 5. Activation
sudo systemctl daemon-reload
echo "✅ Axon Services configured."
echo "💡 Use 'sudo systemctl start axon-core axon-dashboard' to begin indexing."
echo "📊 Logs available in $LOG_DIR (Retention: 48h)"
