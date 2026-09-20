#!/usr/bin/env bash
# Create a self-signed code-signing certificate for local builds.
#
# Why this exists: macOS identifies apps in the privacy database (Accessibility,
# Screen Recording, and so on) by their code signature. An *ad-hoc* signature has
# no certificate, so the identity is a hash of the binary itself -- which changes
# on every single build. The result is that the Accessibility grant silently
# stops applying every time you rebuild, while System Settings still shows the
# app as enabled, because the row refers to a signature that no longer exists.
#
# Signing with a certificate instead makes the designated requirement
#
#     identifier "com.devopsmonk.lector" and certificate leaf = H"<cert hash>"
#
# which is stable for the life of the certificate. Grant Accessibility once and
# it stays granted across rebuilds.
#
# This does NOT make Gatekeeper trust the app: it is self-signed, not notarized,
# so first-launch quarantine still applies. Only a paid Apple Developer ID fixes
# that. Releases stay ad-hoc signed, because CI has no access to this key.
#
#     ./scripts/make-signing-cert.sh
#     export APPLE_SIGNING_IDENTITY="Lector Code Signing"
set -euo pipefail

NAME="Lector Code Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-certificate -c "$NAME" >/dev/null 2>&1; then
  echo "\"$NAME\" already exists in the login keychain. Nothing to do."
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/ext.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = Lector Code Signing
[v3]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
EOF

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$tmp/key.pem" -out "$tmp/cert.pem" -config "$tmp/ext.cnf" 2>/dev/null

# macOS cannot read OpenSSL 3's default PKCS#12 encryption, so the bundle is
# written with the older algorithms its Security framework understands.
openssl pkcs12 -export -out "$tmp/id.p12" \
  -inkey "$tmp/key.pem" -in "$tmp/cert.pem" -name "$NAME" \
  -passout pass:lector \
  -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 2>/dev/null

security import "$tmp/id.p12" -k "$KEYCHAIN" -P lector -T /usr/bin/codesign

echo
echo "Created \"$NAME\" in your login keychain."
echo "Build with it:"
echo "    APPLE_SIGNING_IDENTITY=\"$NAME\" cargo tauri build --bundles app"
echo
echo "Note: 'security find-identity -v -p codesigning' will not list it, because"
echo "the certificate is not trusted as a root. codesign uses it regardless, which"
echo "is all that is needed for a stable identity."
