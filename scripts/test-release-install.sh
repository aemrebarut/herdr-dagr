#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
source_tree="$work/source"
assets="$work/assets"
install_dir="$work/install"
safe_path="$work/path"
mkdir -p "$source_tree/scripts" "$assets" "$install_dir" "$safe_path"

cp "$root/scripts/install.sh" "$source_tree/scripts/install.sh"
printf '%s\n' 'version = "0.3.1"' > "$source_tree/herdr-plugin.toml"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -eu' \
  'printf source-fallback > "${DAGR_TEST_FALLBACK_MARKER:?}"' \
  > "$source_tree/scripts/build.sh"
chmod +x "$source_tree/scripts/install.sh" "$source_tree/scripts/build.sh"

git -C "$source_tree" init -q
git -C "$source_tree" config user.name 'dagr release test'
git -C "$source_tree" config user.email 'dagr-release-test@example.invalid'
git -C "$source_tree" add .
git -C "$source_tree" -c commit.gpgsign=false commit -qm fixture
head_commit=$(git -C "$source_tree" rev-parse HEAD)

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  *) echo "unsupported release-test host" >&2; exit 1 ;;
esac

payload="$work/payload"
mkdir -p "$payload"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" "dagr fixture 0.3.1"' > "$payload/dagr"
chmod +x "$payload/dagr"
archive="dagr-$target.tar.gz"
checksum="dagr-$target.sha256"
tar -czf "$assets/$archive" -C "$payload" dagr
if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$assets/$archive" | awk '{print $1}')
else
  digest=$(shasum -a 256 "$assets/$archive" | awk '{print $1}')
fi
printf '%s  %s\n' "$digest" "$archive" > "$assets/$checksum"
printf '%s\n' "$head_commit" > "$assets/COMMIT"

for command in bash dirname sed head uname rm git mktemp curl tr awk sha256sum shasum mkdir tar gzip install mv; do
  resolved=$(command -v "$command" 2>/dev/null || true)
  if [ -n "$resolved" ]; then
    ln -s "$resolved" "$safe_path/$command"
  fi
done
test -x "$safe_path/bash"
test -x "$safe_path/curl"
test ! -e "$safe_path/cargo"

printf '%s\n' '#!/bin/sh' 'printf "%s\n" old-install' > "$install_dir/dagr"
chmod +x "$install_dir/dagr"
PATH="$safe_path" \
  DAGR_RELEASE_BASE="file://$assets" \
  DAGR_INSTALL_BIN_DIR="$install_dir" \
  "$safe_path/bash" "$source_tree/scripts/install.sh"
test "$("$install_dir/dagr")" = 'dagr fixture 0.3.1'

printf '%064d  %s\n' 0 "$archive" > "$assets/$checksum"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" preserved-old' > "$install_dir/dagr"
chmod +x "$install_dir/dagr"
if PATH="$safe_path" \
  DAGR_RELEASE_BASE="file://$assets" \
  DAGR_INSTALL_BIN_DIR="$install_dir" \
  "$safe_path/bash" "$source_tree/scripts/install.sh" >"$work/checksum.log" 2>&1; then
  echo "checksum mismatch unexpectedly installed" >&2
  exit 1
fi
grep -q 'checksum mismatch' "$work/checksum.log"
test "$("$install_dir/dagr")" = 'preserved-old'

printf '%s\n' '#!/bin/sh' 'exit 99' > "$safe_path/cargo"
chmod +x "$safe_path/cargo"
fallback_marker="$work/fallback"
PATH="$safe_path" \
  DAGR_RELEASE_BASE="file://$work/no-release" \
  DAGR_INSTALL_BIN_DIR="$install_dir" \
  DAGR_TEST_FALLBACK_MARKER="$fallback_marker" \
  "$safe_path/bash" "$source_tree/scripts/install.sh" >"$work/fallback.log" 2>&1
test "$(sed -n '1p' "$fallback_marker")" = 'source-fallback'
grep -q 'building this source with Cargo' "$work/fallback.log"

rm "$safe_path/uname"
printf '%s\n' \
  '#!/bin/sh' \
  'case "$1" in' \
  '  -s) printf "%s\n" Plan9 ;;' \
  '  -m) printf "%s\n" mips64 ;;' \
  'esac' \
  > "$safe_path/uname"
chmod +x "$safe_path/uname"
unsupported_marker="$work/unsupported"
PATH="$safe_path" \
  DAGR_RELEASE_BASE="file://$assets" \
  DAGR_INSTALL_BIN_DIR="$install_dir" \
  DAGR_TEST_FALLBACK_MARKER="$unsupported_marker" \
  "$safe_path/bash" "$source_tree/scripts/install.sh" >"$work/unsupported.log" 2>&1
test "$(sed -n '1p' "$unsupported_marker")" = 'source-fallback'
grep -q 'no prebuilt exists for Plan9-mips64' "$work/unsupported.log"

echo "Unix release install OK: exact no-Cargo, checksum refusal, unreleased/unsupported fallback"
