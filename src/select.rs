//! Mouse text selection inside the pane: drag to select, release to copy.
//!
//! herdr does select-and-copy for every pane it draws — until the pane app
//! turns mouse reporting on. Then herdr forwards the events to the app and
//! skips its own selection entirely (`forward_pane_mouse_button` wins over
//! `Selection::anchor`). dagr wants clicks, so dagr owes the user the rest:
//! this module is the selection herdr can no longer do for us. Linear
//! selection over the DRAWN frame, reverse-video highlight that survives
//! the styling already in the line, and an OSC 52 write, which herdr
//! re-emits to the host clipboard (`AppEvent::ClipboardWrite`).
//!
//! Everything here works on painted lines — the strings we actually put on
//! screen — so what you copy is what you saw, including the help overlay
//! and a confirm gate's argv.

use unicode_width::UnicodeWidthChar;

/// A press-drag in frame coordinates: `(line index in the composed frame,
/// terminal column)`. Frame lines, not screen rows: the viewport can move
/// under a drag and the anchor must not drift with it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sel {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
    /// A press that never moved is a click, not a selection. This is the
    /// whole "not every click copies" rule.
    pub dragged: bool,
}

impl Sel {
    pub fn new(line: usize, col: usize) -> Self {
        Sel { anchor: (line, col), cursor: (line, col), dragged: false }
    }

    pub fn to(&mut self, line: usize, col: usize) {
        if (line, col) != self.anchor {
            self.dragged = true;
        }
        self.cursor = (line, col);
    }

    /// Ordered ends: dragging up or leftward selects the same text as
    /// dragging down or rightward.
    pub fn span(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Selected column range on `line`, end-exclusive, clamped to `width`.
    /// `None` when the line is outside the selection. The cell under the
    /// cursor is included — you selected it, you get it.
    pub fn cols_on(&self, line: usize, width: usize) -> Option<(usize, usize)> {
        if !self.dragged {
            return None;
        }
        let ((l0, c0), (l1, c1)) = self.span();
        if line < l0 || line > l1 {
            return None;
        }
        let start = if line == l0 { c0 } else { 0 };
        let end = if line == l1 { (c1 + 1).min(width) } else { width };
        (start < end).then_some((start, end))
    }
}

/// One piece of a painted line: a printable cell at a display column, or
/// an escape sequence that occupies no columns at all.
enum Piece<'a> {
    Cell(usize, char),
    Esc(&'a str),
}

/// Walk a painted line piece by piece. The one place that knows our lines
/// are plain text sprinkled with CSI SGR codes.
fn walk(line: &str, mut f: impl FnMut(Piece)) {
    let mut chars = line.char_indices().peekable();
    let mut col = 0;
    while let Some((i, c)) = chars.next() {
        if c == '\x1b' {
            let mut end = i + c.len_utf8();
            // CSI: everything up to and including the final byte @-~
            while let Some(&(j, n)) = chars.peek() {
                chars.next();
                end = j + n.len_utf8();
                if matches!(n, '@'..='~') && n != '[' {
                    break;
                }
            }
            f(Piece::Esc(&line[i..end]));
            continue;
        }
        let w = c.width().unwrap_or(0);
        if w == 0 {
            continue;
        }
        f(Piece::Cell(col, c));
        col += w;
    }
}

/// Plain text of a painted line: styling stripped, geometry kept. The
/// copy path slices instead, so this is the tests' reading glasses.
#[cfg(test)]
pub fn plain(line: &str) -> String {
    let mut out = String::new();
    walk(line, |p| {
        if let Piece::Cell(_, c) = p {
            out.push(c);
        }
    });
    out
}

/// The `[c0, c1)` display-column slice of a painted line, as plain text.
/// A wide glyph belongs to the slice that holds its first column.
pub fn slice(line: &str, c0: usize, c1: usize) -> String {
    let mut out = String::new();
    walk(line, |p| {
        if let Piece::Cell(col, c) = p {
            if col >= c0 && col < c1 {
                out.push(c);
            }
        }
    });
    out
}

/// Reverse-video the `[c0, c1)` span of a painted line, keeping the colors
/// already in it. The line carries its own SGR resets, so reverse is
/// re-asserted after every escape inside the span — otherwise the first
/// `\x1b[0m` in the middle of a row would drop the highlight.
/// Short lines are padded with spaces to `c1`, so a block drag over ragged
/// rows reads as one rectangle instead of a comb.
pub fn highlight(line: &str, c0: usize, c1: usize) -> String {
    if c0 >= c1 {
        return line.to_string();
    }
    let mut out = String::new();
    let mut inside = false;
    let mut last = 0usize;
    walk(line, |p| match p {
        Piece::Cell(col, c) => {
            let want = col >= c0 && col < c1;
            if want && !inside {
                out.push_str("\x1b[7m");
            } else if !want && inside {
                out.push_str("\x1b[27m");
            }
            inside = want;
            out.push(c);
            last = col + c.width().unwrap_or(1);
        }
        Piece::Esc(seq) => {
            out.push_str(seq);
            // the line's own reset just cleared our reverse
            if inside {
                out.push_str("\x1b[7m");
            }
        }
    });
    if last < c1 {
        if !inside {
            out.push_str("\x1b[7m");
            inside = true;
        }
        out.push_str(&" ".repeat(c1 - last.max(c0)));
    }
    if inside {
        out.push_str("\x1b[27m");
    }
    out.push_str("\x1b[0m");
    out
}

/// OSC 52 clipboard write. herdr parses this out of the pane's output and
/// re-emits it through its own clipboard writer, so the copy lands on the
/// host clipboard even when the session is remote.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Standard base64. Twenty lines instead of a dependency: the copy path
/// must not be the reason an install-time `cargo build` needs the network.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{paint, Style};

    #[test]
    fn a_press_that_never_moved_selects_nothing() {
        let s = Sel::new(3, 10);
        assert!(!s.dragged);
        assert_eq!(s.cols_on(3, 80), None, "a click must not copy");
    }

    #[test]
    fn selection_geometry_is_linear_and_direction_agnostic() {
        let mut down = Sel::new(2, 5);
        down.to(4, 9);
        let mut up = Sel::new(4, 9);
        up.to(2, 5);
        for s in [down, up] {
            assert_eq!(s.cols_on(1, 40), None);
            assert_eq!(s.cols_on(2, 40), Some((5, 40)), "first line runs to EOL");
            assert_eq!(s.cols_on(3, 40), Some((0, 40)), "middle line is whole");
            assert_eq!(s.cols_on(4, 40), Some((0, 10)), "last line includes the cell");
            assert_eq!(s.cols_on(5, 40), None);
        }
    }

    #[test]
    fn plain_strips_styling_and_slice_counts_columns() {
        let line = format!("{}{}", paint("done ", Style::bold(114)), paint("L1 impl", Style::plain()));
        assert_eq!(plain(&line), "done L1 impl");
        assert_eq!(slice(&line, 5, 12), "L1 impl");
        assert_eq!(slice(&line, 0, 4), "done");
    }

    #[test]
    fn slice_treats_wide_glyphs_as_two_columns() {
        let line = paint("◎ 日本 x", Style::plain());
        assert_eq!(plain(&line), "◎ 日本 x");
        // ◎ is one column, then a space, then two two-column glyphs
        assert_eq!(slice(&line, 2, 6), "日本");
        assert_eq!(slice(&line, 7, 8), "x");
    }

    #[test]
    fn highlight_wraps_the_span_and_survives_the_lines_own_resets() {
        let line = format!("{}{}", paint("ab", Style::bold(114)), paint("cd", Style::fg(81)));
        let out = highlight(&line, 1, 3);
        assert_eq!(plain(&out), "abcd", "text must not change");
        assert!(out.contains("\x1b[7m"), "reverse video is the highlight");
        assert!(out.contains("\x1b[27m"), "and it has to end");
        // the paint() reset between the two spans lands inside the
        // selection, so reverse must be re-asserted after it
        let after_reset = out.split("\x1b[0m").nth(1).unwrap_or("");
        assert!(after_reset.contains("\x1b[7m"), "reset must not eat the highlight");
    }

    #[test]
    fn highlight_pads_short_lines_so_a_block_drag_is_a_rectangle() {
        let out = highlight(&paint("hi", Style::plain()), 0, 6);
        assert_eq!(plain(&out), "hi    ");
    }

    #[test]
    fn highlight_of_an_empty_span_is_the_line_itself() {
        let line = paint("untouched", Style::plain());
        assert_eq!(highlight(&line, 4, 4), line);
    }

    #[test]
    fn osc52_carries_base64_of_the_selection() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64("héllo ✓".as_bytes()), "aMOpbGxvIOKckw==");
    }
}
