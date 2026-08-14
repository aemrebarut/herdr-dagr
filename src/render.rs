//! Rows → ANSI frames. Two layouts, both established terminal grammars:
//! `sidecar` (≥ ~110 cols: trace left, attention queue + focus card right —
//! lazygit's grammar) folding to `cockpit` (full-width trace, detail docked
//! below — tig's grammar). Renderers are pure functions of (state, width).

use crate::contract::{Attempt, Doc, Task};
use crate::model::{parse_min, Row, Scene, Seg};
use crate::style::{self, paint, trunc, Line, Style};
use unicode_width::UnicodeWidthStr;

/// Sidecar needs its left panel to keep the full column grammar (≥96
/// cols beside the 48-col rail + gutter); folding earlier put the trace
/// through compact rows while a full-width cockpit would have been richer
/// (F5's inversion band). 146 = 96 + CARD_W + 2.
pub const FOLD_WIDTH: usize = 146;
const CARD_W: usize = 48;

/// Put segments left-to-right, hard-stopped at `limit` columns so a long
/// tail can never bleed into the next column (F4).
fn seg_put(line: &mut Line, mut x: usize, segs: &[Seg], limit: usize) -> usize {
    for Seg(s, st) in segs {
        if x >= limit {
            break;
        }
        x = line.put(x, &trunc(s, limit - x), *st);
    }
    x
}

/// One full-grammar trace row, responsive: model/status/agent columns
/// hang off the right edge.
fn full_row(row: &Row, w: usize, selected: bool) -> String {
    let model_x = w.saturating_sub(44);
    let st_x = w.saturating_sub(31);
    let ag_x = w.saturating_sub(21);
    let mut l = Line::new(w);
    if row.lit {
        l.put(0, "▍", Style::bold(style::ACCENT));
    }
    let rail_style = if row.dotted { Style::fg(style::GHOST) } else { Style::fg(style::EDGE) };
    let mut x = l.put(1, &row.rail, rail_style);
    if row.reentry && !row.rail.is_empty() {
        // the branch lead's `─` becomes ↩ — the loop lives in the rail
        l.put(x - 1, "↩", Style::bold(style::REJECTED));
    }
    x = l.put(x, &row.glyph.to_string(), Style { fg: Some(row.glyph_color), bold: row.hot, dim: false });
    let name_style = if row.dotted && row.glyph != '»' {
        Style::fg(style::GHOST)
    } else if row.glyph == '»' {
        Style::fg(style::ACCENT)
    } else {
        Style { fg: Some(row.glyph_color), bold: row.hot, dim: false }
    };
    x = l.put(x, &format!(" {}", row.name), name_style);
    x += 2;
    let title_room = model_x.saturating_sub(x + 1);
    let title_style = if row.dotted {
        Style::dim(style::GHOST)
    } else if row.title_dim {
        Style::plain_dim()
    } else {
        Style::fg(style::TEXT)
    };
    x = l.put(x, &trunc(&row.title, title_room), title_style);
    if !row.chips.is_empty() {
        x = seg_put(&mut l, x + 2, &row.chips, model_x.saturating_sub(1));
    }
    if let Some(tag) = &row.tag {
        // selection ink stays in the title column — drop it rather than
        // run under the right-hung model/status columns
        if x + 2 + tag.width() < model_x {
            l.put(x + 2, tag, Style::bold(style::ACCENT));
        }
    }
    l.put(model_x, &trunc(&row.model, 12), Style::dim(style::MUTED));
    seg_put(&mut l, st_x, &row.status, ag_x.saturating_sub(1));
    l.put(ag_x, &trunc(&row.agent, w - ag_x), Style::dim(style::MUTED));
    l.render(if selected { Some(style::SEL_BG) } else { None }, true)
}

/// Narrow two-column row for the sidecar's left panel.
fn compact_row(row: &Row, w: usize, selected: bool) -> String {
    let mut l = Line::new(w);
    if row.lit {
        l.put(0, "▍", Style::bold(style::ACCENT));
    }
    let rail_style = if row.dotted { Style::fg(style::GHOST) } else { Style::fg(style::EDGE) };
    let mut x = l.put(1, &row.rail, rail_style);
    if row.reentry && !row.rail.is_empty() {
        l.put(x - 1, "↩", Style::bold(style::REJECTED));
    }
    x = l.put(x, &row.glyph.to_string(), Style { fg: Some(row.glyph_color), bold: row.hot, dim: false });
    let name_style = if row.dotted && row.glyph != '»' {
        Style::fg(style::GHOST)
    } else if row.glyph == '»' {
        Style::fg(style::ACCENT)
    } else {
        Style { fg: Some(row.glyph_color), bold: row.hot, dim: false }
    };
    x = l.put(x, &format!(" {}", row.name), name_style);
    // the right-aligned status tail is sized FIRST — the title gets what
    // is left of it, so a truncated title can never run under the state
    // word (the "atteBLOCKED" bug the width matrix caught)
    let mut tail: Vec<(String, Style)> =
        row.status.iter().map(|Seg(s, st)| (s.clone(), *st)).collect();
    let tw = |t: &[(String, Style)]| t.iter().map(|(s, _)| s.width()).sum::<usize>();
    if !tail.is_empty() {
        let avail = w.saturating_sub(2).min(18);
        // trailing segs drop first; the leading state word drops last (F6)
        while tail.len() > 1 && tw(&tail) > avail {
            tail.pop();
        }
        if tw(&tail) > avail {
            if let Some((s0, _)) = tail.first_mut() {
                *s0 = trunc(s0.trim(), avail);
            }
        }
    }
    let status_x = w.saturating_sub(tw(&tail) + 1);
    // short title: drop the "kind: " prefix, truncate hard
    let room = status_x.saturating_sub(x + 2);
    // gate rows: the ← fan-in chips ARE the information — they replace the
    // title when space is tight. Annotation chips (⇠ ink) stay secondary:
    // compact rows keep the title and drop them.
    let fanin = row.chips.first().map(|Seg(s, _)| s.starts_with('←')).unwrap_or(false);
    let left_end = if fanin {
        let mut cx = x + 1;
        for Seg(s, st) in &row.chips {
            if cx + s.width() > x + 1 + room {
                break;
            }
            cx = l.put(cx, s, *st);
        }
        cx
    } else {
        let short = row.title.split_once(": ").map(|(_, t)| t).unwrap_or(&row.title);
        let title_style = if row.dotted {
            Style::dim(style::GHOST)
        } else if row.title_dim {
            Style::plain_dim()
        } else {
            Style::fg(style::TEXT)
        };
        l.put(x + 1, &trunc(short, room), title_style)
    };
    // right-anchored annotations, strictly leftover space — never at the
    // title's expense. The » gate tag (selection ink) outranks the model
    // chip; each appears only with a 2-col gap on both sides, and both
    // drop before touching the left content.
    let mut anchor = status_x;
    if let Some(tag) = &row.tag {
        let px = anchor.saturating_sub(tag.width() + 2);
        if px >= left_end + 2 {
            l.put(px, tag, Style::bold(style::ACCENT));
            anchor = px;
        }
    }
    if !row.model.is_empty() {
        let mx = anchor.saturating_sub(row.model.width() + 2);
        if mx >= left_end + 2 {
            l.put(mx, &row.model, Style::dim(style::MUTED));
        }
    }
    // right edge: the pre-sized status tail, right-aligned — the evidence
    // glyph and waits-target survive compaction (F6).
    if !tail.is_empty() {
        let mut sx = status_x;
        for (s, st) in &tail {
            sx = l.put(sx, s, *st);
        }
    }
    l.render(if selected { Some(style::SEL_BG) } else { None }, true)
}

// ── focus card ──────────────────────────────────────────────────────

fn find_selection<'a>(doc: &'a Doc, key: &str) -> Option<(&'a Task, Option<&'a Attempt>)> {
    let tasks = doc.tasks.as_deref()?;
    for t in tasks {
        for a in &t.attempts {
            if a.id.as_deref() == Some(key) {
                return Some((t, Some(a)));
            }
        }
    }
    tasks
        .iter()
        .find(|t| t.id.as_deref() == Some(key))
        .map(|t| {
            let last = t
                .attempts
                .iter()
                .max_by_key(|a| a.n.unwrap_or(0));
            (t, last)
        })
}

fn actions_for(state: &str) -> &'static str {
    match state {
        "blocked" => "[u]nblock  [a]nswer  [enter] focus pane",
        "review" => "[o]k approve  [x] reject  [enter] open diff",
        "working" => "[enter] focus pane  [i]nterrupt  [p]eek",
        "lost" => "[enter] focus pane  [r]espawn?",
        _ => "[enter] focus pane",
    }
}

pub fn focus_card(
    doc: &Doc,
    key: &str,
    cw: usize,
    hints: Option<&crate::herdr::Hints>,
) -> Vec<String> {
    // below ~8 cols no card grammar survives; claim the space, draw nothing
    if cw < 8 {
        return vec![paint("…", Style::dim(style::MUTED))];
    }
    let Some((task, attempt)) = find_selection(doc, key) else {
        return vec![paint("  (nothing selected)", Style::dim(style::MUTED))];
    };
    let state = attempt
        .and_then(|a| a.state.as_deref())
        .or(task.state.as_deref())
        .unwrap_or("queued");
    let col = style::state_color(state);
    let inner = cw.saturating_sub(4);
    let now = doc.generated_at.as_deref().and_then(parse_min);
    let mut body: Vec<(String, Style)> = Vec::new();

    body.push((task.title.clone().unwrap_or_default(), Style::fg(style::TEXT)));

    // meta line
    let mut meta = Vec::new();
    if let Some(a) = attempt {
        if let Some(actor) = a.actor.as_deref() {
            meta.push(actor.to_string());
        }
        if let Some(m) = a.model.as_deref() {
            meta.push(m.to_string());
        }
        let s = a.started_at.as_deref().and_then(parse_min);
        let e = a.ended_at.as_deref().and_then(parse_min);
        match (s, e, now) {
            (Some(s), Some(e), _) if e >= s => meta.push(format!("{}m", e - s)),
            (Some(s), None, Some(n)) if n >= s => meta.push(format!("{}m…", n - s)),
            _ => {}
        }
    } else if let Some(o) = task.owner.as_deref() {
        meta.push(o.to_string());
    }
    if !meta.is_empty() {
        body.push((meta.join(" · "), Style::dim(style::MUTED)));
    }

    // liveness — the anti-silent-stall lines
    if let Some(l) = attempt.and_then(|a| a.liveness.as_ref()) {
        let mut parts = Vec::new();
        if let Some(ack) = l.prompt_acknowledged {
            parts.push(format!("prompt {}", if ack { "ack ✓" } else { "NOT ACKED" }));
        }
        if let (Some(n), Some(lo)) = (now, l.last_output_at.as_deref().and_then(parse_min)) {
            parts.push(format!("silent {}m", (n - lo).max(0)));
        }
        if let Some(q) = l.queued_input {
            if q > 0 {
                parts.push(format!("⚠ {q} queued input"));
            }
        }
        if !parts.is_empty() {
            let hotness = parts.iter().any(|p| p.contains("NOT") || p.contains('⚠'));
            body.push((
                parts.join(" · "),
                if hotness { Style::bold(style::BLOCKED) } else { Style::dim(style::WORKING) },
            ));
        }
    }
    // locator + herdr hint: where the attempt lives, and whether that pane
    // still exists (per the live link) — the dead-locator line
    if let Some(l) = attempt.and_then(|a| a.locator.as_ref()) {
        let loc = l.pane.as_deref().or(l.agent.as_deref());
        if let Some(loc) = loc {
            let kind = if l.pane.is_some() { "pane" } else { "agent" };
            match l.pane.as_deref().and_then(|p| hints.map(|h| h.pane(p))) {
                Some(Some(Some(st))) => body.push((
                    format!("{kind} {loc} · herdr: {st}"),
                    Style::dim(style::WORKING),
                )),
                Some(Some(None)) => body.push((
                    format!("{kind} {loc} · GONE"),
                    Style::bold(style::BLOCKED),
                )),
                // link down or agent-locator: name it, claim nothing
                _ => body.push((format!("{kind} {loc}"), Style::dim(style::MUTED))),
            }
        }
    }
    if let Some(p) = attempt.and_then(|a| a.progress.as_ref()) {
        let mut s = String::from("progress ");
        if let (Some(d), Some(t)) = (p.done, p.total) {
            s.push_str(&format!("{d}/{t}"));
        }
        if let Some(n) = p.note.as_deref() {
            s.push_str(&format!(" · {n}"));
        }
        body.push((s, Style::fg(style::WORKING)));
    }

    // cause chain: why does this attempt exist
    let mut chain: Vec<String> = Vec::new();
    let mut cur = attempt;
    let mut hops = 0;
    while let Some(a) = cur {
        hops += 1;
        if hops > 6 {
            break;
        }
        let Some(c) = &a.cause else { break };
        let ct = c.cause_type.as_deref().unwrap_or("");
        if ct == "initial" {
            break;
        }
        let mut line = format!("↩ {}", c.reference.as_deref().unwrap_or("?"));
        match ct {
            "sent_back" => line.push_str(&format!(
                " sent back by {}{}",
                c.by.as_deref().unwrap_or("?"),
                c.reason.as_deref().map(|r| format!(" \"{r}\"")).unwrap_or_default()
            )),
            "gate_failed" => line.push_str(" gate failed"),
            "followup" => line.push_str(&format!(
                " → follow-up{}",
                c.reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()
            )),
            other => line.push_str(&format!(" ({other})")),
        }
        chain.push(line);
        cur = c
            .reference
            .as_deref()
            .and_then(|r| {
                doc.tasks
                    .as_deref()?
                    .iter()
                    .flat_map(|t| t.attempts.iter())
                    .find(|x| x.id.as_deref() == Some(r))
            });
    }
    for line in chain {
        body.push((line, Style::fg(style::REJECTED)));
    }

    // outcome / evidence
    if let Some(o) = attempt.and_then(|a| a.outcome.as_ref()) {
        let (g, c) = style::evidence(o.evidence.as_deref().unwrap_or("asserted"));
        let mut s = format!("{g} {}", o.evidence.as_deref().unwrap_or("asserted"));
        if let Some(r) = o.receipt.as_deref() {
            s.push_str(&format!(" · {r}"));
        }
        if let Some(r) = o.reason.as_deref() {
            s.push_str(&format!(" · {r}"));
        }
        body.push((s, Style::fg(c)));
    }

    // deps with live state
    if !task.deps.is_empty() {
        let tasks = doc.tasks.as_deref().unwrap_or(&[]);
        let mut s = String::from("deps ");
        for d in &task.deps {
            let dst = tasks
                .iter()
                .find(|t| t.id.as_deref() == Some(d.as_str()))
                .and_then(|t| t.state.as_deref())
                .unwrap_or("?");
            s.push_str(&format!("{d}{} ", style::chip_mark(dst)));
        }
        body.push((s, Style::dim(style::MUTED)));
    }

    // declared futures (with rule provenance)
    if let Some(p) = &task.policy {
        for f in &p.futures {
            let cond = match (f.on.as_deref(), f.streak.unwrap_or(1)) {
                (Some("pass"), _) => "if ✓".to_string(),
                (_, _) => format!("if {}", crate::model::streak_marks(f.streak)),
            };
            let target = f
                .reference
                .clone()
                .or_else(|| f.node.as_ref().and_then(|n| n.id.clone()))
                .unwrap_or_default();
            let mut s = format!("{cond} → {target}");
            if let Some(src) = f.source.as_deref() {
                s.push_str(&format!("  ({src})"));
            }
            body.push((s, Style::dim(style::GHOST)));
        }
    }

    // provenance tail: recent events touching this task
    let tid = task.id.as_deref().unwrap_or("");
    let recent: Vec<&crate::contract::Event> = doc
        .events
        .iter()
        .filter(|e| {
            e.task.as_deref() == Some(tid)
                || e.attempt
                    .as_deref()
                    .map(|a| task.attempts.iter().any(|x| x.id.as_deref() == Some(a)))
                    .unwrap_or(false)
        })
        .collect();
    for e in recent.iter().rev().take(3).rev() {
        let at = e.at.as_deref().map(|t| t.get(11..16).unwrap_or(t)).unwrap_or("--:--");
        let what = e.detail.as_deref().or(e.event_type.as_deref()).unwrap_or("");
        body.push((format!("{at} {what}"), Style::dim(style::MUTED)));
    }

    body.push((actions_for(state).to_string(), Style::fg(style::ACCENT)));

    // frame it
    let disp = attempt.and_then(|a| a.id.clone()).unwrap_or_else(|| tid.to_string());
    let label = match attempt.and_then(|a| a.n) {
        Some(n) => format!("─ {disp} · attempt {n} · {} ", state.to_uppercase()),
        None => format!("─ {disp} · {} ", state.to_uppercase()),
    };
    let mut lines = Vec::new();
    let label = trunc(&label, cw.saturating_sub(2));
    let dash_n = cw.saturating_sub(2 + label.width());
    lines.push(paint(&format!("┌{label}{}┐", "─".repeat(dash_n)), Style::bold(col)));
    for (txt, st) in body {
        let mut l = Line::new(cw);
        l.put(0, "│ ", Style::fg(col));
        l.put(2, &trunc(&txt, inner), st);
        l.put(cw.saturating_sub(2), " │", Style::fg(col));
        lines.push(l.render(None, true));
    }
    lines.push(paint(&format!("└{}┘", "─".repeat(cw.saturating_sub(2))), Style::fg(col)));
    lines
}

// ── attention queue panel ───────────────────────────────────────────

pub fn queue_panel(scene: &Scene, qw: usize) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(
        paint(" attention", Style::bold(style::ACCENT))
            + &paint(&format!("  {} need eyes", scene.queue.len()), Style::dim(style::MUTED)),
    );
    for item in &scene.queue {
        let mut l = Line::new(qw);
        // the selection is an attempt/future key; the queue thinks in
        // tasks — compare against the normalized owner (gpt F18)
        let sel = scene.selected_task.as_deref() == Some(item.task_id.as_str());
        l.put(0, if sel { "▸" } else { " " }, Style::bold(style::ACCENT));
        let col = style::state_color(&item.state);
        let mut x = l.put(2, &style::state_glyph(&item.state).to_string(), Style::bold(col));
        x = l.put(x, &format!(" {}", item.task_id), Style::bold(col));
        let short = item.label.split_once(": ").map(|(_, t)| t).unwrap_or(&item.label);
        let room = qw.saturating_sub(x + 2 + 14);
        l.put(x + 1, &trunc(short, room), Style::fg(style::TEXT));
        let right = format!("{} {}m", item.who, item.minutes);
        let rx = qw.saturating_sub(right.width() + 1);
        l.put(rx, &right, Style::dim(style::MUTED));
        lines.push(l.render(None, true));
    }
    if scene.queue.is_empty() {
        lines.push(paint("  (all quiet)", Style::dim(style::MUTED)));
    }
    lines
}

// ── frame composition ───────────────────────────────────────────────

pub struct FrameInput<'a> {
    pub doc: &'a Doc,
    pub scene: &'a Scene,
    pub selected: Option<&'a str>,
    pub banner: Option<String>,
    pub flash: Option<String>,
    pub stale_min: Option<i64>,
    pub watching: bool,
    /// herdr liveness hints (M2); `None` outside herdr or in snapshots.
    pub herdr: Option<&'a crate::herdr::Hints>,
    /// Modal action prompt (M4): the text-input / confirm-gate line.
    pub prompt: Option<String>,
}

/// A composed frame plus the output line carrying the selection, so the
/// interactive viewport can keep it on screen (scroll-follow).
pub struct Frame {
    pub lines: Vec<String>,
    pub sel_line: Option<usize>,
    /// First line of the modal prompt block, when one was composed. The
    /// confirm gate only counts as shown if these lines were actually
    /// inside the viewport on the last draw.
    pub prompt_line: Option<usize>,
}

pub fn compose(input: &FrameInput, w: usize) -> Frame {
    let scene = input.scene;
    let sel_row = input
        .selected
        .and_then(|s| scene.rows.iter().position(|r| r.key == s));
    let mut sel_line: Option<usize> = None;
    let mut out = Vec::new();

    // header — title/meta across the full width
    {
        let mut l = Line::new(w);
        let limit = w.saturating_sub(1);
        let mut x = l.put(1, &trunc(&scene.run_title, limit.saturating_sub(1)), Style::bold(style::TEXT));
        if x + 3 < limit {
            x = l.put(x + 3, &trunc(&scene.run_meta, limit - x - 3), Style::dim(style::MUTED));
        }
        if let Some(s) = input.stale_min {
            if s >= 3 && x + 3 < limit {
                l.put(x + 3, &trunc(&format!("⚠ data {s}m old"), limit - x - 3), Style::bold(style::REVIEW));
            }
        }
        out.push(l.render(None, false));
    }
    if let Some(b) = &input.banner {
        // banners carry file paths and error text of any length; clamp to
        // the frame instead of wrapping the terminal (F18)
        let mut l = Line::new(w);
        l.put(1, &format!("⚠ {b}"), Style::bold(style::BLOCKED));
        out.push(l.render(None, false));
    }
    out.push(String::new()); // spacer

    let sel = input.selected;
    if w >= FOLD_WIDTH {
        // sidecar: trace left · queue + card right
        let right_w = CARD_W;
        let left_w = w - right_w - 2;
        let mut right: Vec<String> = queue_panel(scene, right_w);
        right.push(String::new());
        if let Some(k) = sel {
            right.extend(focus_card(input.doc, k, right_w, input.herdr));
        }
        let left: Vec<String> = scene
            .rows
            .iter()
            .map(|r| {
                if left_w >= 96 {
                    full_row(r, left_w, sel == Some(r.key.as_str()))
                } else {
                    compact_row(r, left_w, sel == Some(r.key.as_str()))
                }
            })
            .collect();
        let rows_n = left.len().max(right.len());
        let base = out.len();
        if let Some(i) = sel_row {
            sel_line = Some(base + i);
        }
        for i in 0..rows_n {
            let l = left.get(i).cloned().unwrap_or_else(|| {
                Line::new(left_w).render(None, true)
            });
            let r = right.get(i).cloned().unwrap_or_default();
            out.push(format!("{l}  {r}"));
        }
    } else {
        // cockpit: full-width trace, card docked below; below ~96 cols the
        // full column layout self-destructs, so rows go compact
        let base = out.len();
        if let Some(i) = sel_row {
            sel_line = Some(base + i);
        }
        for r in &scene.rows {
            let line = if w >= 96 {
                full_row(r, w, sel == Some(r.key.as_str()))
            } else {
                compact_row(r, w, sel == Some(r.key.as_str()))
            };
            out.push(line);
        }
        out.push(String::new());
        if let Some(k) = sel {
            out.extend(focus_card(input.doc, k, w.min(96), input.herdr));
        }
    }

    // footer: rule + (modal prompt |) key hints
    out.push(paint(&format!(" {}", "─".repeat(w.saturating_sub(2))), Style::dim(style::RULE)));
    let mut prompt_line = None;
    if let Some(p) = &input.prompt {
        prompt_line = Some(out.len());
        // The confirm gate's whole point is that the human inspects the
        // EXACT argv — so the prompt wraps across as many lines as it
        // needs; clipping could hide a trailing argument.
        use unicode_width::UnicodeWidthChar;
        let avail = w.saturating_sub(2).max(8);
        let mut chunk = String::new();
        let mut cw = 0usize;
        let mut chunks = Vec::new();
        for ch in p.chars() {
            let chw = ch.width().unwrap_or(1);
            if cw + chw > avail && !chunk.is_empty() {
                chunks.push(std::mem::take(&mut chunk));
                cw = 0;
            }
            chunk.push(ch);
            cw += chw;
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        for c in chunks {
            let mut l = Line::new(w);
            l.put(1, &c, Style::bold(style::ACCENT));
            out.push(l.render(None, false));
        }
    }
    {
        let mut l = Line::new(w);
        // Link health is the one disclosure that must survive a narrow
        // pane: reserve it right-aligned BEFORE the key legend claims the
        // row, and shrink the legend into what remains.
        let mut avail = w.saturating_sub(1);
        if let Some(h) = input.herdr {
            let (txt, st) = if h.connected {
                ("⟂ herdr", Style::dim(style::WORKING))
            } else {
                ("⟂ herdr off", Style::dim(style::MUTED))
            };
            let tw = txt.width();
            if w > tw + 2 {
                l.put(w - tw - 1, txt, st);
                avail = w - tw - 3;
            }
        }
        let full = "j/k move · tab queue · enter focus · u/a/o/x act · r reload · ? help · q quit";
        let mid = "j/k · tab · enter · u/a/o/x · r · ? · q";
        let keys = if full.width() < avail {
            full
        } else if mid.width() < avail {
            mid
        } else {
            "? help"
        };
        let mut x = l.put(1, keys, Style::dim(style::MUTED)) + 2;
        if input.watching && x + 10 <= avail {
            x = l.put(x, "⟳ watching", Style::dim(style::WORKING));
        }
        if let Some(f) = &input.flash {
            seg_put(&mut l, x + 2, &[Seg(f.clone(), Style::bold(style::ACCENT))], avail);
        }
        out.push(l.render(None, false));
    }
    Frame { lines: out, sel_line, prompt_line }
}

pub fn help_lines() -> Vec<String> {
    let rows = [
        ("j / k", "move the cursor through the trace"),
        ("tab", "cycle the attention queue (blocked → review → working)"),
        ("enter", "focus the selected attempt's herdr pane (zoom-cycle)"),
        ("u / a / o / x", "unblock · answer · accept · reject — producer-declared, confirm-gated"),
        ("r", "reload the run file now"),
        ("arrows, H/J/K/L", "move this pane left · below · above · right of the work"),
        ("?", "toggle this help"),
        ("q / esc", "quit (esc closes help first)"),
    ];
    let mut out = vec![paint(" keys", Style::bold(style::ACCENT))];
    for (k, v) in rows {
        out.push(format!("   {}  {}", paint(&format!("{k:14}"), Style::fg(style::TEXT)), paint(v, Style::dim(style::MUTED))));
    }
    out.push(String::new());
    out.push(paint(
        " grammar: ● done ◐ working ◈ review ■ blocked ○ queued ✗ failed/sent-back ↩ re-entry ⟲ loop-stub » forward-ref ← fan-in",
        Style::dim(style::MUTED),
    ));
    out.push(paint(
        " evidence: ◆ verified ◇ reported ≈ heuristic ! asserted — dotted rows are futures, not facts",
        Style::dim(style::MUTED),
    ));
    out
}
