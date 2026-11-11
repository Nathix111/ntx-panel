#!/usr/bin/env bash
# ==============================================================================
# NTX Server Release Packaging Script
# Builds release binaries, strips symbols, and creates release tarballs:
#   - dist/ntx-server-linux-x86_64.tar.gz
#   - dist/ntx-server-linux-aarch64.tar.gz
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"
DIST_DIR="$ROOT_DIR/dist"

mkdir -p "$DIST_DIR"

# Targets to build (can pass custom target as argument)
TARGETS=("${@:-x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu}")

echo "=================================================================="
echo "               NTX SERVER RELEASE PACKAGING                       "
echo "=================================================================="
echo "Root directory: $ROOT_DIR"
echo "Output dist   : $DIST_DIR"
echo "Targets       : ${TARGETS[*]}"
echo "------------------------------------------------------------------"

cd "$ROOT_DIR"

for target in "${TARGETS[@]}"; do
    case "$target" in
        x86_64*linux*)
            ARCH="x86_64"
            STRIP_CMD="strip"
            ;;
        aarch64*linux*)
            ARCH="aarch64"
            if command -v aarch64-linux-gnu-strip >/dev/null 2>&1; then
                STRIP_CMD="aarch64-linux-gnu-strip"
            else
                STRIP_CMD="strip"
            fi
            ;;
        *)
            ARCH="$target"
            STRIP_CMD="strip"
            ;;
    esac

    echo "==> Building ntx-server for $target ($ARCH)..."

    # Use cross if target does not match host architecture or if cross is available
    BUILD_TOOL="cargo"
    if [ "$ARCH" = "aarch64" ] && [ "$(uname -m)" != "aarch64" ]; then
        if command -v cross >/dev/null 2>&1; then
            BUILD_TOOL="cross"
        else
            echo "Notice: 'cross' not found. Trying cargo with target $target..."
        fi
    fi

    echo "    Using $BUILD_TOOL build --release --target $target -p ntx-server"
    $BUILD_TOOL build --release --target "$target" -p ntx-server

    BIN_PATH="$ROOT_DIR/target/$target/release/ntx-server"
    if [ ! -f "$BIN_PATH" ]; then
        echo "Error: Binary not found at $BIN_PATH" >&2
        exit 1
    fi

    # Create staging folder for tarball
    STAGE_DIR="$DIST_DIR/stage-$ARCH"
    rm -rf "$STAGE_DIR"
    mkdir -p "$STAGE_DIR"

    cp "$BIN_PATH" "$STAGE_DIR/ntx-server"
    cp "$SCRIPT_DIR/install.sh" "$STAGE_DIR/install.sh"
    cp "$SCRIPT_DIR/uninstall.sh" "$STAGE_DIR/uninstall.sh"
    chmod +x "$STAGE_DIR"/*

    # Strip binary if strip command is available and works on this architecture
    if command -v "$STRIP_CMD" >/dev/null 2>&1; then
        echo "    Stripping symbols with $STRIP_CMD..."
        "$STRIP_CMD" "$STAGE_DIR/ntx-server" 2>/dev/null || true
    fi

    ORIG_SIZE=$(ls -lh "$BIN_PATH" | awk '{print $5}')
    STRIPPED_SIZE=$(ls -lh "$STAGE_DIR/ntx-server" | awk '{print $5}')
    echo "    Binary size: original=$ORIG_SIZE, stripped=$STRIPPED_SIZE"

    TARBALL="$DIST_DIR/ntx-server-linux-${ARCH}.tar.gz"
    echo "    Creating tarball: $TARBALL"
    tar -czf "$TARBALL" -C "$STAGE_DIR" ntx-server install.sh uninstall.sh

    TARBALL_SIZE=$(ls -lh "$TARBALL" | awk '{print $5}')
    echo "    Tarball size: $TARBALL_SIZE"

    rm -rf "$STAGE_DIR"
done

echo ""
echo "==> Packaging complete! Generated artifacts in dist/:"
ls -lh "$DIST_DIR"/*.tar.gz || true

if command -v sha256sum >/dev/null 2>&1; then
    echo ""
    echo "==> SHA-256 Checksums:"
    (cd "$DIST_DIR" && sha256sum *.tar.gz)
fi
echo "=================================================================="
