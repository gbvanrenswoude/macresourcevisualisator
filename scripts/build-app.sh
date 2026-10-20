#!/usr/bin/env bash
# Build Tessera.app into target/Tessera.app.
#
#   scripts/build-app.sh
#
# Signs with your "Apple Development" / "Developer ID" identity when one is
# in the keychain, otherwise ad-hoc. Override with CODESIGN_IDENTITY=...
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
APP=target/Tessera.app

echo "==> Building tessera $VERSION (release)"
cargo build --release -p tessera

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/tessera "$APP/Contents/MacOS/tessera"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

IDENTITY="${CODESIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application|Apple Development/ { print $2; exit }')
fi
echo "==> Signing (${IDENTITY:-ad-hoc})"
codesign --force --timestamp=none --sign "${IDENTITY:--}" "$APP"
codesign --verify --strict "$APP"

echo "==> Done: $APP ($(du -sh "$APP" | cut -f1))"
