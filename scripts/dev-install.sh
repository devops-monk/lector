#!/usr/bin/env bash
# Build Lector, sign it with the local certificate, and install it to
# /Applications -- replacing whatever is there.
#
# Using one canonical location matters more than it sounds: macOS tracks
# permissions per app, and a copy left in target/release/bundle is a *different*
# app to the system even though it has the same name and icon. Running that one
# while /Applications holds another is how the Accessibility grant appears to be
# enabled and ignored at the same time.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="Lector Code Signing"
if ! security find-certificate -c "$NAME" >/dev/null 2>&1; then
  echo "No local signing certificate. Run ./scripts/make-signing-cert.sh first." >&2
  echo "Without it the build is ad-hoc signed and Accessibility resets on every build." >&2
  exit 1
fi

pkill -f 'Lector.app/Contents/MacOS' 2>/dev/null || true
APPLE_SIGNING_IDENTITY="$NAME" cargo tauri build --bundles app

rm -rf /Applications/Lector.app
ditto target/release/bundle/macos/Lector.app /Applications/Lector.app
xattr -dr com.apple.quarantine /Applications/Lector.app 2>/dev/null || true

echo
codesign -d -r- /Applications/Lector.app 2>&1 | tail -1
open /Applications/Lector.app
echo "Installed and launched from /Applications."
