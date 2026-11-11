#!/usr/bin/env bash
# ==============================================================================
# NTX Panel Server Agent — Automated Clean Uninstall & Purge Script
# Completely removes ntx-server binary, systemd service, and all configuration.
# ==============================================================================

set -euo pipefail

# 1. Root / Sudo check
if [ "${EUID:-$(id -u)}" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    else
        echo "Error: Root privileges required. Please run with sudo or as root." >&2
        exit 1
    fi
else
    SUDO=""
fi

echo "=================================================================="
echo "                 NTX PANEL AGENT — UNINSTALL                      "
echo "=================================================================="

# 2. Invoke binary uninstall if available
if [ -x "/usr/local/bin/ntx-server" ]; then
    echo "==> Invoking ntx-server uninstall --purge..."
    $SUDO /usr/local/bin/ntx-server uninstall --purge 2>/dev/null || true
fi

# 3. Ensure systemd service is completely stopped and disabled
if command -v systemctl >/dev/null 2>&1; then
    echo "==> Ensuring systemd service is stopped and disabled..."
    $SUDO systemctl stop ntx-server.service 2>/dev/null || true
    $SUDO systemctl disable ntx-server.service 2>/dev/null || true
fi

# 4. Remove service unit file
UNIT_FILE="/etc/systemd/system/ntx-server.service"
if [ -f "$UNIT_FILE" ]; then
    echo "==> Removing systemd service unit ($UNIT_FILE)..."
    $SUDO rm -f "$UNIT_FILE"
    if command -v systemctl >/dev/null 2>&1; then
        $SUDO systemctl daemon-reload 2>/dev/null || true
    fi
fi

# 5. Remove binary
BIN_PATH="/usr/local/bin/ntx-server"
if [ -f "$BIN_PATH" ]; then
    echo "==> Removing binary ($BIN_PATH)..."
    $SUDO rm -f "$BIN_PATH"
fi

# 6. Wipe configuration directories
echo "==> Purging configuration directories..."
CONFIG_DIRS=(
    "/etc/ntx-server"
    "/root/.config/ntx-server"
)

if [ -n "${SUDO_USER:-}" ]; then
    CONFIG_DIRS+=("/home/${SUDO_USER}/.config/ntx-server")
fi

if [ -n "${HOME:-}" ] && [ "${HOME:-}" != "/root" ]; then
    CONFIG_DIRS+=("${HOME}/.config/ntx-server")
fi

for dir in "${CONFIG_DIRS[@]}"; do
    if [ -d "$dir" ]; then
        echo "    Deleting $dir"
        $SUDO rm -rf "$dir"
    fi
done

echo ""
echo "=================================================================="
echo "     NTX Panel Agent has been completely removed and purged.      "
echo "=================================================================="
