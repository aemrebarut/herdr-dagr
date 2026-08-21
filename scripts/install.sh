#!/usr/bin/env bash
# Herdr install-time build: prefer the checksum-verified release binary only
# when it was built from this exact source revision. Source builds remain the
# fallback for unreleased or locally modified refs; released installs need no
# Rust toolchain.
set -euo pipefail

name="dagr"
repo="aemrebarut/herdr-dagr"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin_dir="${DAGR_INSTALL_BIN_DIR:-$root/bin}"
version="$(sed -nE 's/^version = "([^"]+)"/\1/p' "$root/herdr-plugin.toml" | head -1)"
tag="v$version"

target=""
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)                 target="aarch64-apple-darwin" ;;
  Darwin-x86_64)                target="x86_64-apple-darwin" ;;
  Linux-aarch64 | Linux-arm64)  target="aarch64-unknown-linux-musl" ;;
  Linux-x86_64)                 target="x86_64-unknown-linux-musl" ;;
esac

tmp=""
stage=""
cleanup() {
  [ -z "$tmp" ] || rm -rf "$tmp"
  [ -z "$stage" ] || rm -f "$stage"
}
trap cleanup EXIT

source_fallback() {
  reason="$1"
  if command -v cargo >/dev/null 2>&1; then
    echo "$name: $reason; building this source with Cargo" >&2
    cleanup
    trap - EXIT
    exec bash "$root/scripts/build.sh"
  fi
  echo "$name: $reason, and Cargo is not installed" >&2
  echo "$name: install the released revision, or install Rust to build this source" >&2
  exit 1
}

[ -n "$target" ] || source_fallback "no prebuilt exists for $(uname -s)-$(uname -m)"

archive="$name-$target.tar.gz"
checksum="$name-$target.sha256"
commit_marker="COMMIT"
base="${DAGR_RELEASE_BASE:-https://github.com/$repo/releases/download/$tag}"
tmp="$(mktemp -d)"

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --retry-delay 2 --retry-all-errors "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    return 1
  fi
}

head_commit=""
if command -v git >/dev/null 2>&1 &&
   git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  head_commit="$(git -C "$root" rev-parse HEAD 2>/dev/null || true)"
  if ! git -C "$root" diff --quiet --ignore-submodules -- ||
     ! git -C "$root" diff --cached --quiet --ignore-submodules -- ||
     [ -n "$(git -C "$root" ls-files --others --exclude-standard)" ]; then
    source_fallback "the checkout has local changes"
  fi
elif [ -f "$root/.git/HEAD" ]; then
  # Herdr's managed installs are detached shallow clones. This keeps the
  # no-toolchain path available even if `git` is not on the build-step PATH.
  head_commit="$(sed -nE 's/^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$/\1/p' "$root/.git/HEAD")"
fi
case "$head_commit" in
  '' | *[!0-9a-fA-F]*) source_fallback "cannot verify the checkout revision" ;;
esac

if ! download "$base/$commit_marker" "$tmp/$commit_marker"; then
  source_fallback "no release marker is available for $tag"
fi
release_commit="$(tr -d '[:space:]' < "$tmp/$commit_marker")"
case "$release_commit" in
  '' | *[!0-9a-fA-F]*) source_fallback "the $tag release marker is malformed" ;;
esac
if [ "$head_commit" != "$release_commit" ]; then
  source_fallback "checkout $head_commit does not match the $tag release revision $release_commit"
fi

echo "$name: downloading $archive ($tag)"
if ! download "$base/$archive" "$tmp/$archive" ||
   ! download "$base/$checksum" "$tmp/$checksum"; then
  source_fallback "no prebuilt asset is available for $tag"
fi

expected="$(awk 'NR == 1 {print $1}' "$tmp/$checksum")"
case "$expected" in
  '' | *[!0-9a-fA-F]*)
    echo "$name: malformed checksum asset $checksum" >&2
    exit 1
    ;;
esac
[ "${#expected}" -eq 64 ] || {
  echo "$name: malformed checksum asset $checksum" >&2
  exit 1
}
expected="$(printf '%s' "$expected" | tr 'A-F' 'a-f')"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"
else
  echo "$name: need sha256sum or shasum to verify the release asset" >&2
  exit 1
fi
if [ "$expected" != "$actual" ]; then
  echo "$name: checksum mismatch (expected $expected, got $actual)" >&2
  exit 1
fi

mkdir -p "$tmp/unpack" "$bin_dir"
tar -xzf "$tmp/$archive" -C "$tmp/unpack"
if [ ! -f "$tmp/unpack/$name" ]; then
  echo "$name: release archive does not contain $name" >&2
  exit 1
fi
stage="$(mktemp "$bin_dir/.dagr-install.XXXXXX")"
install -m 0755 "$tmp/unpack/$name" "$stage"
mv -f "$stage" "$bin_dir/$name"
stage=""
echo "$name: installed $bin_dir/$name"
