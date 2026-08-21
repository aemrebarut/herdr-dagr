#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
if [ ! -f "$root/scripts/build-public-tree.sh" ]; then
  echo "public staging construction is source-tree-only"
  exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/public"

bash "$root/scripts/build-public-tree.sh" "$stage" --stage-only

for dir in src tests samples skills assets demos/selfrun; do
  diff -qr "$root/$dir" "$stage/$dir"
done

for file in \
  Cargo.toml Cargo.lock herdr-plugin.toml CONTRACT.md .gitignore \
  LICENSE-MIT LICENSE-APACHE \
  scripts/open-dagr.sh scripts/open-dagr.ps1 \
  scripts/build.sh scripts/build.ps1 \
  scripts/install.sh scripts/install.ps1 \
  scripts/test-release-install.sh scripts/test-public-stage.sh \
  scripts/snapshot-svg.py \
  .github/workflows/release.yml .github/workflows/ci.yml; do
  cmp "$root/$file" "$stage/$file"
done

cmp "$root/scripts/public-overlays/README.md" "$stage/README.md"
test ! -e "$stage/src/action.rs"
test ! -e "$stage/demos/actions"

echo "public staging parity OK"
