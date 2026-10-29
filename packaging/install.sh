#!/bin/sh
# Install Figma Agent for Linux for the current user (no root needed).
# Copies the bundle to ~/.local/share/figma-agent-linux and starts it at login:
# a systemd user service when systemd is available, otherwise an XDG autostart entry.
set -eu
SRC="$(cd "$(dirname "$0")" && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/figma-agent-linux"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"

if [ "$SRC" != "$DEST" ]; then
    "$SRC/uninstall.sh" --quiet 2>/dev/null || true
    mkdir -p "$DEST"
    cp -a "$SRC/." "$DEST/"
fi
mkdir -p "$HOME/.local/bin"
ln -sf "$DEST/figma-agent" "$HOME/.local/bin/figma-agent"

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    mkdir -p "$CONFIG/systemd/user"
    cat > "$CONFIG/systemd/user/figma-agent.service" <<UNIT
[Unit]
Description=Figma Agent for Linux (local fonts for figma.com)

[Service]
ExecStart=$DEST/figma-agent
Restart=on-failure
# Extra font folders (colon separated):
#Environment=FIGMA_AGENT_FONT_DIRS=%h/MyFonts

[Install]
WantedBy=default.target
UNIT
    systemctl --user daemon-reload
    systemctl --user enable figma-agent.service >/dev/null
    systemctl --user restart figma-agent.service
    echo "Installed as systemd user service (systemctl --user status figma-agent)."
else
    mkdir -p "$CONFIG/autostart"
    cat > "$CONFIG/autostart/figma-agent.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Figma Agent
Comment=Local fonts for figma.com
Exec=$DEST/figma-agent
NoDisplay=true
X-GNOME-Autostart-enabled=true
DESKTOP
    pkill -x -f "$DEST/figma-agent" 2>/dev/null || true
    nohup "$DEST/figma-agent" >"$DEST/agent.log" 2>&1 &
    echo "No systemd user session: installed XDG autostart entry and started the agent (log: $DEST/agent.log)."
fi

i=0
while [ $i -lt 30 ]; do
    if out="$(curl -fsS -m 2 http://127.0.0.1:44950/figma/version 2>/dev/null || wget -qO- -T 2 http://127.0.0.1:44950/figma/version 2>/dev/null)"; then
        echo "Figma Agent is running: $out"
        echo "In Figma: Preferences > Permissions and helpers > Connect, then allow the browser prompt."
        exit 0
    fi
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
        echo "Figma Agent started (install curl or wget to have this script verify it)."
        exit 0
    fi
    i=$((i + 1)); sleep 1
done
echo "Agent did not answer on 127.0.0.1:44950 yet; check the service log." >&2
exit 1
