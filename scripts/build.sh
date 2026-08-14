#!/bin/sh
# Install-time build step (herdr [[build]]): produce target/release/dagr.
set -eu
cd "$(dirname "$0")/.."
# --locked: install-time builds must reproduce the reviewed dependency
# graph, never re-resolve it.
cargo build --release --locked
