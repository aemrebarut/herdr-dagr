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

# Screenshot rails are cell geometry, not font glyphs: this keeps rounded
# frames connected in browsers just as they are in a terminal emulator.
box_svg="$work/box.svg"
printf '\033[38;5;45m╭─╮ ┌─┐\n│x│ │x│\n╰─╯ └─┘\033[0m\n' \
  | python3 "$root/scripts/snapshot-svg.py" "$box_svg" >/dev/null 2>&1
grep -q 'stroke-linecap="square"' "$box_svg"
if grep -q '[╭╮╰╯┌┐└┘─│]' "$box_svg"; then
  echo "box glyph leaked into screenshot text" >&2
  exit 1
fi
for asset in "$root/assets/pane-sidecar.svg" "$root/assets/pane-cockpit.svg"; do
  grep -q 'opus5·max' "$asset"
  grep -q '38m' "$asset"
  grep -q '<path ' "$asset"
done
grep -q '  1 need eyes' "$root/assets/pane-sidecar.svg"
grep -q 'l5-rev 38m' "$root/assets/pane-sidecar.svg"
python3 - "$root/assets/pane-sidecar.svg" "$root/assets/pane-cockpit.svg" <<'PY'
import sys
import xml.etree.ElementTree as ET

for path in sys.argv[1:]:
    root = ET.parse(path).getroot()
    frame_width = float(root.attrib["viewBox"].split()[2])
    right_edges = []
    for element in root:
        tag = element.tag.rsplit("}", 1)[-1]
        if tag == "text":
            right_edges.append(float(element.attrib["x"]) + float(element.attrib["textLength"]))
        elif tag == "rect" and "x" in element.attrib:
            right_edges.append(float(element.attrib["x"]) + float(element.attrib["width"]))
    assert right_edges and max(right_edges) <= frame_width, (path, max(right_edges), frame_width)
    assert max(right_edges) >= frame_width - 32, (path, "right side is unexpectedly empty")
PY

echo "public staging parity OK"
