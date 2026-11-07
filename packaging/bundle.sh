#!/usr/bin/env bash
# Build Notchisland.app (Apple Silicon by default, optionally universal),
# sign it, and optionally notarize it.
#
#   packaging/bundle.sh                       # arm64, signed for this Mac
#   INSTALL=1 packaging/bundle.sh             # quick local build, then install it
#   INSTALL=1 RELEASE=1 packaging/bundle.sh   # the fully optimized build, then install it
#   UNIVERSAL=1 packaging/bundle.sh           # arm64 + x86_64 via lipo
#   CODESIGN_IDENTITY="Developer ID Application: You (TEAMID)" packaging/bundle.sh
#   CODESIGN_IDENTITY=… NOTARY_PROFILE=notchforge packaging/bundle.sh
#
# The user-facing name is Notchisland. The bundle identifier, executable names
# and entitlements file keep their NotchForge names on purpose: macOS ties
# privacy permissions to the bundle id + signature, so changing them would make
# every user grant Accessibility, Calendar, etc. again.
#
# NOTARY_PROFILE is a keychain profile created once with:
#   xcrun notarytool store-credentials notchforge --apple-id … --team-id … --password …
#
# Note: macOS ties privacy permissions (Accessibility, Full Disk Access) to the
# code signature. Ad-hoc builds get a new identity each time, so permissions
# would have to be granted again after every rebuild. Without CODESIGN_IDENTITY,
# builds use the local identity from packaging/make_signing_identity.sh if it
# exists, and are ad-hoc signed if not.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 1)"
DIST="$ROOT/dist"
APP_NAME="Notchisland"
APP="$DIST/$APP_NAME.app"
# Bundle name used before the rename; INSTALL=1 removes it from /Applications.
LEGACY_APP_NAME="NotchForge"
BINS=(notchforge notchforge-notify notchforge-mcp)

if [[ "$(uname)" != "Darwin" ]]; then
  echo "bundle.sh must run on macOS" >&2
  exit 1
fi

# Trying a change locally shouldn't wait for link-time optimization, which is
# slow and can take many minutes when the Mac is short on memory.
if [[ "${INSTALL:-0}" == "1" && "${RELEASE:-0}" != "1" && "${UNIVERSAL:-0}" != "1" ]]; then
  echo "==> Building $APP_NAME $VERSION ($BUILD), quick local build"
  cargo build --bins
  BUILT="target/debug"
else
  echo "==> Building $APP_NAME $VERSION ($BUILD)"
  cargo build --release --target aarch64-apple-darwin --bins
  BUILT="target/aarch64-apple-darwin/release"
fi
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
    cp "$BUILT/$bin" "$APP/Contents/MacOS/$bin"
  fi
done
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
cp config/notchforge.example.toml "$APP/Contents/Resources/"
packaging/make_icns.sh packaging/icon/AppIcon-1024.png "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"
# Native Shortcuts actions (App Intents): a small Swift library plus the
# metadata Shortcuts reads. Skipped without Xcode.
echo "==> Building the Shortcuts actions"
packaging/intents/build.sh "$APP" "$([[ "${UNIVERSAL:-0}" == "1" ]] && echo universal || echo arm64)"

IDENTITY="${CODESIGN_IDENTITY:--}"
LOCAL_KEYCHAIN="$HOME/.config/notchify/signing/signing.keychain-db"
LOCAL_ARGS=()
if [[ "$IDENTITY" == "-" && -f "$LOCAL_KEYCHAIN" ]]; then
  security unlock-keychain -p "$(cat "$(dirname "$LOCAL_KEYCHAIN")/password")" "$LOCAL_KEYCHAIN"
  IDENTITY="$(security find-identity -p codesigning "$LOCAL_KEYCHAIN" | awk '/\)/ {print $2; exit}')"
  LOCAL_ARGS=(--keychain "$LOCAL_KEYCHAIN")
fi
echo "==> Signing with identity: $IDENTITY"
# A Developer ID has a Team ID, so the bundled Shortcuts library passes
# library validation; local and ad-hoc identities don't (see the .local file).
ENTITLEMENTS=packaging/NotchForge.entitlements
if [[ "$IDENTITY" == "-" || ${#LOCAL_ARGS[@]} -gt 0 ]]; then
  ENTITLEMENTS=packaging/NotchForge.local.entitlements
fi
SIGN_ARGS=(--force --options runtime --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" ${LOCAL_ARGS[@]+"${LOCAL_ARGS[@]}"})
# Apple's timestamp service only serves Developer ID certificates.
if [[ "$IDENTITY" != "-" && ${#LOCAL_ARGS[@]} -eq 0 ]]; then
  SIGN_ARGS+=(--timestamp)
fi
# Sign nested code first (libraries, helper executables), then the bundle.
for lib in "$APP"/Contents/Frameworks/*.dylib; do
  [[ -e "$lib" ]] && codesign "${SIGN_ARGS[@]}" "$lib"
done
for bin in "${BINS[@]}"; do
  [[ "$bin" == notchforge ]] || codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/$bin"
done
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  [[ -z "${CODESIGN_IDENTITY:-}" ]] && { echo "Notarization needs a Developer ID identity" >&2; exit 1; }
  echo "==> Notarizing"
  ZIP="$DIST/$APP_NAME-$VERSION.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "==> Notarized: $ZIP"
fi

echo "==> Done: $APP"

# INSTALL=1: quit the running copy, replace /Applications/Notchisland.app and
# relaunch. (A plain `cp -R` onto an existing bundle copies the new app *inside*
# the old one, so the old build keeps launching.) A copy installed under the
# old name (/Applications/NotchForge.app) is removed so there is only one.
if [[ "${INSTALL:-0}" == "1" ]]; then
  DEST="/Applications/$APP_NAME.app"
  LEGACY_DEST="/Applications/$LEGACY_APP_NAME.app"
  echo "==> Installing to $DEST"
  # Same bundle id for both names, so this quits whichever copy is running.
  osascript -e 'tell application id "dev.notchforge.NotchForge" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 1 20); do pgrep -xq notchforge || break; sleep 0.25; done
  pkill -x notchforge 2>/dev/null || true
  rm -rf "$DEST"
  if [[ -d "$LEGACY_DEST" ]]; then
    echo "==> Removing the old $LEGACY_DEST"
    rm -rf "$LEGACY_DEST"
  fi
  # "Launch at login" writes the executable's path into a LaunchAgent; point an
  # agent written by the old bundle at the new one.
  LOGIN_AGENT="$HOME/Library/LaunchAgents/dev.notchforge.NotchForge.plist"
  if [[ -f "$LOGIN_AGENT" ]] && grep -q "$LEGACY_DEST/" "$LOGIN_AGENT"; then
    sed -i '' "s#$LEGACY_DEST/#$DEST/#g" "$LOGIN_AGENT"
  fi
  ditto "$APP" "$DEST"
  open "$DEST"
  echo "==> Installed and launched build $BUILD"
else
  echo "    Install: INSTALL=1 packaging/bundle.sh   (quits, replaces and relaunches)"
fi
echo "    CLI:     ln -sf /Applications/$APP_NAME.app/Contents/MacOS/notchforge-notify /usr/local/bin/"
