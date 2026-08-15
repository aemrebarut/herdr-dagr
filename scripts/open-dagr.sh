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

# Placement, from the action's argv. herdr splits only know right and
# down, so left/up open as right/down and then swap across. Placement is
# the action's job: bind the side you want in ~/.config/herdr/config.toml.
place="${1:-right}"
case "$place" in
  right) direction="right"; swap="" ;;
  down)  direction="down";  swap="" ;;
  left)  direction="right"; swap="left" ;;
  up)    direction="down";  swap="up" ;;
  *)     direction="right"; swap="" ;;
esac

# Context parsing lives in the dagr binary itself (`dagr pane-cwd`) so the
# launcher needs no interpreter. The binary is built by scripts/build.sh at
# plugin-link time and sits next to this script's parent.
dagr_bin="$(cd "$(dirname "$0")/.." && pwd)/target/release/dagr"
run_cwd=""
if [ -x "$dagr_bin" ]; then
  run_cwd=$("$dagr_bin" pane-cwd 2>/dev/null || true)
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

if [ -z "$swap" ]; then
  exec "$herdr_bin" "${args[@]}"
fi

# left/up: capture the opened pane's id from the JSON reply, then swap
# it across the anchor. No jq on purpose — first "pane_id" wins.
out=$("$herdr_bin" "${args[@]}") || exit 1
pane_id=$(printf '%s' "$out" | sed -nE 's/.*"pane_id":"([^"]+)".*/\1/p' | head -1)
if [ -n "$pane_id" ]; then
  "$herdr_bin" pane swap --pane "$pane_id" --direction "$swap" >/dev/null
fi
