#!/usr/bin/env bash
# bump.sh — bump the app version across all sources of truth and push a tag.
#
# Usage: ./scripts/bump.sh [major|minor|patch]
#
# What this updates:
#   - package.json                  (version)
#   - src-tauri/Cargo.toml          ([package] version)
#   - src-tauri/Cargo.lock          (refreshed via cargo update -p fuji-culler)
#   - src-tauri/tauri.conf.json     (version)
#
# Then:
#   - commits the four files with "chore: bump version to X.Y.Z"
#   - tags vX.Y.Z
#   - pushes the branch and the tag (which triggers .github/workflows/release.yml)
set -euo pipefail

BUMP_TYPE="${1:-}"

usage() {
  echo "Usage: $0 [major|minor|patch]"
  exit 1
}

if [[ -z "$BUMP_TYPE" ]]; then usage; fi
case "$BUMP_TYPE" in major|minor|patch) ;; *) usage ;; esac

# Ensure clean working tree — bumping with uncommitted changes would lump
# unrelated edits into the version commit.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: working tree has uncommitted changes. Commit or stash them first."
  exit 1
fi

CURRENT=$(node -p "require('./package.json').version")

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP_TYPE" in
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  patch) PATCH=$((PATCH + 1)) ;;
esac

NEW="$MAJOR.$MINOR.$PATCH"
TAG="v$NEW"

echo "Bumping $CURRENT → $NEW"

# package.json
node -e "
  const fs = require('fs');
  const p = JSON.parse(fs.readFileSync('package.json', 'utf8'));
  p.version = '$NEW';
  fs.writeFileSync('package.json', JSON.stringify(p, null, 2) + '\n');
"

# src-tauri/Cargo.toml — only the first occurrence (the [package] version)
awk -v cur="$CURRENT" -v new="$NEW" '
  !done && $0 == "version = \"" cur "\"" { print "version = \"" new "\""; done=1; next }
  { print }
' src-tauri/Cargo.toml > src-tauri/Cargo.toml.tmp && mv src-tauri/Cargo.toml.tmp src-tauri/Cargo.toml

# src-tauri/tauri.conf.json
node -e "
  const fs = require('fs');
  const c = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  c.version = '$NEW';
  fs.writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(c, null, 2) + '\n');
"

# Refresh Cargo.lock so the fuji-culler package version matches Cargo.toml
(cd src-tauri && cargo update -p fuji-culler --precise "$NEW")

# Commit and tag — pushing the tag triggers the release workflow.
git add package.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json
git commit -m "chore: bump version to $NEW"
git tag "$TAG"
git push origin HEAD
git push origin "$TAG"

echo "Released $TAG"
