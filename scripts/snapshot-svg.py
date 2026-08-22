#!/usr/bin/env python3
"""Render `dagr view --snapshot` ANSI output to an SVG screenshot.

The README images are generated, not drawn: pipe a snapshot in, get a
terminal-styled SVG out. Handles the subset of SGR dagr emits (38;5;N
foreground, 48;5;N background, bold, dim, resets).

Usage: dagr view run.json --snapshot --compact --width 150 | snapshot-svg.py out.svg
"""

import re
import sys
import unicodedata

CW = 8.4          # cell width (px) — enforced per-run via textLength
LH = 19           # line height
FS = 14           # font size
PAD = 16          # frame padding
BG = "#101318"    # terminal background
FG = "#e6e6e6"    # default foreground (SGR 39)
FONT = "'SF Mono','Cascadia Code','JetBrains Mono',Menlo,Consolas,monospace"

SGR = re.compile(r"\x1b\[([0-9;]*)m")


def xterm256(n):
    basic = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc",
        "#11a8cd", "#e5e5e5", "#666666", "#f14c4c", "#23d18b", "#f5f543",
        "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ]
    if n < 16:
        return basic[n]
    if n < 232:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        r, g, b = lv[n // 36], lv[(n // 6) % 6], lv[n % 6]
    else:
        r = g = b = 8 + 10 * (n - 232)
    return f"#{r:02x}{g:02x}{b:02x}"


def cells(s):
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)


def parse_line(line):
    """Split one ANSI line into (text, fg, bg, bold, dim) runs."""
    runs, pos = [], 0
    fg = bg = None
    bold = dim = False
    for m in SGR.finditer(line):
        if m.start() > pos:
            runs.append((line[pos : m.start()], fg, bg, bold, dim))
        pos = m.end()
        codes = [int(c or 0) for c in m.group(1).split(";")]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                fg = bg = None
                bold = dim = False
            elif c == 1:
                bold = True
            elif c == 2:
                dim = True
            elif c == 22:
                bold = dim = False
            elif c == 39:
                fg = None
            elif c == 49:
                bg = None
            elif c in (38, 48) and i + 2 < len(codes) and codes[i + 1] == 5:
                if c == 38:
                    fg = codes[i + 2]
                else:
                    bg = codes[i + 2]
                i += 2
            i += 1
    if pos < len(line):
        runs.append((line[pos:], fg, bg, bold, dim))
    return runs


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def main():
    out_path = sys.argv[1]
    lines = sys.stdin.read().split("\n")
    while lines and not lines[-1].strip("\x1b[0m \t"):
        lines.pop()
    ncols = max((cells(SGR.sub("", l)) for l in lines), default=80)
    w = ncols * CW + 2 * PAD
    h = len(lines) * LH + 2 * PAD

    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" '
        f'viewBox="0 0 {w:.0f} {h:.0f}" font-family="{FONT}" font-size="{FS}">',
        f'<rect width="100%" height="100%" rx="8" fill="{BG}"/>',
    ]
    for row, line in enumerate(lines):
        y = PAD + row * LH + FS
        col = 0
        for text, fg, bgc, bold, dim in parse_line(line):
            n = cells(text)
            if not n:
                continue
            x = PAD + col * CW
            if bgc is not None:
                svg.append(
                    f'<rect x="{x:.1f}" y="{PAD + row * LH:.1f}" '
                    f'width="{n * CW:.1f}" height="{LH}" fill="{xterm256(bgc)}"/>'
                )
            if text.strip():
                attrs = [
                    f'x="{x:.1f}"',
                    f'y="{y:.1f}"',
                    f'textLength="{n * CW:.1f}"',
                    'lengthAdjust="spacingAndGlyphs"',
                    f'fill="{xterm256(fg) if fg is not None else FG}"',
                    'xml:space="preserve"',
                ]
                if bold:
                    attrs.append('font-weight="bold"')
                if dim:
                    attrs.append('fill-opacity="0.55"')
                svg.append(f"<text {' '.join(attrs)}>{esc(text)}</text>")
            col += n
    svg.append("</svg>")
    with open(out_path, "w") as f:
        f.write("\n".join(svg))
    print(f"{out_path}: {ncols} cols x {len(lines)} rows", file=sys.stderr)


if __name__ == "__main__":
    main()
