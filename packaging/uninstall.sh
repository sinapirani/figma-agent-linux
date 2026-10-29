#!/bin/sh
# Remove Figma Agent for Linux for the current user.
set -u
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/figma-agent-linux"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
if command -v systemctl >/dev/null 2>&1 && [ -f "$CONFIG/systemd/user/figma-agent.service" ]; then
    systemctl --user disable --now figma-agent.service >/dev/null 2>&1
    rm -f "$CONFIG/systemd/user/figma-agent.service"
    systemctl --user daemon-reload
fi
rm -f "$CONFIG/autostart/figma-agent.desktop"
pkill -x -f "$DEST/figma-agent" 2>/dev/null
pkill -f "$DEST/app/figma_agent.py" 2>/dev/null  # older Python-based releases
[ "$(readlink "$HOME/.local/bin/figma-agent" 2>/dev/null)" = "$DEST/figma-agent" ] && rm -f "$HOME/.local/bin/figma-agent"
rm -rf "$DEST"
[ "${1:-}" = "--quiet" ] || echo "Figma Agent for Linux removed."
