#!/usr/bin/env bash
# Cut a release: bump the version, tag it, push.
#
# The pipeline refuses to build a tag whose version disagrees with Cargo.toml, so
# this does both together rather than leaving them to drift.
#
#   ./scripts/release.sh 0.2.0
set -euo pipefail

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <major.minor.patch>" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

if [[ -n "$(git status --porcelain)" ]]; then
  echo "working tree is dirty; commit or stash first" >&2
  exit 1
fi
if git rev-parse "v$version" >/dev/null 2>&1; then
  echo "tag v$version already exists" >&2
  exit 1
fi

# Workspace version, which every crate inherits, and tauri.conf.json, which
# carries its own copy and names the bundle.
#
# Done in python rather than sed: BSD sed has no 0,/re/ range, so the obvious
# one-liner silently matches nothing and ships a tag the pipeline then rejects.
python3 - "$version" <<'PY'
import re, sys
version = sys.argv[1]

for path, pattern, repl in [
    ("Cargo.toml", r'^version = "[^"]*"', f'version = "{version}"'),
    ("src-tauri/tauri.conf.json", r'"version": "[^"]*"', f'"version": "{version}"'),
]:
    text = open(path).read()
    new, n = re.subn(pattern, repl, text, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"{path}: expected exactly one version to replace, changed {n}")
    open(path, "w").write(new)
PY

cargo check --workspace --quiet   # refreshes Cargo.lock with the new version

# Fail loudly rather than tagging a version that only half-landed.
grep -q "^version = \"$version\"" Cargo.toml \
  || { echo "Cargo.toml was not updated" >&2; exit 1; }
grep -q "\"version\": \"$version\"" src-tauri/tauri.conf.json \
  || { echo "tauri.conf.json was not updated" >&2; exit 1; }

git add Cargo.toml Cargo.lock src-tauri/tauri.conf.json
git commit -m "Release v$version"
git tag -a "v$version" -m "v$version"

echo
echo "Tagged v$version. To publish:"
echo "  git push origin main && git push origin v$version"
echo
echo "That starts the release workflow, which bundles, smoke-tests and opens a"
echo "DRAFT release. Review it on GitHub before publishing."
