#!/usr/bin/env bash
# ==============================================================================
# NTX Panel Server Agent — Automated Installer
# Supported OS: Linux (Debian/Ubuntu, Arch Linux, Fedora, Alpine, etc.)
# Supported Arch: x86_64, aarch64 (ARM64 / Raspberry Pi 5)
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
echo "                 NTX PANEL AGENT INSTALLER                        "
echo "=================================================================="

# 2. Architecture auto-detection
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
    x86_64|amd64)
        ARCH="x86_64"
        ;;
    aarch64|arm64)
        ARCH="aarch64"
        ;;
    *)
        echo "Error: Unsupported architecture '$ARCH_RAW'." >&2
        echo "NTX Server supports x86_64 and aarch64 (64-bit ARM, e.g. Raspberry Pi 5)." >&2
        exit 1
        ;;
esac
echo "==> Architecture: $ARCH ($ARCH_RAW)"

# 3. Detect and ensure required utilities
install_deps_if_needed() {
    local missing=()
    if ! command -v tar >/dev/null 2>&1; then missing+=("tar"); fi
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then missing+=("curl"); fi

    if [ ${#missing[@]} -gt 0 ]; then
        echo "==> Installing missing dependencies: ${missing[*]}..."
        if command -v apt-get >/dev/null 2>&1; then
            $SUDO apt-get update -qq && $SUDO apt-get install -y -qq "${missing[@]}" ca-certificates
        elif command -v pacman >/dev/null 2>&1; then
            $SUDO pacman -Sy --noconfirm "${missing[@]}" ca-certificates
        elif command -v dnf >/dev/null 2>&1; then
            $SUDO dnf install -y "${missing[@]}" ca-certificates
        elif command -v apk >/dev/null 2>&1; then
            $SUDO apk add --no-cache "${missing[@]}" ca-certificates
        fi
    fi
}
install_deps_if_needed

TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# 4. Acquire binary
BINARY_SRC=""
TARBALL_NAME="ntx-server-linux-${ARCH}.tar.gz"

# Check if a custom binary path was provided
if [ -n "${NTX_BINARY_PATH:-}" ] && [ -f "$NTX_BINARY_PATH" ]; then
    echo "==> Using specified binary: $NTX_BINARY_PATH"
    cp "$NTX_BINARY_PATH" "$TMP_DIR/ntx-server"
    BINARY_SRC="$TMP_DIR/ntx-server"
fi

# Check for local pre-compiled binary or tarball (e.g. running from repo or release bundle)
if [ -z "$BINARY_SRC" ]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd || pwd)"
    SEARCH_PATHS=(
        "$SCRIPT_DIR/../target/release/ntx-server"
        "$SCRIPT_DIR/target/release/ntx-server"
        "$SCRIPT_DIR/ntx-server"
        "./target/release/ntx-server"
        "./ntx-server"
    )
    for p in "${SEARCH_PATHS[@]}"; do
        if [ -f "$p" ]; then
            echo "==> Found local binary at $p"
            cp "$p" "$TMP_DIR/ntx-server"
            BINARY_SRC="$TMP_DIR/ntx-server"
            break
        fi
    done
fi

if [ -z "$BINARY_SRC" ]; then
    TARBALL_SEARCH_PATHS=(
        "$SCRIPT_DIR/$TARBALL_NAME"
        "$SCRIPT_DIR/../dist/$TARBALL_NAME"
        "./dist/$TARBALL_NAME"
        "./$TARBALL_NAME"
    )
    for t in "${TARBALL_SEARCH_PATHS[@]}"; do
        if [ -f "$t" ]; then
            echo "==> Extracting local release tarball: $t"
            tar -xzf "$t" -C "$TMP_DIR"
            BINARY_SRC="$TMP_DIR/ntx-server"
            break
        fi
    done
fi

# If still not found, download from GitHub Releases
if [ -z "$BINARY_SRC" ]; then
    NTX_REPO="${NTX_REPO:-Nath-11/ntx-panel}"
    NTX_VERSION="${NTX_VERSION:-latest}"

    if [ "$NTX_VERSION" = "latest" ]; then
        DOWNLOAD_URL="https://github.com/${NTX_REPO}/releases/latest/download/${TARBALL_NAME}"
    else
        DOWNLOAD_URL="https://github.com/${NTX_REPO}/releases/download/${NTX_VERSION}/${TARBALL_NAME}"
    fi

    echo "==> Downloading ${TARBALL_NAME} from GitHub Releases..."
    echo "    URL: $DOWNLOAD_URL"
    if command -v curl >/dev/null 2>&1; then
        curl -fSL "$DOWNLOAD_URL" -o "$TMP_DIR/$TARBALL_NAME"
    else
        wget -qO "$TMP_DIR/$TARBALL_NAME" "$DOWNLOAD_URL"
    fi

    tar -xzf "$TMP_DIR/$TARBALL_NAME" -C "$TMP_DIR"
    BINARY_SRC="$TMP_DIR/ntx-server"
fi

if [ ! -f "$BINARY_SRC" ]; then
    echo "Error: Failed to obtain ntx-server binary for $ARCH." >&2
    exit 1
fi

chmod +x "$BINARY_SRC"

# 5. Install binary to /usr/local/bin
echo "==> Installing executable to /usr/local/bin/ntx-server..."
$SUDO install -d /usr/local/bin
$SUDO install -m 755 "$BINARY_SRC" /usr/local/bin/ntx-server

# Install uninstall helper to /usr/local/bin/ntx-uninstall if available
if [ -f "$TMP_DIR/uninstall.sh" ]; then
    $SUDO install -m 755 "$TMP_DIR/uninstall.sh" /usr/local/bin/ntx-uninstall
elif [ -f "${SCRIPT_DIR:-}/uninstall.sh" ]; then
    $SUDO install -m 755 "$SCRIPT_DIR/uninstall.sh" /usr/local/bin/ntx-uninstall
fi

# 6. Register & start systemd service
if command -v systemctl >/dev/null 2>&1; then
    echo "==> Registering systemd service..."
    $SUDO /usr/local/bin/ntx-server install
else
    echo "Notice: systemctl not found; skipping automatic service registration."
    echo "You can launch ntx-server directly by running: /usr/local/bin/ntx-server run"
fi

# 7. Retrieve and display Pre-Shared Key (PSK)
echo "==> Retrieving configuration and credentials..."
CONFIG_OUTPUT="$($SUDO /usr/local/bin/ntx-server config 2>/dev/null || true)"
PSK=$(echo "$CONFIG_OUTPUT" | grep -E '^[[:space:]]*psk[[:space:]]*=' | head -n1 | sed -E 's/.*=[[:space:]]*"([^"]+)".*/\1/' || true)
HOSTNAME_STR="$(hostname 2>/dev/null || echo "ntx-device")"

echo ""
echo "=================================================================="
echo "           NTX PANEL AGENT INSTALLED SUCCESSFULLY                 "
echo "=================================================================="
echo "  Node Hostname   : ${HOSTNAME_STR}"
echo "  Architecture    : ${ARCH}"
echo "  Binary Location : /usr/local/bin/ntx-server"
if command -v systemctl >/dev/null 2>&1; then
echo "  Systemd Service : ntx-server.service (active & enabled)"
fi
echo "  Pre-Shared Key  : ${PSK:-<Run 'sudo ntx-server config' to view>}"
echo "------------------------------------------------------------------"
echo "  CONNECTING FROM NTX VIEWER:"
echo "  1. Launch ntx-viewer on your management machine."
echo "  2. The agent will be discovered automatically via mDNS, or add"
echo "     it using this device's IP and port 9850."
echo "  3. Enter the Pre-Shared Key (PSK) shown above to authenticate."
echo "------------------------------------------------------------------"
echo "  COMMANDS:"
echo "  - Check status  : sudo ntx-server status"
echo "  - View config   : sudo ntx-server config"
echo "  - Clean purge   : sudo /usr/local/bin/ntx-server uninstall --purge"
echo "=================================================================="
