#!/usr/bin/env bash
# Re-download every catalogued model and check it still hashes to what we claim.
#
# Upstream release assets do get re-uploaded. A stale hash means the app refuses
# to install a model that is actually fine, which looks like a broken download to
# the user -- so this is worth running before a release.
set -uo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0

# Pull the url/sha pairs straight out of the catalog so the two cannot drift.
urls=$(grep -o 'url: "[^"]*"' crates/lector-engine/src/catalog.rs | cut -d'"' -f2)
shas=$(grep -o 'sha256: "[^"]*"' crates/lector-engine/src/catalog.rs | cut -d'"' -f2)

paste <(echo "$urls") <(echo "$shas") | while IFS=$'\t' read -r url want; do
  name=$(basename "$url")
  printf '%s\n  ' "$name"
  if ! curl -sSL -o "$tmp/$name" "$url"; then
    echo "DOWNLOAD FAILED"; fail=1; continue
  fi
  got=$(shasum -a 256 "$tmp/$name" | cut -d' ' -f1)
  if [ "$got" = "$want" ]; then
    echo "ok  $got"
  else
    echo "MISMATCH"
    echo "    catalog: $want"
    echo "    actual:  $got"
    fail=1
  fi
  rm -f "$tmp/$name"
done

exit $fail
