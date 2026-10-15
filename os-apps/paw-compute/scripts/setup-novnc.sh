#!/usr/bin/env bash
# Browser access for a Computer's desktop (TensorLake ubuntu-vnc image).
#
# Installs a noVNC/websockify bridge as a systemd service so a human can
# reach the same desktop the agents work on at:
#
#   https://6080-<machine_id>.sandbox.tensorlake.ai/vnc.html
#
# after exposing the port once with `tl sbx port expose <name> 6080`.
# TensorLake port exposure is unauthenticated by design; the desktop stays
# gated by TigerVNC's VncAuth password (the image runs the server with
# -SecurityTypes VncAuth,TLSVnc), so the exposed page alone grants nothing.
# Set/rotate the password out-of-band with vncpasswd — never through an
# audited Exec command line.
#
# Idempotent. Run on the computer (e.g. through a governed Exec).
set -euo pipefail

VNC_PORT="${VNC_PORT:-5901}"
WEB_PORT="${WEB_PORT:-6080}"

if ! command -v websockify >/dev/null 2>&1 || [ ! -f /usr/share/novnc/vnc.html ]; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq novnc websockify
fi

sudo tee /etc/systemd/system/novnc.service >/dev/null <<EOF
[Unit]
Description=noVNC websockify bridge
After=network.target

[Service]
ExecStart=/usr/bin/websockify --web=/usr/share/novnc ${WEB_PORT} localhost:${VNC_PORT}
Restart=always
User=$(id -un)

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now novnc

sleep 2
curl -sf -o /dev/null "http://127.0.0.1:${WEB_PORT}/vnc.html"
echo "novnc bridge active on :${WEB_PORT} (vnc :${VNC_PORT})"
