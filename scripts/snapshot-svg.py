#!/usr/bin/env python3
"""Render `dagr view --snapshot` ANSI output to an SVG screenshot.

The README images are generated, not drawn: pipe a snapshot in, get a
terminal-styled SVG out. Handles the subset of SGR dagr emits (38;5;N
foreground, 48;5;N background, bold, dim, resets).
Box-drawing cells become vector paths so browser font metrics cannot open
seams that a terminal emulator correctly joins.

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
BOX_DRAWING = frozenset("─│┄┌┐└┘├╭╮╯╰")
SQUARE_CORNERS = frozenset("┌┐└┘")

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


def box_path(ch, x, top, span=1):
    """Draw a box glyph on terminal-cell boundaries, with no font seams."""
    x0, x1 = x, x + CW * span
    y0, y1 = top, top + LH
    cx, cy = x + CW / 2, top + LH / 2
    radius = min(CW * 0.38, LH * 0.22)
    if ch == "─":
        return f"M{x0:.2f},{cy:.2f} H{x1:.2f}"
    if ch == "│":
        return f"M{cx:.2f},{y0:.2f} V{y1:.2f}"
    if ch == "┄":
        gap = CW * 0.12
        dash = (CW - 4 * gap) / 3
        return " ".join(
            f"M{x0 + gap + i * (dash + gap):.2f},{cy:.2f} h{dash:.2f}"
            for i in range(3)
        )
    if ch == "├":
        return f"M{cx:.2f},{y0:.2f} V{y1:.2f} M{cx:.2f},{cy:.2f} H{x1:.2f}"
    if ch == "┌":
        return f"M{cx:.2f},{y1:.2f} V{cy:.2f} H{x1:.2f}"
    if ch == "┐":
        return f"M{x0:.2f},{cy:.2f} H{cx:.2f} V{y1:.2f}"
    if ch == "└":
        return f"M{cx:.2f},{y0:.2f} V{cy:.2f} H{x1:.2f}"
    if ch == "┘":
        return f"M{x0:.2f},{cy:.2f} H{cx:.2f} V{y0:.2f}"
    if ch == "╭":
        return (
            f"M{cx:.2f},{y1:.2f} V{cy + radius:.2f} "
            f"Q{cx:.2f},{cy:.2f} {cx + radius:.2f},{cy:.2f} H{x1:.2f}"
        )
    if ch == "╮":
        return (
            f"M{x0:.2f},{cy:.2f} H{cx - radius:.2f} "
            f"Q{cx:.2f},{cy:.2f} {cx:.2f},{cy + radius:.2f} V{y1:.2f}"
        )
    if ch == "╰":
        return (
            f"M{cx:.2f},{y0:.2f} V{cy - radius:.2f} "
            f"Q{cx:.2f},{cy:.2f} {cx + radius:.2f},{cy:.2f} H{x1:.2f}"
        )
    if ch == "╯":
        return (
            f"M{x0:.2f},{cy:.2f} H{cx - radius:.2f} "
            f"Q{cx:.2f},{cy:.2f} {cx:.2f},{cy - radius:.2f} V{y0:.2f}"
        )
    raise ValueError(f"unsupported box glyph: {ch!r}")


def text_element(text, x, y, fg, bold, dim):
    n = cells(text)
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
    return f"<text {' '.join(attrs)}>{esc(text)}</text>"


def box_element(ch, x, top, fg, bold, dim, span=1):
    line_join = "miter" if ch in SQUARE_CORNERS else "round"
    attrs = [
        f'd="{box_path(ch, x, top, span)}"',
        f'stroke="{xterm256(fg) if fg is not None else FG}"',
        'fill="none"',
        f'stroke-width="{1.45 if bold else 1.1}"',
        'stroke-linecap="square"',
        f'stroke-linejoin="{line_join}"',
    ]
    if dim:
        attrs.append('stroke-opacity="0.55"')
    return f"<path {' '.join(attrs)}/>"


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
        f'viewBox="0 0 {w:.0f} {h:.0f}" preserveAspectRatio="xMinYMin meet" '
        f'font-family="{FONT}" font-size="{FS}" font-variant-ligatures="none" '
        f'text-rendering="geometricPrecision" shape-rendering="geometricPrecision">',
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
            # Terminal emulators join box-drawing glyphs across adjacent
            # cells. Browser fonts generally do not, leaving visible seams
            # in frames and graph rails. Keep ordinary text as shaped runs,
            # but draw the box glyphs dagr emits as exact cell paths.
            offset = 0
            start = 0
            chunk = ""
            index = 0
            while index < len(text):
                char = text[index]
                if char in BOX_DRAWING:
                    if chunk.strip():
                        svg.append(text_element(chunk, x + start * CW, y, fg, bold, dim))
                    chunk = ""
                    # A horizontal rail is one path, not one glyph per cell:
                    # this is both smaller and mathematically seamless.
                    span = 1
                    if char == "─":
                        while index + span < len(text) and text[index + span] == char:
                            span += 1
                    svg.append(
                        box_element(
                            char,
                            x + offset * CW,
                            PAD + row * LH,
                            fg,
                            bold,
                            dim,
                            span,
                        )
                    )
                    offset += span
                    index += span
                    start = offset
                else:
                    if not chunk:
                        start = offset
                    chunk += char
                    offset += cells(char)
                    index += 1
            if chunk.strip():
                svg.append(text_element(chunk, x + start * CW, y, fg, bold, dim))
            col += n
    svg.append("</svg>")
    with open(out_path, "w") as f:
        f.write("\n".join(svg))
    print(f"{out_path}: {ncols} cols x {len(lines)} rows", file=sys.stderr)


if __name__ == "__main__":
    main()
