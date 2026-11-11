# NTX Panel

A lightweight, secure, cross-platform remote management and monitoring panel with zero-configuration mDNS discovery, gRPC streaming metrics, remote PTY terminal, and pre-shared key (PSK) authentication.

## NTX Server (Agent Node)

`ntx-server` is a standalone daemon designed to run on remote Linux and Windows nodes (including Raspberry Pi 5 / ARM64 SBCs).

### 1-Command Fast Install (Debian, Ubuntu, Arch Linux, ARM64 / Pi 5)

Run as any user (will elevate with `sudo` if needed):

```bash
curl -sSL https://raw.githubusercontent.com/Nath-11/ntx-panel/master/scripts/install.sh | bash
```

The script automatically:
1. Detects system architecture (`x86_64` or `aarch64` / ARM64).
2. Downloads and installs the stripped `ntx-server` binary (~4MB) to `/usr/local/bin/ntx-server`.
3. Registers, enables, and starts the systemd unit (`ntx-server.service`).
4. Generates a secure Pre-Shared Key (PSK) and displays it on screen for easy pairing in `ntx-viewer`.

### 1-Command Clean Purge / Uninstall

To stop the service, remove all binaries, and wipe all configuration directories (`/etc/ntx-server`, `~/.config/ntx-server`, `/root/.config/ntx-server`):

```bash
curl -sSL https://raw.githubusercontent.com/Nath-11/ntx-panel/master/scripts/uninstall.sh | bash
```

Or using the installed binary directly:

```bash
sudo ntx-server uninstall --purge
```

### Server Management Commands

```bash
# Check service status
sudo ntx-server status
# or
systemctl status ntx-server

# View current configuration and PSK
sudo ntx-server config

# Standard uninstall (keeps configs)
sudo ntx-server uninstall

# Clean purge uninstall (removes configs and binary)
sudo ntx-server uninstall --purge
```

## Releases & Pre-built Binaries

Pre-compiled release tarballs are built automatically via GitHub Actions:
- `ntx-server-linux-x86_64.tar.gz` (for standard 64-bit x86 servers)
- `ntx-server-linux-aarch64.tar.gz` (for 64-bit ARM servers, e.g. Raspberry Pi 5)
