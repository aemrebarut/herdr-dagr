#!/usr/bin/env bash
# Launcher for the dagr pane (herdr [[actions]] open-dagr). herdr actions
# run a command — there is no declarative "open this pane" field — so this
# shells back into the herdr CLI ($HERDR_BIN_PATH is injected by herdr;
# fall back to `herdr` on PATH) and opens the [[panes]] entrypoint as a
# split beside the current work.
#
# The pane's own context is computed at pane-launch time, AFTER focus has
# already moved — so the run-file directory is resolved HERE, from the
# action's context (where the user's pane is unambiguously the focused
# one), and passed down with --cwd instead of being inferred inside.
set -uo pipefail

herdr_bin="${HERDR_BIN_PATH:-herdr}"
plugin_id="${HERDR_PLUGIN_ID:-herdr-dagr}"

# Split direction, from the action's argv. herdr accepts right | down.
direction="${1:-right}"
case "$direction" in right|down) ;; *) direction="right" ;; esac

# Context parsing lives in the dagr binary itself (`dagr pane-cwd`) so the
# launcher needs no interpreter. The binary is built by scripts/build.sh at
# plugin-link time and sits next to this script's parent.
dagr_bin="$(cd "$(dirname "$0")/.." && pwd)/target/release/dagr"
run_cwd=""
anchor=""
if [ -x "$dagr_bin" ]; then
  run_cwd=$("$dagr_bin" pane-cwd 2>/dev/null || true)
  # the pane this action was invoked from — the viewer's H/J/K/L
  # placement keys split against it
  anchor=$("$dagr_bin" pane-anchor 2>/dev/null || true)
fi

args=(plugin pane open
  --plugin "$plugin_id"
  --entrypoint dagr
  --placement split
  --direction "$direction"
  --focus)
if [ -n "$run_cwd" ]; then
  args+=(--cwd "$run_cwd")
fi
if [ -n "$anchor" ]; then
  args+=(--env "DAGR_ANCHOR_PANE=$anchor")
fi

exec "$herdr_bin" "${args[@]}"
