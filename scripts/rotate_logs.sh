#!/bin/bash
# Rotation des logs Axon - Rétention 48h
find "/home/dstadel/projects/axon/logs" -name "*.log.*" -mmin +2880 -delete
for f in "/home/dstadel/projects/axon/logs"/*.log "/home/dstadel/projects/axon/logs/watchers"/*.log; do
    if [ -f "$f" ] && [ $(stat -c%s "$f") -gt 10485760 ]; then # 10Mo max par segment
        mv "$f" "$f.$(date +%Y%m%d%H%M)"
        touch "$f"
    fi
done
