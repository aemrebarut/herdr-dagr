//! Palette, glyphs, and the styled line buffer — one palette for every
//! surface, keyed by state.

#[derive(Clone, Copy, PartialEq, Default)]
pub struct Style {
    pub fg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
}

impl Style {
    pub const fn fg(c: u8) -> Self {
        Style { fg: Some(c), bold: false, dim: false }
    }
    pub const fn bold(c: u8) -> Self {
        Style { fg: Some(c), bold: true, dim: false }
    }
    pub const fn dim(c: u8) -> Self {
        Style { fg: Some(c), bold: false, dim: true }
    }
    pub const fn plain() -> Self {
        Style { fg: None, bold: false, dim: false }
    }
    pub const fn plain_dim() -> Self {
        Style { fg: None, bold: false, dim: true }
    }
}

// ── palette (256-color), one hue vocabulary for the whole surface ────
pub const DONE: u8 = 114;
pub const WORKING: u8 = 81;
pub const BLOCKED: u8 = 203;
pub const REVIEW: u8 = 221;
pub const QUEUED: u8 = 245;
pub const FAILED: u8 = 167;
pub const REJECTED: u8 = 209; // orange — a human/review sent it back
pub const EDGE: u8 = 240;
pub const TEXT: u8 = 252;
pub const MUTED: u8 = 245;
pub const ACCENT: u8 = 141; // all relational ink: » ← ▍ tags
pub const RULE: u8 = 238;
pub const GHOST: u8 = 246; // dotted-future rows
pub const EV_REPORTED: u8 = 110;
pub const EV_HEURISTIC: u8 = 180;
pub const SEL_BG: u8 = 236;

pub fn state_color(state: &str) -> u8 {
    match state {
        "done" => DONE,
        "working" => WORKING,
        "blocked" => BLOCKED,
        "review" => REVIEW,
        "queued" | "waiting" | "unassigned" => QUEUED,
        "ready" => DONE,
        "failed" => FAILED,
        "rejected" => REJECTED,
        "canceled" => MUTED,
        "settled_unverified" => EV_HEURISTIC,
        "lost" => BLOCKED,
        "needs_answer" => REVIEW,
        _ => MUTED,
    }
}

pub fn state_glyph(state: &str) -> char {
    match state {
        "done" => '●',
        // Some terminal/font stacks fall back to a mismatched face for
        // geometric symbols. ◎ is the coherent default; an explicit `*`
        // override is the portable escape hatch (auto-detecting visual font
        // metrics is not possible through a terminal protocol).
        "working" if std::env::var("DAGR_WORKING_GLYPH").as_deref() == Ok("*") => '*',
        "working" => '◎',
        "blocked" => '■',
        "review" => '◈',
        "queued" | "waiting" | "ready" | "unassigned" => '○',
        "failed" | "rejected" => '✗',
        "canceled" => '×',
        "settled_unverified" => '◌',
        "lost" => '?',
        "needs_answer" => '◈',
        _ => '·',
    }
}

/// Characters that can change terminal state or reorder visible text. This is
/// intentionally a small terminal-safety set, not a generated Unicode policy.
pub fn terminal_active(c: char) -> bool {
    matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f | 0x061c | 0x200e..=0x200f | 0x2028..=0x202e | 0x2066..=0x2069)
}

/// Escape producer text at the display boundary. Backslashes are left alone,
/// making repeated boundary calls idempotent while the source document remains
/// untouched.
pub fn terminal_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) <= 0xff && terminal_active(c) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if terminal_active(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Evidence tier → (glyph, color). The "how do we know" primitive.
pub fn evidence(tier: &str) -> (char, u8) {
    match tier {
        "verified" => ('◆', DONE),
        "reported" => ('◇', EV_REPORTED),
        "heuristic" => ('≈', EV_HEURISTIC),
        _ => ('!', BLOCKED), // asserted — or absent, which renders the same
    }
}

/// Live-state chip mark for compact dependency lists.
pub fn chip_mark(state: &str) -> char {
    match state {
        "done" => '✓',
        s => state_glyph(s),
    }
}

// ── styled line buffer: build one row as (text, style) spans ────────
pub struct Line {
    w: usize,
    ch: Vec<char>,
    st: Vec<Style>,
}

impl Line {
    pub fn new(w: usize) -> Self {
        Line { w, ch: vec![' '; w], st: vec![Style::plain(); w] }
    }

    /// Place `s` starting at visible column `x`; clips at the buffer edge.
    /// Place text at column x; returns the next free column. Cells are
    /// terminal columns: double-width chars claim two (the second holds a
    /// '\0' continuation the renderer skips), zero-width marks are dropped
    /// — producer text can be anything, geometry must survive it.
    pub fn put(&mut self, x: usize, s: &str, style: Style) -> usize {
        use unicode_width::UnicodeWidthChar;
        let mut xi = x;
        for c in terminal_safe(s).chars() {
            let cw = c.width().unwrap_or(0);
            if cw == 0 {
                continue;
            }
            if xi + cw > self.w {
                break;
            }
            self.ch[xi] = c;
            self.st[xi] = style;
            if cw == 2 {
                self.ch[xi + 1] = '\0';
                self.st[xi + 1] = style;
            }
            xi += cw;
        }
        xi
    }

    /// Emit the row as ANSI. `bg` lays a selection background under the
    /// full width; `pad` keeps trailing blank cells (for side-by-side
    /// panel joins) instead of trimming them.
    pub fn render(&self, bg: Option<u8>, pad: bool) -> String {
        let end = if bg.is_some() || pad {
            self.w
        } else {
            self.ch.iter().rposition(|c| *c != ' ').map_or(0, |p| p + 1)
        };
        let mut out = String::new();
        if let Some(b) = bg {
            out.push_str(&format!("\x1b[48;5;{b}m"));
        }
        let mut cur: Option<Style> = None;
        for i in 0..end {
            let sty = self.st[i];
            if cur != Some(sty) {
                out.push_str("\x1b[39m\x1b[22m");
                if sty.bold {
                    out.push_str("\x1b[1m");
                }
                if sty.dim {
                    out.push_str("\x1b[2m");
                }
                if let Some(c) = sty.fg {
                    out.push_str(&format!("\x1b[38;5;{c}m"));
                }
                cur = Some(sty);
            }
            if self.ch[i] != '\0' {
                out.push(self.ch[i]);
            }
        }
        out.push_str("\x1b[0m");
        out
    }
}

pub fn paint(s: &str, style: Style) -> String {
    let mut out = String::new();
    if style.bold {
        out.push_str("\x1b[1m");
    }
    if style.dim {
        out.push_str("\x1b[2m");
    }
    if let Some(c) = style.fg {
        out.push_str(&format!("\x1b[38;5;{c}m"));
    }
    out.push_str(&terminal_safe(s));
    out.push_str("\x1b[0m");
    out
}

/// Truncate to n terminal COLUMNS (not chars): wide glyphs count double,
/// combining marks count zero.
pub fn trunc(s: &str, n: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let safe = terminal_safe(s);
    let s = safe.as_str();
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= n {
        return s.to_string();
    }
    if n == 0 {
        return String::new();
    }
    let mut t = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > n - 1 {
            break;
        }
        t.push(c);
        used += cw;
    }
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_controls_become_visible_text_only_at_paint_boundaries() {
        let source = "title\n\x1b[31m\u{202e}tail".to_string();
        assert_eq!(
            terminal_safe(&source),
            "title\\n\\x1b[31m\\u{202e}tail"
        );
        assert_eq!(source.as_bytes()[5], b'\n', "source stays byte-for-byte unchanged");

        let mut line = Line::new(80);
        line.put(0, &source, Style::plain());
        let rendered = line.render(None, false);
        assert!(!rendered.contains("\x1b[31m"));
        assert!(rendered.contains("\\x1b[31m\\u{202e}"), "{rendered:?}");
    }
}
