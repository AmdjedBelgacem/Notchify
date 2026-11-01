#!/usr/bin/env bash
# Build NotchForge.app (Apple Silicon by default, optionally universal),
# sign it, and optionally notarize it.
#
#   packaging/bundle.sh                       # arm64, ad-hoc signed
#   UNIVERSAL=1 packaging/bundle.sh           # arm64 + x86_64 via lipo
#   CODESIGN_IDENTITY="Developer ID Application: You (TEAMID)" packaging/bundle.sh
#   CODESIGN_IDENTITY=… NOTARY_PROFILE=notchforge packaging/bundle.sh
#
# NOTARY_PROFILE is a keychain profile created once with:
#   xcrun notarytool store-credentials notchforge --apple-id … --team-id … --password …
#
# Note: macOS ties privacy permissions (Accessibility, Full Disk Access) to the
# code signature. Ad-hoc builds get a new identity each time, so you may have
# to re-grant permissions after rebuilding. Use a stable signing identity to
# avoid that.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
DIST="$ROOT/dist"
APP="$DIST/NotchForge.app"
BINS=(notchforge notchforge-notify)

if [[ "$(uname)" != "Darwin" ]]; then
  echo "bundle.sh must run on macOS" >&2
  exit 1
fi

echo "==> Building NotchForge $VERSION ($BUILD)"
cargo build --release --target aarch64-apple-darwin --bins
if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  rustup target add x86_64-apple-darwin >/dev/null
  cargo build --release --target x86_64-apple-darwin --bins
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
for bin in "${BINS[@]}"; do
  if [[ "${UNIVERSAL:-0}" == "1" ]]; then
    lipo -create -output "$APP/Contents/MacOS/$bin" \
      "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
  else
    cp "target/aarch64-apple-darwin/release/$bin" "$APP/Contents/MacOS/$bin"
  fi
done
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
cp config/notchforge.example.toml "$APP/Contents/Resources/"
packaging/make_icns.sh packaging/icon/AppIcon-1024.png "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"

IDENTITY="${CODESIGN_IDENTITY:--}"
echo "==> Signing with identity: $IDENTITY"
SIGN_ARGS=(--force --options runtime --entitlements packaging/NotchForge.entitlements --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then
  SIGN_ARGS+=(--timestamp)
fi
# Sign nested executables first, then the bundle.
codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/notchforge-notify"
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  [[ "$IDENTITY" == "-" ]] && { echo "Notarization needs a Developer ID identity" >&2; exit 1; }
  echo "==> Notarizing"
  ZIP="$DIST/NotchForge-$VERSION.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "==> Notarized: $ZIP"
fi

echo "==> Done: $APP"

# INSTALL=1: quit the running copy, replace /Applications/NotchForge.app and
# relaunch. (A plain `cp -R` onto an existing bundle copies the new app *inside*
# the old one, so the old build keeps launching.)
if [[ "${INSTALL:-0}" == "1" ]]; then
  DEST="/Applications/NotchForge.app"
  echo "==> Installing to $DEST"
  osascript -e 'tell application id "dev.notchforge.NotchForge" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do pgrep -xq notchforge || break; sleep 0.25; done
  pkill -x notchforge 2>/dev/null || true
  rm -rf "$DEST"
  ditto "$APP" "$DEST"
  open "$DEST"
  echo "==> Installed and launched build $BUILD"
else
  echo "    Install: INSTALL=1 packaging/bundle.sh   (quits, replaces and relaunches)"
fi
echo "    CLI:     ln -sf /Applications/NotchForge.app/Contents/MacOS/notchforge-notify /usr/local/bin/"
