#!/bin/bash
# Install Axon as a systemd user service (works in modern WSL2 with systemd enabled)
SERVICE_FILE="$HOME/.config/systemd/user/axon.service"
mkdir -p "$HOME/.config/systemd/user"

cat << INNER_EOF > "$SERVICE_FILE"
[Unit]
Description=Axon Oracle (Lattice Engine)
After=network.target

[Service]
Type=simple
WorkingDirectory=/home/dstadel/projects/axon
ExecStart=/home/dstadel/projects/axon/scripts/start-v2.sh
ExecStop=/home/dstadel/projects/axon/scripts/stop-v2.sh
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
INNER_EOF

systemctl --user daemon-reload
systemctl --user enable axon.service
echo "✅ Axon service installed and enabled for automatic startup."
