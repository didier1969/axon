#!/bin/bash
# Axon Service Installer - Industrial v1.2
# Installe Axon comme un service persistant avec logs tournants (48h)

PROJECT_ROOT="/home/dstadel/projects/axon"
LOG_DIR="$PROJECT_ROOT/logs"
RETENTION_HOURS=48

echo "🚀 Installing Axon System Service..."

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

# 3. Création du service Systemd (pour Linux/WSL)
cat <<EOF | sudo tee /etc/systemd/system/axon.service
[Unit]
Description=Axon Code Intelligence Daemon
After=network.target

[Service]
Type=simple
User=dstadel
WorkingDirectory=$PROJECT_ROOT
Environment=PYTHONPATH=$PROJECT_ROOT/src
ExecStart=/usr/bin/nix develop $PROJECT_ROOT --no-write-lock-file -c python3 -u $PROJECT_ROOT/scripts/axon-fleet-daemon.py
ExecReload=/bin/kill -HUP \$MAINPID
Restart=always
RestartSec=10
StandardOutput=append:$LOG_DIR/axon.log
StandardError=append:$LOG_DIR/axon.log

[Install]
WantedBy=multi-user.target
EOF

# 4. Activation
sudo systemctl daemon-reload
echo "✅ Axon Service configured."
echo "💡 Use 'sudo systemctl start axon' to begin indexing."
echo "📊 Logs available in $LOG_DIR (Retention: 48h)"
