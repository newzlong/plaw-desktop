# macOS Update and Uninstall Guide

This page documents supported update and uninstall procedures for Plaw on macOS (OS X).

Last verified: **February 22, 2026**.

## 1) Check current install method

```bash
which plaw
plaw --version
```

Typical locations:

- Homebrew: `/opt/homebrew/bin/plaw` (Apple Silicon) or `/usr/local/bin/plaw` (Intel)
- Cargo/bootstrap/manual: `~/.cargo/bin/plaw`

If both exist, your shell `PATH` order decides which one runs.

## 2) Update on macOS

### A) Homebrew install

```bash
brew update
brew upgrade plaw
plaw --version
```

### B) Clone + bootstrap install

From your local repository checkout:

```bash
git pull --ff-only
./bootstrap.sh --prefer-prebuilt
plaw --version
```

If you want source-only update:

```bash
git pull --ff-only
cargo install --path . --force --locked
plaw --version
```

### C) Manual prebuilt binary install

Re-run your download/install flow with the latest release asset, then verify:

```bash
plaw --version
```

## 3) Uninstall on macOS

### A) Stop and remove background service first

This prevents the daemon from continuing to run after binary removal.

```bash
plaw service stop || true
plaw service uninstall || true
```

Service artifacts removed by `service uninstall`:

- `~/Library/LaunchAgents/com.plaw.daemon.plist`

### B) Remove the binary by install method

Homebrew:

```bash
brew uninstall plaw
```

Cargo/bootstrap/manual (`~/.cargo/bin/plaw`):

```bash
cargo uninstall plaw || true
rm -f ~/.cargo/bin/plaw
```

### C) Optional: remove local runtime data

Only run this if you want a full cleanup of config, auth profiles, logs, and workspace state.

```bash
rm -rf ~/.plaw
```

## 4) Verify uninstall completed

```bash
command -v plaw || echo "plaw binary not found"
pgrep -fl plaw || echo "No running plaw process"
```

If `pgrep` still finds a process, stop it manually and re-check:

```bash
pkill -f plaw
```

## Related docs

- [One-Click Bootstrap](../one-click-bootstrap.md)
- [Commands Reference](../commands-reference.md)
- [Troubleshooting](../troubleshooting.md)
