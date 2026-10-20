#!/usr/bin/env bash
# Build Tessera.app and install it into /Applications (or ~/Applications).
#
#   scripts/install.sh            build, install and launch
#   scripts/install.sh --no-open  build and install only
#
# INSTALL_DIR=... installs somewhere other than /Applications.
set -euo pipefail
cd "$(dirname "$0")/.."

scripts/build-app.sh

DEST="${INSTALL_DIR:-/Applications}"
if [[ ! -w "$DEST" ]]; then
  DEST="$HOME/Applications"
  mkdir -p "$DEST"
fi

# Quit a running copy so it can be replaced.
if pgrep -x tessera >/dev/null 2>&1; then
  echo "==> Quitting the running Tessera"
  osascript -e 'tell application id "com.gbvanrenswoude.tessera" to quit' >/dev/null 2>&1 || true
  for _ in {1..20}; do pgrep -x tessera >/dev/null || break; sleep 0.25; done
  pkill -x tessera 2>/dev/null || true
fi

echo "==> Installing to $DEST/Tessera.app"
rm -rf "$DEST/Tessera.app"
ditto target/Tessera.app "$DEST/Tessera.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST/Tessera.app" >/dev/null 2>&1 || true

if [[ "${1:-}" != "--no-open" ]]; then
  echo "==> Launching"
  open "$DEST/Tessera.app"
fi
echo "==> Installed. Look for the mosaic icon in your menu bar."
