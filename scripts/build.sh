#!/bin/sh
# Local-development build: produce the Cargo artifact and the stable plugin
# launch path. GitHub installs use scripts/install.sh and prefer a prebuilt.
set -eu
cd "$(dirname "$0")/.."
# --locked: install-time builds must reproduce the reviewed dependency
# graph, never re-resolve it.
cargo build --release --locked
mkdir -p bin
install -m 0755 target/release/dagr bin/dagr
