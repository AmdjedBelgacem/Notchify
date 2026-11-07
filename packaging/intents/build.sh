#!/usr/bin/env bash
# Build Notchisland's native Shortcuts actions (App Intents) into an app bundle.
#
#   packaging/intents/build.sh dist/Notchisland.app [arm64|universal]
#
# Compiles Intents.swift into Contents/Frameworks/libNotchislandIntents.dylib
# and writes the action metadata Shortcuts reads to
# Contents/Resources/Metadata.appintents. Needs Xcode (not just the command
# line tools); without it the bundle simply has no native actions and the
# notchisland:// links and integrations/shortcuts/ still work.
set -euo pipefail

APP="$1"
ARCHS="${2:-arm64}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/Intents.swift"
MIN="13.0"
MODULE="NotchislandIntents"

if ! xcrun --find appintentsmetadataprocessor >/dev/null 2>&1; then
  echo "    (no Xcode: skipping native Shortcuts actions)"
  exit 0
fi
SDK="$(xcrun --sdk macosx --show-sdk-path)"
TOOLCHAIN="$(dirname "$(dirname "$(xcrun --find swiftc)")")"
XCODE_BUILD="$(xcodebuild -version 2>/dev/null | awk '/Build version/ {print $3}')"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/protocols.json" <<'JSON'
["AppIntent", "EntityQuery", "AppEntity", "TransientEntity", "AppEnum", "AppShortcutProviding",
 "AppShortcutsProvider", "AnyResolverProviding", "AppIntentsPackage", "DynamicOptionsProvider"]
JSON

case "$ARCHS" in
  universal) TARGETS=(arm64 x86_64) ;;
  *) TARGETS=(arm64) ;;
esac

LIBS=()
for arch in "${TARGETS[@]}"; do
  triple="$arch-apple-macos$MIN"
  # Compile (whole module, so the compiler writes the constant values the
  # metadata processor reads), then link the object into a dylib.
  xcrun swiftc -swift-version 5 -parse-as-library -wmo -c -O \
    -module-name "$MODULE" -target "$triple" -sdk "$SDK" \
    -emit-const-values-path "$WORK/$arch.swiftconstvalues" \
    -Xfrontend -const-gather-protocols-file -Xfrontend "$WORK/protocols.json" \
    "$SRC" -o "$WORK/$arch.o"
  xcrun swiftc -emit-library -target "$triple" -sdk "$SDK" -module-name "$MODULE" \
    -Xlinker -install_name -Xlinker "@rpath/lib$MODULE.dylib" \
    "$WORK/$arch.o" -o "$WORK/lib$MODULE-$arch.dylib"
  LIBS+=("$WORK/lib$MODULE-$arch.dylib")
done

mkdir -p "$APP/Contents/Frameworks" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/Frameworks/lib$MODULE.dylib" "${LIBS[@]}"

echo "$SRC" > "$WORK/sources.txt"
echo "$WORK/${TARGETS[0]}.swiftconstvalues" > "$WORK/constvals.txt"
xcrun appintentsmetadataprocessor \
  --output "$APP/Contents/Resources" \
  --toolchain-dir "$TOOLCHAIN" \
  --module-name "$MODULE" \
  --sdk-root "$SDK" \
  --xcode-version "${XCODE_BUILD:-17A1}" \
  --platform-family macOS \
  --deployment-target "$MIN" \
  --target-triple "${TARGETS[0]}-apple-macos$MIN" \
  --source-file-list "$WORK/sources.txt" \
  --swift-const-vals-list "$WORK/constvals.txt" \
  --no-app-shortcuts-localization \
  --force
ls "$APP/Contents/Resources/Metadata.appintents" >/dev/null
echo "    native Shortcuts actions: $(ls "$APP/Contents/Resources/Metadata.appintents" | tr '\n' ' ')"
