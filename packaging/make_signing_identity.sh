#!/usr/bin/env bash
# Create a signing identity for local builds, so macOS keeps permissions such as
# Full Disk Access and Accessibility across rebuilds. Ad-hoc builds get a new
# identity every time, which makes macOS treat each one as a different app.
#
# It lives in its own keychain file, away from the login keychain, and the
# self-signed certificate is trusted for code signing in the *user* domain —
# nothing else about trust changes. It can only sign for this Mac; sharing the
# app with others still needs a Developer ID.
#
#   packaging/make_signing_identity.sh          # create it, if there is none
#   rm -rf ~/.config/notchify/signing           # remove it
set -euo pipefail

DIR="$HOME/.config/notchify/signing"
KEYCHAIN="$DIR/signing.keychain-db"
NAME="Notchify Local Signing"

if [[ -f "$KEYCHAIN" ]]; then
  echo "==> Local signing identity already exists: $KEYCHAIN"
  exit 0
fi

mkdir -p "$DIR"
chmod 700 "$DIR"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

(umask 077; /usr/bin/openssl rand -hex 32 > "$DIR/password")
PASSWORD="$(cat "$DIR/password")"

cat > "$WORK/cert.conf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$WORK/cert.conf" \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -out "$WORK/identity.p12" -passout pass:notchify

security create-keychain -p "$PASSWORD" "$KEYCHAIN"
security set-keychain-settings "$KEYCHAIN"
security unlock-keychain -p "$PASSWORD" "$KEYCHAIN"
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P notchify -T /usr/bin/codesign
# Lets codesign use the key without a password dialog.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PASSWORD" "$KEYCHAIN" >/dev/null

# A self-signed certificate is only a valid codesigning identity once it is
# trusted for code signing (user domain; no sudo, no other change).
security find-certificate -c "$NAME" -p "$KEYCHAIN" > "$WORK/cert.pem"
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$WORK/cert.pem"

# Fail loudly rather than leaving an empty keychain that bundle.sh silently
# works around with ad-hoc signing.
if ! security find-identity -v -p codesigning "$KEYCHAIN" | grep -qE '[1-9][0-9]* valid identit'; then
  echo "!! no valid signing identity was created; bundle.sh will fall back to ad-hoc" >&2
  exit 1
fi

echo "==> Created local signing identity \"$NAME\""
