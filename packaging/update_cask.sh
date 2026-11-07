#!/usr/bin/env bash
# Fill the version and sha256 of the Homebrew cask from a release zip.
#
#   packaging/update_cask.sh VERSION ZIP [CASK]
#   packaging/update_cask.sh 0.2.0 dist/Notchisland-0.2.0.zip
#   packaging/update_cask.sh 0.2.0 dist/Notchisland-0.2.0.zip ../homebrew-tap/Casks/notchisland.rb
#
# VERSION may carry a leading "v". CASK defaults to packaging/homebrew/notchisland.rb.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 VERSION ZIP [CASK]" >&2
  exit 2
fi
VERSION="${1#v}"
ZIP="$2"
CASK="${3:-$(cd "$(dirname "$0")" && pwd)/homebrew/notchisland.rb}"

[[ "$VERSION" =~ ^[0-9]+(\.[0-9]+)*([-+][0-9A-Za-z.-]+)?$ ]] || { echo "bad version: $VERSION" >&2; exit 2; }
[[ -f "$ZIP" ]] || { echo "zip not found: $ZIP" >&2; exit 1; }
[[ -f "$CASK" ]] || { echo "cask not found: $CASK" >&2; exit 1; }

SHA="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
# BSD and GNU sed differ on -i; write through a temp file instead.
TMP="$(mktemp)"
sed -E \
  -e "s/^([[:space:]]*version )\"[^\"]*\"/\1\"$VERSION\"/" \
  -e "s/^([[:space:]]*sha256 )\"[^\"]*\"/\1\"$SHA\"/" \
  "$CASK" > "$TMP"
cat "$TMP" > "$CASK"
rm -f "$TMP"
grep -q "version \"$VERSION\"" "$CASK" && grep -q "sha256 \"$SHA\"" "$CASK" || { echo "failed to update $CASK" >&2; exit 1; }
echo "Updated $CASK: version $VERSION, sha256 $SHA"
