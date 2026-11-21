# NTX Panel

Panneau léger et sécurisé de supervision et d'administration de serveurs Linux sur réseau local, avec découverte mDNS zéro-configuration, transport gRPC chiffré TLS (TOFU SHA-256), authentification challenge-response HMAC-SHA256, gestion de processus/services et émulateur terminal VT100 distant.

## Architecture & Sécurité (v0.3.1)

- **Transport TLS obligatoire** : `ntx-server` génère automatiquement une paire ECDSA P-256 (`server.crt` / `server.key` en `0600`) et expose son empreinte SHA-256 (`AA:BB:...`) dans `/etc/ntx-server/`, en console au premier démarrage et dans les annonces mDNS. `ntx-viewer` vérifie et épingle l'empreinte (TOFU) avec alerte bloquante si le certificat change.
- **Authentification Challenge-Response** : échange `GetAuthChallenge` + `HMAC-SHA256(SHA-256(PSK), nonce || ":" || viewer_id)` en temps constant, jeton de session temporaire (TTL configurable, 3600s par défaut), protection contre les rejeux et rate-limiting par IP.
- **Client HTTP/2 multiplexé persistant** : `ntx-viewer` (Windows & Linux) maintient une seule connexion TLS persistante par serveur pour le monitoring temps réel, les actions processus/services et le terminal VT100 (`htop`, `vim`, `screen`, `tmux`, `sudo`, etc.).
- **Services gérés (PTY temps réel) & Docker** : supervision native de services personnalisés côté `ntx-server` (ex. serveur Minecraft, bot, script) avec console interactive temps réel (`stdin`/`stdout`), arrêt propre (`stop_command`), auto-restart et gestion complète des conteneurs Docker (`logs`, `attach` interactif, `shell`, `exec`).
- **Vue d'ensemble et Vue & Métriques retravaillées** : tableau de bord multi-serveurs (mode Cartes enrichies avec sparklines + mode Tableau triable, centre d'alertes) et fiche serveur avec graphiques 2×2 auto-adaptatifs (CPU, RAM, Réseau avec filtre par interface, I/O Disque), export CSV et synthèse matérielle.

## NTX Server (Agent Linux)

`ntx-server` est un démon systemd autonome conçu pour les nœuds Linux `x86_64` et `aarch64` / ARM64 (Debian, Ubuntu, Arch Linux, Fedora, Raspberry Pi OS 64-bit).

### Installation rapide en 1 commande

Exécutez en tant qu'utilisateur avec `sudo` ou `root` :

```bash
curl -sSL https://raw.githubusercontent.com/Nathix111/ntx-panel/master/scripts/install.sh | bash
```

Le script :
1. Détecte l'architecture (`x86_64` ou `aarch64`).
2. Télécharge le binaire `ntx-server` strippé et vérifie son empreinte `SHA256SUMS`.
3. L'installe dans `/usr/local/bin/ntx-server` et configure l'unité systemd (`ntx-server.service`) avec la configuration dans `/etc/ntx-server/` (`0600`).
4. Affiche la clé pré-partagée (`PSK`) et l'empreinte TLS SHA-256 à vérifier dans `ntx-viewer`.

### Désinstallation & purge complète

```bash
curl -sSL https://raw.githubusercontent.com/Nathix111/ntx-panel/master/scripts/uninstall.sh | bash
```

Ou directement via le binaire installé :

```bash
sudo ntx-server uninstall --purge
```

### Commandes de gestion du serveur

```bash
# Vérifier le statut du service
sudo ntx-server status
# ou
systemctl status ntx-server

# Afficher la configuration, la PSK et l'empreinte TLS SHA-256
sudo ntx-server config

# Désinstallation standard (conserve /etc/ntx-server)
sudo ntx-server uninstall

# Purge complète (supprime binaire, service et configurations)
sudo ntx-server uninstall --purge
```

## NTX Viewer (Client Desktop Windows / Linux)

```bash
cargo run --release -p ntx-viewer
```

## Releases & Binaires précompilés

Les archives Linux sont générées via GitHub Actions :
- `ntx-server-linux-x86_64.tar.gz` (serveurs x86 64-bit)
- `ntx-server-linux-aarch64.tar.gz` (serveurs ARM 64-bit, ex: Raspberry Pi 5)
- `SHA256SUMS` (empreintes d'intégrité des archives)
