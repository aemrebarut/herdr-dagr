//! Rows → ANSI frames. Two layouts, both established terminal grammars:
//! `sidecar` (≥ ~146 cols: trace left, attention queue right) folding to
//! `cockpit` (full-width trace). Interactive browsing uses a stable
//! three-row inspector; an explicit detail mode swaps the trace for a
//! focus-plus-context lens above the full card. Renderers are pure functions
//! of (state, width, presentation mode).

use crate::contract::{Attempt, Doc, Task};
use crate::model::{parse_min, GateJoin, Row, Scene, Seg};
use crate::style::{self, paint, trunc, Line, Style};
use unicode_width::UnicodeWidthStr;

/// Sidecar needs its left panel to keep the full column grammar (≥96
/// cols beside the 48-col attention rail + gutter); folding earlier put the trace
/// through compact rows while a full-width cockpit would have been richer
/// (F5's inversion band). 146 = 96 + QUEUE_W + 2.
pub const FOLD_WIDTH: usize = 146;
const QUEUE_W: usize = 48;

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

fn seg_width(segs: &[Seg]) -> usize {
    segs.iter().map(|Seg(s, _)| s.width()).sum()
}

fn join_mark_style(state: &str) -> Style {
    Style {
        fg: Some(style::state_color(state)),
        bold: matches!(state, "working" | "blocked" | "failed" | "lost"),
        dim: false,
    }
}

/// The join carries the same information at three densities. Prefer the
/// per-input strip; collapse to state counts, then to N→1 when the rail or
/// terminal leaves less room. The selected gate still unrolls exact ids.
fn join_candidates(join: &GateJoin) -> [Vec<Seg>; 3] {
    let mut full: Vec<Seg> = join
        .states
        .iter()
        .map(|state| {
            Seg(
                style::state_glyph(state).to_string(),
                join_mark_style(state),
            )
        })
        .collect();
    full.push(Seg("→".into(), Style::fg(style::ACCENT)));

    let mut counts = Vec::new();
    let mut emitted = 0usize;
    for state in [
        "done",
        "working",
        "review",
        "needs_answer",
        "blocked",
        "ready",
        "waiting",
        "unassigned",
        "queued",
        "failed",
        "rejected",
        "canceled",
        "settled_unverified",
        "lost",
    ] {
        let count = join.states.iter().filter(|s| s.as_str() == state).count();
        if count == 0 {
            continue;
        }
        if emitted > 0 {
            counts.push(Seg(" ".into(), Style::plain()));
        }
        counts.push(Seg(
            format!("{}{count}", style::state_glyph(state)),
            join_mark_style(state),
        ));
        emitted += count;
    }
    let unknown = join.states.len().saturating_sub(emitted);
    if unknown > 0 {
        if emitted > 0 {
            counts.push(Seg(" ".into(), Style::plain()));
        }
        counts.push(Seg(format!("·{unknown}"), Style::fg(style::MUTED)));
    }
    counts.push(Seg("→".into(), Style::fg(style::ACCENT)));

    let total = vec![Seg(
        format!("{}→1 ", join.states.len()),
        Style::bold(style::ACCENT),
    )];
    [full, counts, total]
}

/// Draw the optional join prefix, row glyph, and id while reserving enough
/// room for a recognizable id. Returns the first column after the id.
fn row_head(
    line: &mut Line,
    row: &Row,
    mut x: usize,
    limit: usize,
    force_tiny_join: bool,
    name_prefix: usize,
) -> usize {
    if let Some(join) = &row.join {
        let reserve_name = row.name.width().min(name_prefix) + 2; // glyph + gap + useful id
        let room = limit.saturating_sub(x + reserve_name);
        let candidates = join_candidates(join);
        let picked = if force_tiny_join {
            candidates.get(2).filter(|segs| seg_width(segs) <= room)
        } else {
            candidates.iter().find(|segs| seg_width(segs) <= room)
        };
        if let Some(segs) = picked {
            x = seg_put(line, x, segs, limit);
        }
    }
    x = line.put(
        x,
        &row.glyph.to_string(),
        Style { fg: Some(row.glyph_color), bold: row.hot, dim: false },
    );
    let name_style = if row.dotted && row.glyph != '»' {
        Style::fg(style::GHOST)
    } else if row.glyph == '»' {
        Style::fg(style::ACCENT)
    } else {
        Style { fg: Some(row.glyph_color), bold: row.hot, dim: false }
    };
    line.put(x, &format!(" {}", row.name), name_style)
}

/// Preserve the row's local branch ending while eliding ancestry that cannot
/// fit beside the minimum useful head. This is only used after status has
/// already yielded its space, so normal-width tree geometry is unchanged.
fn fit_rail(rail: &str, max_width: usize) -> String {
    if rail.width() <= max_width {
        return rail.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".into();
    }
    use unicode_width::UnicodeWidthChar;
    let mut used = 0usize;
    let mut tail = Vec::new();
    for c in rail.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max_width - 1 {
            break;
        }
        used += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

fn head_min_width(row: &Row, name_prefix: usize) -> usize {
    let name = row.name.width().min(name_prefix);
    let base = 1 + usize::from(name > 0) + name; // glyph, gap, recognizable id
    row.join
        .as_ref()
        .map(|join| seg_width(&join_candidates(join)[2]))
        .unwrap_or(0)
        + base
}

/// Recursive projects are structural scope nodes, not task records. Keep the
/// fold caret outside the graph; the aggregate-state node then occupies the
/// exact column from which the project's direct child rails descend.
fn project_line(row: &Row, w: usize, selected: bool) -> (String, Option<(usize, usize)>) {
    let mut l = Line::new(w);
    let status_w = seg_width(&row.status).min(30);
    let min_head = 4 + row.name.width().min(8); // ▾ + gap + state node + gap + useful id
    let show_status =
        status_w > 0 && w > 1 + row.rail.width() + min_head + 2 + status_w;
    let status_x = if show_status { w.saturating_sub(status_w + 1) } else { w };
    let max_rail = status_x.saturating_sub(2).saturating_sub(min_head);
    let rail = fit_rail(&row.rail, max_rail);
    let mut x = l.put(1, &rail, Style::fg(style::EDGE));
    let fold_x = x;
    x = l.put(x, &format!("{} ", row.glyph), Style::bold(row.glyph_color));
    x = l.put(
        x,
        &format!("{} ", style::state_glyph(&row.state)),
        Style::bold(style::state_color(&row.state)),
    );
    x = l.put(
        x,
        &trunc(&row.name, status_x.saturating_sub(x)),
        Style::bold(style::TEXT),
    );
    if !row.title.is_empty() && x + 3 < status_x {
        x = l.put(
            x,
            &trunc(
                &format!(" · {}", row.title),
                status_x.saturating_sub(x + 1),
            ),
            Style::fg(style::TEXT),
        );
    }
    if !row.chips.is_empty() && x + 2 < status_x {
        x = seg_put(&mut l, x + 2, &row.chips, status_x.saturating_sub(1));
    }
    if show_status && x + 2 < status_x {
        l.put(
            x + 1,
            &"─".repeat(status_x.saturating_sub(x + 2)),
            Style::dim(style::RULE),
        );
    }
    if show_status && status_x > x + 1 {
        seg_put(&mut l, status_x, &row.status, w.saturating_sub(1));
    }
    (
        l.render(if selected { Some(style::SEL_BG) } else { None }, true),
        row.fold.as_ref().map(|_| (fold_x, fold_x + 1)),
    )
}

/// One full-grammar trace row, responsive: model/status/agent columns
/// hang off the right edge. Also returns the column span of the fold
/// chip when one was drawn, so a click on it can toggle the fold.
fn full_row(row: &Row, w: usize, selected: bool) -> (String, Option<(usize, usize)>) {
    if row.project {
        return project_line(row, w, selected);
    }
    let model_x = w.saturating_sub(44);
    let st_x = w.saturating_sub(31);
    let ag_x = w
        .saturating_sub(21)
        .max(st_x.saturating_add(seg_width(&row.status).min(13) + 1))
        .min(w.saturating_sub(1));
    let mut l = Line::new(w);
    if row.lit {
        l.put(0, "▍", Style::bold(style::ACCENT));
    }
    let rail_style = if row.dotted { Style::fg(style::GHOST) } else { Style::fg(style::EDGE) };
    let max_rail = model_x.saturating_sub(2).saturating_sub(head_min_width(row, 8));
    let rail = fit_rail(&row.rail, max_rail);
    let mut x = l.put(1, &rail, rail_style);
    if row.reentry && !rail.is_empty() {
        // the branch lead's `─` becomes ↩ — the loop lives in the rail
        l.put(x - 1, "↩", Style::bold(style::REJECTED));
    }
    x = row_head(&mut l, row, x, model_x.saturating_sub(1), false, 8);
    // the fold chip sits BEFORE the title: on a folded row the fold is
    // the fact, and a long title must never push it off the row
    let mut fold_span = None;
    if let Some(f) = &row.fold {
        let fx = x + 2;
        x = seg_put(&mut l, fx, &f.segs, model_x.saturating_sub(1));
        fold_span = Some((fx, x));
    }
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
    if row.milestone && x + 2 < model_x {
        l.put(
            x + 1,
            &"─".repeat(model_x.saturating_sub(x + 2)),
            Style::dim(style::ACCENT),
        );
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
    (l.render(if selected { Some(style::SEL_BG) } else { None }, true), fold_span)
}

/// Narrow two-column row for the sidecar's left panel.
fn compact_row(row: &Row, w: usize, selected: bool) -> (String, Option<(usize, usize)>) {
    if row.project {
        return project_line(row, w, selected);
    }
    // Size the right-aligned status before the join/head. Both are primary
    // signals; title/model text uses only the space that remains.
    let mut tail: Vec<(String, Style)> =
        row.status.iter().map(|Seg(s, st)| (s.clone(), *st)).collect();
    let tw = |t: &[(String, Style)]| t.iter().map(|(s, _)| s.width()).sum::<usize>();
    if !tail.is_empty() {
        let avail = w.saturating_sub(2).min(18);
        while tail.len() > 1 && tw(&tail) > avail {
            tail.pop();
        }
        if tw(&tail) > avail {
            if let Some((s0, _)) = tail.first_mut() {
                *s0 = trunc(s0.trim(), avail);
            }
        }
    }
    let mut status_x = w.saturating_sub(tw(&tail) + 1);

    let mut l = Line::new(w);
    if row.lit {
        l.put(0, "▍", Style::bold(style::ACCENT));
    }
    let rail_style = if row.dotted { Style::fg(style::GHOST) } else { Style::fg(style::EDGE) };
    let mut rail = row.rail.clone();
    // A very deep rail can leave less room than the right-anchored status
    // assumed. The row identity and tiny N→1 join outrank a clipped status
    // word; shorten or drop the tail before it can overwrite the head.
    let tail_room = w
        .saturating_sub(2)
        .saturating_sub(1 + rail.width() + head_min_width(row, 4));
    if tw(&tail) > tail_room {
        while tail.len() > 1 && tw(&tail) > tail_room {
            tail.pop();
        }
        if tw(&tail) > tail_room {
            if tail_room < 5 {
                tail.clear();
            } else if let Some((s0, _)) = tail.first_mut() {
                *s0 = trunc(s0.trim(), tail_room);
            }
        }
        status_x = w.saturating_sub(tw(&tail) + 1);
    }
    let max_rail = status_x
        .saturating_sub(2)
        .saturating_sub(head_min_width(row, 4));
    rail = fit_rail(&rail, max_rail);
    let mut x = l.put(1, &rail, rail_style);
    if row.reentry && !rail.is_empty() {
        l.put(x - 1, "↩", Style::bold(style::REJECTED));
    }
    x = row_head(&mut l, row, x, status_x.saturating_sub(1), w <= 24, 4);
    // The aggregate row already says `▸ N items`; use the compact space for
    // its composition rather than repeating the hidden count.
    let mut fold_span = None;
    if let Some(f) = &row.fold {
        let fx = x + 1;
        x = seg_put(&mut l, fx, &f.segs, status_x.saturating_sub(1));
        fold_span = Some((fx, x));
    }
    // short title: drop the "kind: " prefix, truncate hard
    let room = status_x.saturating_sub(x + 2);
    // Relational ink is part of the graph, not decoration. When compact
    // rows have enough room, reserve a small lane for cross-project/extra
    // dependency and criteria chips instead of letting the title consume
    // every remaining cell.
    let chip_room = if row.chips.is_empty() || room < 10 {
        0
    } else {
        seg_width(&row.chips).min(18).min(room / 2)
    };
    let title_room = room.saturating_sub(chip_room + usize::from(chip_room > 0));
    let short = row.title.split_once(": ").map(|(_, t)| t).unwrap_or(&row.title);
    let title_style = if row.dotted {
        Style::dim(style::GHOST)
    } else if row.title_dim {
        Style::plain_dim()
    } else {
        Style::fg(style::TEXT)
    };
    let mut left_end = l.put(x + 1, &trunc(short, title_room), title_style);
    if chip_room > 0 {
        left_end = seg_put(
            &mut l,
            left_end + 1,
            &row.chips,
            (left_end + 1 + chip_room).min(status_x.saturating_sub(1)),
        );
    }
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
            anchor = mx;
        }
    }
    if row.milestone && left_end + 2 < anchor {
        l.put(
            left_end + 1,
            &"─".repeat(anchor.saturating_sub(left_end + 2)),
            Style::dim(style::ACCENT),
        );
    }
    // right edge: the pre-sized status tail, right-aligned — the evidence
    // glyph and waits-target survive compaction (F6).
    if !tail.is_empty() {
        let mut sx = status_x;
        for (s, st) in &tail {
            sx = l.put(sx, s, *st);
        }
    }
    (l.render(if selected { Some(style::SEL_BG) } else { None }, true), fold_span)
}

// ── focus card ──────────────────────────────────────────────────────

/// The run document is bounded, but a single producer-controlled field can
/// still be very large. Normal focus detail wraps completely; this ceiling
/// prevents one selected value from expanding into an unbounded frame.
const MAX_FOCUS_BODY_ROWS: usize = 256;

/// Wrap terminal-safe text to visible columns, preferring word boundaries
/// and hard-wrapping only when a token itself is wider than the card.
/// Returns whether the explicit safety limit omitted a remaining tail.
fn wrap_focus_text(text: &str, width: usize, max_lines: usize) -> (Vec<String>, bool) {
    use unicode_width::UnicodeWidthChar;

    let safe = style::terminal_safe(text);
    if width == 0 || max_lines == 0 {
        return (Vec::new(), !safe.is_empty());
    }
    if safe.is_empty() {
        return (vec![String::new()], false);
    }

    let mut rest = safe.as_str();
    let mut lines = Vec::new();
    while !rest.is_empty() && lines.len() < max_lines {
        let mut used = 0usize;
        let mut hard_end = 0usize;
        let mut word_break: Option<(usize, usize)> = None;
        let mut overflow = false;

        for (idx, ch) in rest.char_indices() {
            let char_width = ch.width().unwrap_or(0);
            if used + char_width > width {
                overflow = true;
                break;
            }
            used += char_width;
            hard_end = idx + ch.len_utf8();
            if ch.is_whitespace() && idx > 0 {
                word_break = Some((idx, hard_end));
            }
        }

        if !overflow {
            lines.push(rest.trim_end().to_string());
            rest = "";
            break;
        }

        let (line_end, next_start) = word_break.unwrap_or_else(|| {
            if hard_end > 0 {
                (hard_end, hard_end)
            } else {
                // Defensive progress for a glyph wider than `width`. Focus
                // cards currently have at least four inner columns.
                let first_end = rest.chars().next().map(char::len_utf8).unwrap_or(0);
                (first_end, first_end)
            }
        });
        lines.push(rest[..line_end].trim_end().to_string());
        rest = rest[next_start..].trim_start();
    }

    let clipped = !rest.is_empty();
    if clipped {
        if let Some(last) = lines.last_mut() {
            *last = trunc(&format!("{last}…"), width);
        }
    }
    (lines, clipped)
}

fn focus_body_row(cw: usize, border_color: u8, text: &str, text_style: Style) -> String {
    let mut line = Line::new(cw);
    line.put(0, "│ ", Style::fg(border_color));
    line.put(2, text, text_style);
    line.put(cw.saturating_sub(2), " │", Style::fg(border_color));
    line.render(None, true)
}

fn focus_body_rows(body: Vec<(String, Style)>, cw: usize, border_color: u8) -> Vec<String> {
    let inner = cw.saturating_sub(4);
    let mut rows = Vec::new();
    let mut limited = false;

    for (text, text_style) in body {
        let remaining = MAX_FOCUS_BODY_ROWS.saturating_sub(rows.len());
        if remaining == 0 {
            limited = true;
            break;
        }
        let (wrapped, clipped) = wrap_focus_text(&text, inner, remaining);
        rows.extend(
            wrapped
                .iter()
                .map(|line| focus_body_row(cw, border_color, line, text_style)),
        );
        if clipped {
            limited = true;
            break;
        }
    }

    if limited {
        rows.push(focus_body_row(
            cw,
            border_color,
            "… additional detail omitted at safety limit",
            Style::dim(style::MUTED),
        ));
    }
    rows
}

fn find_selection<'a>(doc: &'a Doc, key: &str) -> Option<(&'a Task, Option<&'a Attempt>)> {
    if !crate::contract::valid_identity(key) {
        return None;
    }
    if doc
        .projects
        .iter()
        .any(|project| project.id.as_deref().is_some_and(|id| format!("project:{id}") == key))
    {
        return None;
    }
    let tasks = doc.tasks.as_deref()?;
    let mut found = Vec::new();
    for t in tasks {
        for a in &t.attempts {
            if a.id.as_deref() == Some(key) {
                found.push((t, Some(a)));
            }
        }
        if t.id.as_deref() == Some(key) {
            let current = if super::model::needs_current_stub(t) {
                None
            } else {
                t.attempts.iter().max_by_key(|a| a.n.unwrap_or(0))
            };
            found.push((t, current));
        }
    }
    (found.len() == 1).then(|| found[0])
}

/// Preserve the reasoning-effort suffix when a producer supplied the
/// recommended `model·effort` chip and an unusually narrow pane forces a
/// truncation. `very-long-model·max` becoming `very…·max` is more useful than
/// silently dropping the effort from the right edge.
fn compact_model_chip(model: &str, width: usize) -> String {
    let safe = style::terminal_safe(model);
    if safe.width() <= width {
        return safe;
    }
    if let Some((name, effort)) = safe.rsplit_once('·') {
        let suffix = format!("·{effort}");
        if suffix.width() + 2 <= width {
            return format!("{}{}", trunc(name, width - suffix.width()), suffix);
        }
    }
    trunc(&safe, width)
}

fn task_state<'a>(doc: &'a Doc, id: &str) -> &'a str {
    doc.tasks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .find(|task| task.id.as_deref() == Some(id))
        .and_then(|task| task.state.as_deref())
        .unwrap_or("queued")
}

fn dependency_summary(doc: &Doc, task: &Task) -> Option<String> {
    let ids = task.inputs.as_deref().unwrap_or(&task.deps);
    if ids.is_empty() {
        return None;
    }
    let mut out = if task.kind.as_deref() == Some("gate") {
        String::from("joins ")
    } else {
        String::from("waits ")
    };
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            out.push_str(" · ");
        }
        out.push(style::chip_mark(task_state(doc, id)));
        out.push(' ');
        out.push_str(id);
    }
    Some(out)
}

/// The one decision-relevant sentence in the compact inspector. This is a
/// semantic projection, not "the first wrapped focus-card line": attention
/// and failure reasons outrank provenance, while healthy work leads with
/// progress and queued work names what it is waiting for.
fn operational_summary(
    doc: &Doc,
    task: &Task,
    attempt: Option<&Attempt>,
    state: &str,
) -> (String, Style) {
    if matches!(state, "blocked" | "needs_answer") {
        if let Some(unblock) = task.unblock.as_deref().filter(|value| !value.is_empty()) {
            return (format!("needs {unblock}"), Style::bold(style::state_color(state)));
        }
        if let Some(reason) = attempt
            .and_then(|a| a.cause.as_ref())
            .and_then(|cause| cause.reason.as_deref())
            .filter(|value| !value.is_empty())
        {
            return (reason.to_string(), Style::bold(style::state_color(state)));
        }
    }

    if matches!(state, "failed" | "rejected" | "lost") {
        if let Some(reason) = attempt
            .and_then(|a| a.outcome.as_ref())
            .and_then(|outcome| outcome.reason.as_deref())
            .filter(|value| !value.is_empty())
        {
            return (reason.to_string(), Style::bold(style::state_color(state)));
        }
    }

    if state == "working" {
        if let Some(progress) = attempt.and_then(|a| a.progress.as_ref()) {
            let mut text = String::from("progress");
            if let (Some(done), Some(total)) = (progress.done, progress.total) {
                text.push_str(&format!(" {done}/{total}"));
            }
            if let Some(note) = progress.note.as_deref().filter(|value| !value.is_empty()) {
                text.push_str(&format!(" · {note}"));
            }
            return (text, Style::fg(style::WORKING));
        }
        if let Some(live) = attempt.and_then(|a| a.liveness.as_ref()) {
            let mut parts = Vec::new();
            if live.prompt_acknowledged == Some(false) {
                parts.push("prompt not acknowledged".to_string());
            }
            if let (Some(now), Some(last)) = (
                doc.generated_at.as_deref().and_then(parse_min),
                live.last_output_at.as_deref().and_then(parse_min),
            ) {
                parts.push(format!("last output {}m ago", (now - last).max(0)));
            }
            if live.queued_input.unwrap_or(0) > 0 {
                parts.push(format!("{} queued input", live.queued_input.unwrap_or(0)));
            }
            if !parts.is_empty() {
                return (parts.join(" · "), Style::dim(style::WORKING));
            }
        }
    }

    if state == "review" {
        return ("awaiting review".into(), Style::fg(style::REVIEW));
    }
    if matches!(state, "queued" | "waiting" | "ready" | "unassigned") {
        if let Some(summary) = dependency_summary(doc, task) {
            return (summary, Style::dim(style::MUTED));
        }
        return ("ready · no unmet dependencies".into(), Style::fg(style::DONE));
    }
    if state == "done" {
        if let Some(outcome) = attempt.and_then(|a| a.outcome.as_ref()) {
            let (glyph, color) = style::evidence(outcome.evidence.as_deref().unwrap_or("asserted"));
            let mut text = format!("{glyph} {}", outcome.evidence.as_deref().unwrap_or("asserted"));
            if let Some(receipt) = outcome.receipt.as_deref().filter(|value| !value.is_empty()) {
                text.push_str(&format!(" · {receipt}"));
            }
            return (text, Style::fg(color));
        }
    }
    if state == "canceled" {
        return ("canceled · retained as history".into(), Style::dim(style::MUTED));
    }
    if let Some(note) = task.note.as_deref().filter(|value| !value.is_empty()) {
        return (note.to_string(), Style::fg(style::TEXT));
    }
    if let Some(criteria) = task.criteria.as_deref().filter(|value| !value.is_empty()) {
        return (format!("criteria {criteria}"), Style::dim(style::DONE));
    }
    if let Some(summary) = dependency_summary(doc, task) {
        return (summary, Style::dim(style::MUTED));
    }
    ("no additional context".into(), Style::dim(style::MUTED))
}

fn compact_rule(width: usize, left: &str, right: &str) -> Line {
    let mut line = Line::new(width);
    if width == 0 {
        return line;
    }
    line.put(0, &"═".repeat(width), Style::fg(style::EDGE));
    line.put(0, left, Style::fg(style::EDGE));
    if width > 1 {
        line.put(width - 1, right, Style::fg(style::EDGE));
    }
    line
}

fn compact_header_row(display: &str, title: &str, state: &str, width: usize) -> String {
    let mut line = Line::new(width);
    if width > 0 {
        line.put(0, "║", Style::fg(style::EDGE));
    }
    if width > 1 {
        line.put(width - 1, "║", Style::fg(style::EDGE));
    }
    if width < 4 {
        return line.render(None, true);
    }
    let boundary = width - 1;
    let limit = boundary.saturating_sub(1);
    let color = style::state_color(state);
    let mut x = 2.min(limit);
    if x < limit {
        x = line.put(x, &style::state_glyph(state).to_string(), Style::bold(color));
    }
    if x < limit {
        x = line.put(x, " ", Style::plain());
    }
    if x < limit {
        x = line.put(x, &trunc(display, limit.saturating_sub(x)), Style::bold(color));
    }
    if x + 3 < limit {
        x = line.put(x, " · ", Style::dim(style::RULE));
        x = line.put(
            x,
            &trunc(&state.to_uppercase(), limit.saturating_sub(x)),
            Style::bold(color),
        );
    }
    if !title.is_empty() && x + 3 < limit {
        x = line.put(x, " · ", Style::dim(style::RULE));
        x = line.put(x, &trunc(title, limit.saturating_sub(x)), Style::fg(style::TEXT));
    }
    if x < limit {
        line.put(x, " ", Style::plain());
    }
    line.render(None, true)
}

fn compact_body_row(text: &str, text_style: Style, width: usize) -> String {
    let mut line = Line::new(width);
    if width == 0 {
        return line.render(None, true);
    }
    line.put(0, "║", Style::fg(style::EDGE));
    if width > 1 {
        line.put(width - 1, "║", Style::fg(style::EDGE));
    }
    if width > 4 {
        line.put(2, &trunc(text, width - 4), text_style);
    }
    line.render(None, true)
}

fn compact_metadata_border(
    actor: &str,
    elapsed: Option<&str>,
    model: &str,
    width: usize,
) -> String {
    let mut line = compact_rule(width, "╚", "╝");
    if width < 4 {
        return line.render(None, true);
    }
    let inner_start = 1usize;
    let inner_end = width - 1;
    let inner_width = inner_end - inner_start;

    // Model+effort is the most fragile identity at narrow widths, so reserve
    // it from the right before spending cells on actor decoration or timing.
    let model_budget = inner_width.saturating_sub(usize::from(!actor.is_empty()) * 3);
    let model_chip = compact_model_chip(model, model_budget);
    let decorated_model = format!(" [{model_chip}] ");
    let actor_width = style::terminal_safe(actor).width();
    let model_block = if !model_chip.is_empty()
        && decorated_model.width()
            + if actor.is_empty() { 0 } else { actor_width.saturating_add(1) }
            <= inner_width
    {
        decorated_model
    } else {
        model_chip
    };
    let model_x = inner_end.saturating_sub(model_block.width());
    if !model_block.is_empty() {
        line.put(model_x, &model_block, Style::bold(style::MUTED));
    }

    let actor_room = if model_block.is_empty() {
        inner_width
    } else {
        model_x.saturating_sub(inner_start + 1)
    };
    let actor_chip = trunc(actor, actor_room);
    let decorated_actor = format!(" {actor_chip} ");
    let (actor_x, actor_block) = if !actor_chip.is_empty() && decorated_actor.width() <= actor_room {
        (inner_start + 1, decorated_actor)
    } else {
        (inner_start, actor_chip)
    };
    let actor_end = if actor_block.is_empty() {
        inner_start
    } else {
        line.put(actor_x, &actor_block, Style::dim(style::MUTED))
    };

    if let Some(elapsed) = elapsed.filter(|value| !value.is_empty()) {
        let right = if model_block.is_empty() { inner_end } else { model_x };
        let gap = right.saturating_sub(actor_end);
        let elapsed_block = format!(" {elapsed} ");
        if elapsed_block.width() + 2 <= gap {
            let x = actor_end + (gap - elapsed_block.width()) / 2;
            line.put(x, &elapsed_block, Style::dim(style::MUTED));
        }
    }
    line.render(None, true)
}

fn compact_identity_border(
    task: &Task,
    attempt: Option<&Attempt>,
    now: Option<i64>,
    width: usize,
) -> String {
    let actor = attempt
        .and_then(|a| a.actor.as_deref())
        .or(task.owner.as_deref())
        .unwrap_or("");
    let model = attempt.and_then(|a| a.model.as_deref()).unwrap_or("");
    let elapsed = attempt.and_then(|a| {
        let start = a.started_at.as_deref().and_then(parse_min)?;
        match (a.ended_at.as_deref().and_then(parse_min), now) {
            (Some(end), _) if end >= start => Some(format!("{}m", end - start)),
            (None, Some(current)) if current >= start => Some(format!("{}m…", current - start)),
            _ => None,
        }
    });
    compact_metadata_border(actor, elapsed.as_deref(), model, width)
}

/// Exactly four rows at every selection and width. The fixed contract is
/// what keeps cursor movement from changing the graph viewport's geometry.
pub fn compact_inspector(
    doc: &Doc,
    key: Option<&str>,
    width: usize,
    _hints: Option<&crate::herdr::Hints>,
    _messages: &[crate::message::Summary],
) -> Vec<String> {
    if width < 8 {
        return vec![
            compact_rule(width, "╔", "╗").render(None, true),
            compact_header_row("…", "", "queued", width),
            compact_body_row("", Style::plain(), width),
            compact_metadata_border("", None, "", width),
        ];
    }
    let Some(key) = key else {
        return vec![
            compact_rule(width, "╔", "╗").render(None, true),
            compact_header_row("nothing selected", "", "queued", width),
            compact_body_row("j/k selects a row", Style::dim(style::MUTED), width),
            compact_metadata_border("", None, "", width),
        ];
    };

    if let Some(id) = key.strip_prefix("project:") {
        let project = doc.projects.iter().find(|project| project.id.as_deref() == Some(id));
        if let Some(project) = project {
            let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "queued".into());
            return vec![
                compact_rule(width, "╔", "╗").render(None, true),
                compact_header_row(
                    id,
                    project.title.as_deref().unwrap_or("project"),
                    &state,
                    width,
                ),
                compact_body_row(
                    project.note.as_deref().unwrap_or("project scope"),
                    Style::dim(style::MUTED),
                    width,
                ),
                compact_metadata_border(
                    project.owner.as_deref().unwrap_or("unassigned"),
                    None,
                    "",
                    width,
                ),
            ];
        }
    }

    let Some((task, attempt)) = find_selection(doc, key) else {
        return vec![
            compact_rule(width, "╔", "╗").render(None, true),
            compact_header_row("selection unavailable", "", "blocked", width),
            compact_body_row(
                "reload or choose another row",
                Style::dim(style::MUTED),
                width,
            ),
            compact_metadata_border("", None, "", width),
        ];
    };
    let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "invalid".into());
    let display = attempt.and_then(|a| a.id.as_deref()).or(task.id.as_deref()).unwrap_or(key);

    let (summary, summary_style) = operational_summary(doc, task, attempt, &state);
    vec![
        compact_rule(width, "╔", "╗").render(None, true),
        compact_header_row(display, task.title.as_deref().unwrap_or(""), &state, width),
        compact_body_row(&summary, summary_style, width),
        compact_identity_border(
            task,
            attempt,
            doc.generated_at.as_deref().and_then(parse_min),
            width,
        ),
    ]
}

pub fn focus_card(
    doc: &Doc,
    key: &str,
    cw: usize,
    hints: Option<&crate::herdr::Hints>,
    messages: &[crate::message::Summary],
) -> Vec<String> {
    // below ~8 cols no card grammar survives; claim the space, draw nothing
    if cw < 8 {
        return vec![paint("…", Style::dim(style::MUTED))];
    }
    if let Some(id) = key.strip_prefix("project:") {
        let mut projects = doc.projects.iter().filter(|project| {
            project.id.as_deref() == Some(id) && crate::contract::valid_identity(id)
        });
        let node_collision = doc.tasks.as_deref().unwrap_or(&[]).iter().any(|task| {
            task.id.as_deref() == Some(key)
                || task.attempts.iter().any(|attempt| attempt.id.as_deref() == Some(key))
        });
        if let (Some(project), None, false) = (projects.next(), projects.next(), node_collision) {
            let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "queued".into());
            let col = style::state_color(&state);
            let label = trunc(&format!("─ project {id} "), cw.saturating_sub(2));
            let mut lines = vec![paint(
                &format!("┌{label}{}┐", "─".repeat(cw.saturating_sub(2 + label.width()))),
                Style::bold(col),
            )];
            let body = [
                (project.title.as_deref(), Style::fg(style::TEXT)),
                (project.owner.as_deref(), Style::dim(style::MUTED)),
                (project.note.as_deref(), Style::dim(style::MUTED)),
            ]
            .into_iter()
            .filter_map(|(text, text_style)| {
                text.filter(|text| !text.is_empty())
                    .map(|text| (text.to_string(), text_style))
            })
            .collect();
            lines.extend(focus_body_rows(body, cw, col));
            lines.push(paint(
                &format!("└{}┘", "─".repeat(cw.saturating_sub(2))),
                Style::fg(col),
            ));
            return lines;
        }
    }
    let Some((task, attempt)) = find_selection(doc, key) else {
        return vec![paint("  (nothing selected)", Style::dim(style::MUTED))];
    };
    let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "invalid".into());
    let col = style::state_color(&state);
    let now = doc.generated_at.as_deref().and_then(parse_min);
    let mut body: Vec<(String, Style)> = Vec::new();

    body.push((task.title.clone().unwrap_or_default(), Style::fg(style::TEXT)));
    if let Some(project) = task.project.as_deref() {
        body.push((format!("project {project}"), Style::dim(style::MUTED)));
    }
    if let Some(criteria) = task.criteria.as_deref() {
        body.push((format!("criteria {criteria}"), Style::fg(style::DONE)));
    }

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

    let recent_messages: Vec<_> = messages.iter().filter(|m| m.target == tid).collect();
    for message in recent_messages.iter().skip(recent_messages.len().saturating_sub(2)) {
        let resolved = doc.events.iter().any(|e| {
            e.message_id.as_deref() == Some(message.id.as_str())
                || e.source_messages.iter().any(|id| id == &message.id)
        });
        let status = if resolved { "resolved" } else { message.status.as_str() };
        let text = message
            .text
            .chars()
            .filter_map(|c| match c {
                '\n' | '\r' => Some('↵'),
                '\t' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect::<String>();
        body.push((
            format!(
                "↗ {} · {} · {} · {} · {}",
                message.id,
                status,
                message.starter,
                message.authority.label(),
                text
            ),
            if resolved { Style::dim(style::DONE) } else { Style::fg(style::ACCENT) },
        ));
    }

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
    lines.extend(focus_body_rows(body, cw, col));
    // The action is structural, not producer detail: keep it available even
    // when a pathological field consumes the entire safety budget.
    lines.push(focus_body_row(
        cw,
        col,
        "[m] message orchestrator  [enter] focus pane",
        Style::fg(style::ACCENT),
    ));
    lines.push(paint(&format!("└{}┘", "─".repeat(cw.saturating_sub(2))), Style::fg(col)));
    lines
}

fn project_breadcrumb(doc: &Doc, project_id: Option<&str>) -> String {
    let Some(mut current) = project_id else { return "run root".into() };
    let mut path = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current.to_string()) {
        let Some(project) = doc.projects.iter().find(|project| project.id.as_deref() == Some(current))
        else {
            path.push(current.to_string());
            break;
        };
        path.push(project.title.as_deref().or(project.id.as_deref()).unwrap_or(current).to_string());
        let Some(parent) = project.parent.as_deref() else { break };
        current = parent;
    }
    path.reverse();
    if path.is_empty() { "run root".into() } else { path.join(" / ") }
}

fn relation_row(label: &str, relations: &[(String, String)], width: usize) -> String {
    let mut line = Line::new(width);
    let content_x = if width >= 48 { 14 } else { 9.min(width.saturating_sub(1)) };
    line.put(1.min(width), label, Style::dim(style::MUTED));
    if relations.is_empty() {
        line.put(content_x, "· none", Style::dim(style::MUTED));
        return line.render(None, true);
    }

    let mut x = content_x;
    let mut shown = 0usize;
    for (id, state) in relations {
        let chip = format!("{} {id}", style::state_glyph(state));
        // Keep four cells for a truthful +N tail when more relationships do
        // not fit. A clipped identifier is worse than an explicit count.
        let reserve = if shown + 1 < relations.len() { 5 } else { 1 };
        if x + chip.width() + reserve > width {
            break;
        }
        if shown > 0 {
            x = line.put(x, "  ", Style::plain());
        }
        x = line.put(x, &style::state_glyph(state).to_string(), Style::bold(style::state_color(state)));
        x = line.put(x + 1, id, Style::fg(style::TEXT));
        shown += 1;
    }
    let hidden = relations.len().saturating_sub(shown);
    if hidden > 0 {
        let more = format!("+{hidden}");
        let more_x = x.max(width.saturating_sub(more.width() + 1));
        line.put(more_x.min(width.saturating_sub(more.width())), &more, Style::bold(style::ACCENT));
    }
    line.render(None, true)
}

fn lens_connector(count: usize, width: usize) -> String {
    let mut line = Line::new(width);
    let x = if width >= 48 { 15 } else { 10.min(width.saturating_sub(1)) };
    let mark = match count {
        0 => "·",
        1 => "│",
        _ => "╲┼╱",
    };
    line.put(x.saturating_sub(usize::from(count > 1)), mark, Style::dim(style::ACCENT));
    line.render(None, true)
}

fn selected_lens_row(key: &str, title: &str, state: &str, glyph: char, width: usize) -> String {
    let mut line = Line::new(width);
    let content_x = if width >= 48 { 14 } else { 9.min(width.saturating_sub(1)) };
    line.put(1.min(width), "focus", Style::bold(style::ACCENT));
    let mut x = line.put(content_x, &glyph.to_string(), Style::bold(style::state_color(state)));
    x = line.put(x + 1, key, Style::bold(style::state_color(state)));
    if x + 2 < width {
        line.put(x + 2, &trunc(title, width.saturating_sub(x + 3)), Style::fg(style::TEXT));
    }
    line.render(Some(style::SEL_BG), true)
}

/// A six-row causal lens for explicit detail mode. It follows declared DAG
/// relationships rather than neighboring display rows, so cross-project
/// dependencies and gate fan-in remain truthful after the full trace is
/// compressed. Direct nodes keep short labels; overflow becomes dots/counts.
fn focus_lens(doc: &Doc, scene: &Scene, key: &str, width: usize) -> (Vec<String>, usize) {
    if let Some((task, attempt)) = find_selection(doc, key) {
        let tid = task.id.as_deref().unwrap_or(key);
        let input_ids = task.inputs.as_deref().unwrap_or(&task.deps);
        let inputs = input_ids
            .iter()
            .map(|id| (id.clone(), task_state(doc, id).to_string()))
            .collect::<Vec<_>>();
        let outputs = doc
            .tasks
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|candidate| {
                let deps = candidate.inputs.as_deref().unwrap_or(&candidate.deps);
                deps.iter().any(|dependency| dependency == tid).then(|| {
                    (
                        candidate.id.clone().unwrap_or_else(|| "?".into()),
                        candidate.state.clone().unwrap_or_else(|| "queued".into()),
                    )
                })
            })
            .collect::<Vec<_>>();
        let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "queued".into());
        let glyph = scene
            .rows
            .iter()
            .find(|row| row.key == key)
            .map(|row| row.glyph)
            .unwrap_or_else(|| style::state_glyph(&state));
        let display = attempt.and_then(|a| a.id.as_deref()).unwrap_or(tid);
        let scope = project_breadcrumb(doc, task.project.as_deref());
        let mut scope_line = Line::new(width);
        scope_line.put(1.min(width), "scope", Style::dim(style::MUTED));
        scope_line.put(
            if width >= 48 { 14 } else { 9.min(width.saturating_sub(1)) },
            &trunc(&scope, width.saturating_sub(if width >= 48 { 15 } else { 10 })),
            Style::bold(style::TEXT),
        );
        return (
            vec![
                scope_line.render(None, true),
                relation_row("inputs", &inputs, width),
                lens_connector(inputs.len(), width),
                selected_lens_row(display, task.title.as_deref().unwrap_or(""), &state, glyph, width),
                lens_connector(usize::from(!outputs.is_empty()), width),
                relation_row("unlocks", &outputs, width),
            ],
            3,
        );
    }

    // Projects use the same lens grammar: parent scope above, direct child
    // projects/tasks below. They have no model identity and no task action.
    if let Some(id) = key.strip_prefix("project:") {
        if let Some(project) = doc.projects.iter().find(|project| project.id.as_deref() == Some(id)) {
            let inputs = project
                .parent
                .as_deref()
                .map(|parent| {
                    let project_key = format!("project:{parent}");
                    vec![(
                        parent.to_string(),
                        crate::model::selection_state(doc, &project_key).unwrap_or_else(|| "queued".into()),
                    )]
                })
                .unwrap_or_default();
            let mut outputs = doc
                .projects
                .iter()
                .filter(|child| child.parent.as_deref() == Some(id))
                .map(|child| {
                    let child_id = child.id.as_deref().unwrap_or("?");
                    let child_key = format!("project:{child_id}");
                    (
                        child_id.to_string(),
                        crate::model::selection_state(doc, &child_key).unwrap_or_else(|| "queued".into()),
                    )
                })
                .collect::<Vec<_>>();
            outputs.extend(
                doc.tasks
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|task| task.project.as_deref() == Some(id))
                    .map(|task| {
                        (
                            task.id.clone().unwrap_or_else(|| "?".into()),
                            task.state.clone().unwrap_or_else(|| "queued".into()),
                        )
                    }),
            );
            let state = crate::model::selection_state(doc, key).unwrap_or_else(|| "queued".into());
            let scope = project_breadcrumb(doc, Some(id));
            let mut scope_line = Line::new(width);
            scope_line.put(1.min(width), "scope", Style::dim(style::MUTED));
            scope_line.put(
                if width >= 48 { 14 } else { 9.min(width.saturating_sub(1)) },
                &trunc(&scope, width.saturating_sub(if width >= 48 { 15 } else { 10 })),
                Style::bold(style::TEXT),
            );
            return (
                vec![
                    scope_line.render(None, true),
                    relation_row("parent", &inputs, width),
                    lens_connector(inputs.len(), width),
                    selected_lens_row(
                        id,
                        project.title.as_deref().unwrap_or("project"),
                        &state,
                        style::state_glyph(&state),
                        width,
                    ),
                    lens_connector(usize::from(!outputs.is_empty()), width),
                    relation_row("contains", &outputs, width),
                ],
                3,
            );
        }
    }

    (
        vec![
            paint("scope  unavailable", Style::dim(style::MUTED)),
            String::new(),
            String::new(),
            paint("focus  selection unavailable", Style::bold(style::BLOCKED)),
            String::new(),
            String::new(),
        ],
        3,
    )
}

// ── attention queue panel ───────────────────────────────────────────

pub fn queue_panel(scene: &Scene, qw: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut head = paint(" attention", Style::bold(style::ACCENT))
        + &paint(&format!("  {} need eyes", scene.queue.len()), Style::dim(style::MUTED));
    if let Some(z) = &scene.zoom {
        if z.outside > 0 {
            // a zoom narrows the queue; what it cut out stays counted
            head += &paint(&format!(" · +{} outside", z.outside), Style::bold(style::BLOCKED));
        }
    }
    lines.push(head);
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
    /// Contextual message-composer prompt.
    pub prompt: Option<String>,
    /// Append-only operator-message state reconstructed once per reload.
    pub messages: &'a [crate::message::Summary],
}

/// The logical frame can serve three consumers without conflating their
/// geometry: snapshots keep the complete card, ordinary interaction gets a
/// stable three-row inspector, and explicit details get a causal lens plus
/// the complete card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorMode {
    Full,
    Compact,
    Focus,
}

/// What a left-click on a frame region means. The renderer owns the
/// layout, so it is the only thing that can say which screen cells
/// belong to which row — the view just replays these against (x, y).
#[derive(Clone)]
pub enum HitTarget {
    /// select this trace row (row key)
    Row(String),
    /// jump to this task's latest attempt (attention-queue item)
    Task(String),
    /// toggle this row's fold open (the ▸ chip)
    Fold(String),
    /// Open the contextual orchestrator-message composer.
    Message,
    /// Expand the compact inspector into focus-plus-context detail mode.
    Details,
}

#[derive(Clone)]
pub struct Hit {
    pub line: usize,
    pub x0: usize,
    pub x1: usize,
    pub target: HitTarget,
}

/// A composed frame plus the output line carrying the selection, so the
/// interactive viewport can keep it on screen (scroll-follow).
pub struct Frame {
    pub lines: Vec<String>,
    pub sel_line: Option<usize>,
    /// End of the independently scrollable graph region. Selection
    /// details live after this boundary so the interactive view can dock
    /// them instead of pretending the whole screen is one long document.
    pub graph_end: usize,
    /// End of the detail dock and start of the fixed footer. Together with
    /// `graph_end`, this makes the frame's vertical regions explicit:
    /// graph | selected-item detail | footer.
    pub detail_end: usize,
    /// Clickable regions of this frame (mouse support).
    pub hits: Vec<Hit>,
}

pub fn compose(input: &FrameInput, w: usize) -> Frame {
    compose_with_inspector(input, w, InspectorMode::Full)
}

pub fn compose_with_inspector(input: &FrameInput, w: usize, inspector: InspectorMode) -> Frame {
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
        if let Some(z) = &scene.zoom {
            // the breadcrumb keeps a zoomed view from passing as the whole
            // run — and names what attention the zoom cropped out
            x = l.put(x + 1, &trunc(&format!("▸ {}", z.root), limit.saturating_sub(x + 1)), Style::bold(style::ACCENT));
            if z.outside > 0 {
                x = l.put(
                    x + 2,
                    &trunc(&format!("+{} need eyes outside", z.outside), limit.saturating_sub(x + 2)),
                    Style::bold(style::BLOCKED),
                );
            }
        }
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
    let mut hits: Vec<Hit> = Vec::new();
    if inspector == InspectorMode::Focus {
        let base = out.len();
        let (lens, selected) = focus_lens(input.doc, scene, sel.unwrap_or(""), w);
        sel_line = Some(base + selected);
        out.extend(lens);
    } else if w >= FOLD_WIDTH {
        // sidecar: trace left · compact attention queue right. Selection
        // detail is deliberately not part of this fixed-width rail: it is
        // docked across the full frame after the graph in both layouts.
        let right_w = QUEUE_W;
        let left_w = w - right_w - 2;
        let right: Vec<String> = queue_panel(scene, right_w);
        let left: Vec<(String, Option<(usize, usize)>)> = scene
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
        for (i, r) in scene.rows.iter().enumerate() {
            // the fold chip's hit is narrower than the row's and must win:
            // click resolution takes the first match, so it goes in first
            if let Some((x0, x1)) = left[i].1 {
                if x1 > x0 {
                    hits.push(Hit { line: base + i, x0, x1, target: HitTarget::Fold(r.key.clone()) });
                }
            }
            if r.selectable {
                hits.push(Hit { line: base + i, x0: 0, x1: left_w, target: HitTarget::Row(r.key.clone()) });
            }
        }
        // queue_panel: one header line, then one line per item
        for (i, item) in scene.queue.iter().enumerate() {
            hits.push(Hit {
                line: base + 1 + i,
                x0: left_w + 2,
                x1: w,
                target: HitTarget::Task(item.task_id.clone()),
            });
        }
        for i in 0..rows_n {
            let l = left
                .get(i)
                .map(|(s, _)| s.clone())
                .unwrap_or_else(|| Line::new(left_w).render(None, true));
            let r = right.get(i).cloned().unwrap_or_default();
            out.push(format!("{l}  {r}"));
        }
    } else {
        // cockpit: full-width trace; below ~96 cols the full column layout
        // self-destructs, so rows go compact
        let base = out.len();
        if let Some(i) = sel_row {
            sel_line = Some(base + i);
        }
        for (i, r) in scene.rows.iter().enumerate() {
            let (line, fold_span) = if w >= 96 {
                full_row(r, w, sel == Some(r.key.as_str()))
            } else {
                compact_row(r, w, sel == Some(r.key.as_str()))
            };
            if let Some((x0, x1)) = fold_span {
                if x1 > x0 {
                    hits.push(Hit { line: base + i, x0, x1, target: HitTarget::Fold(r.key.clone()) });
                }
            }
            if r.selectable {
                hits.push(Hit { line: base + i, x0: 0, x1: w, target: HitTarget::Row(r.key.clone()) });
            }
            out.push(line);
        }
    }

    // Browse mode has exactly four inspector rows; selection content can
    // never renegotiate the graph viewport. Full/focus modes retain complete
    // wrapped detail for snapshots and explicit drill-down respectively.
    let graph_end = out.len();
    let card_w = w.saturating_sub(1);
    if inspector == InspectorMode::Compact {
        let detail_start = out.len();
        out.extend(compact_inspector(input.doc, sel, card_w, input.herdr, input.messages));
        for line in detail_start..out.len() {
            hits.push(Hit { line, x0: 0, x1: card_w, target: HitTarget::Details });
        }
    } else if let Some(k) = sel {
        if inspector == InspectorMode::Full {
            out.push(String::new());
        }
        // Never write the card into the terminal's final auto-wrap cell.
        // Ghostty and Herdr both correctly treat that cell as a wrap trigger,
        // which made the right border disappear at otherwise valid widths.
        let card = focus_card(input.doc, k, card_w, input.herdr, input.messages);
        let card_start = out.len();
        if let Some(i) = card.iter().position(|line| line.contains("[m] message")) {
            hits.push(Hit {
                line: card_start + i,
                x0: 0,
                x1: card_w,
                target: HitTarget::Message,
            });
        }
        out.extend(card);
    } else if w < FOLD_WIDTH && inspector == InspectorMode::Full {
        // Preserve the cockpit's breathing room when nothing is selected.
        out.push(String::new());
    }
    let detail_end = out.len();

    // The compact inspector's metadata row is already a bottom border. Do
    // not stack a second horizontal rule directly beneath it; full/focus
    // cards retain the traditional footer separator.
    if inspector != InspectorMode::Compact {
        out.push(paint(&format!(" {}", "─".repeat(w.saturating_sub(2))), Style::dim(style::RULE)));
    }
    if let Some(p) = &input.prompt {
        // The confirm gate's whole point is that the human inspects the
        // EXACT argv — so the prompt wraps across as many lines as it
        // needs; clipping could hide a trailing argument.
        use unicode_width::UnicodeWidthChar;
        let avail = w.saturating_sub(2).max(8);
        let mut chunks = Vec::new();
        for logical in p.split('\n') {
            let mut chunk = String::new();
            let mut cw = 0usize;
            for ch in logical.chars() {
                let chw = ch.width().unwrap_or(1);
                if cw + chw > avail && !chunk.is_empty() {
                    chunks.push(std::mem::take(&mut chunk));
                    cw = 0;
                }
                chunk.push(ch);
                cw += chw;
            }
            if !chunk.is_empty() || logical.is_empty() {
                chunks.push(chunk);
            }
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
        // curated, not complete — the flash shares this row and must keep
        // room to speak; `?` holds the full list
        let (full, mid, tiny) = if inspector == InspectorMode::Focus {
            (
                "j/k scroll · ctrl-u/d page · d/esc close · enter focus · m message · ? help · q quit",
                "j/k scroll · d/esc close · enter · m message · ? · q",
                "d/esc close · ? help",
            )
        } else {
            (
                "j/k move · ←/→ fold/zoom · d details · tab queue · enter focus · m message · f open · ? help · q quit",
                "j/k · ←/→ · d details · tab · enter · m message · f · ? · q",
                "d details · ? help",
            )
        };
        let keys = if full.width() < avail {
            full
        } else if mid.width() < avail {
            mid
        } else {
            tiny
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
    Frame { lines: out, sel_line, graph_end, detail_end, hits }
}

pub fn help_lines() -> Vec<String> {
    let rows = [
        ("j / k", "move the cursor through the trace"),
        ("→ / l", "zoom the trace to the selected branch (a folded row opens first)"),
        ("← / h", "fold the branch · on a folded or leaf row, jump to its parent"),
        ("z", "fold every settled branch — the trace shows what still needs you · again unfolds"),
        ("g / G", "top · bottom of the trace"),
        ("ctrl-d / ctrl-u", "half a screen down · up"),
        ("d", "open selection details with a causal neighborhood lens · d/esc closes"),
        ("tab", "cycle attention (blocked → review/questions → working)"),
        ("enter", "focus the selected attempt's herdr pane (zoom-cycle)"),
        ("m", "message the orchestrator · Tab picks a starter · text stays editable"),
        ("ctrl-t in message", "toggle explicit authority: return to me ↔ may decide + continue"),
        ("f", "open another run file (type to filter · recent files first)"),
        ("/", "find a row by id, title, or agent · n/N cycle the matches"),
        ("y", "copy the selected row id to the clipboard"),
        ("r", "reload the run file now"),
        ("mouse", "click selects · click inspector opens details · wheel moves/scrolls detail"),
        ("drag", "select text · copied to the clipboard when you let go"),
        ("?", "toggle this help"),
        ("q / esc", "quit (esc backs out of help, details, and zoom first)"),
    ];
    let mut out = vec![paint(" keys", Style::bold(style::ACCENT))];
    for (k, v) in rows {
        out.push(format!("   {}  {}", paint(&format!("{k:15}"), Style::fg(style::TEXT)), paint(v, Style::dim(style::MUTED))));
    }
    out.push(String::new());
    out.push(paint(
        " grammar: ● done ◎ working/join ◈ review/question ■ blocked ○ queued ✗ failed × canceled ↩ re-entry ⟲ loop-stub » forward-ref",
        Style::dim(style::MUTED),
    ));
    out.push(paint(
        " evidence: ◆ verified ◇ reported ≈ heuristic ! asserted — dotted rows are futures, not facts",
        Style::dim(style::MUTED),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    fn sample() -> Doc {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/samples/run.json"
        ))
        .expect("sample run file");
        serde_json::from_str(&raw).expect("sample parses")
    }

    fn frame(doc: &Doc, w: usize) -> (Frame, model::Scene) {
        let scene = model::build(doc, None, None, None, &model::ViewOpts::default());
        let f = compose(
            &FrameInput {
                doc,
                scene: &scene,
                selected: None,
                banner: None,
                flash: None,
                stale_min: None,
                watching: false,
                herdr: None,
                prompt: None,
                messages: &[],
            },
            w,
        );
        let scene = model::build(doc, None, None, None, &model::ViewOpts::default());
        (f, scene)
    }

    #[test]
    fn project_state_nodes_connect_to_all_content_rails() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "project-rails"},
            "projects": [
                {"id": "ROOT", "title": "Root"},
                {"id": "CHILD", "title": "Child", "parent": "ROOT"}
            ],
            "tasks": [
                {"id": "ROOT-TASK", "title": "root task", "kind": "impl",
                 "project": "ROOT", "owner": "dev", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "CHILD-TASK", "title": "child task", "kind": "impl",
                 "project": "CHILD", "owner": "dev", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "ROOT-GATE", "title": "root milestone", "kind": "gate",
                 "project": "ROOT", "owner": "lead", "state": "queued",
                 "deps": ["ROOT-TASK", "CHILD-TASK"], "attempts": []}
            ]
        }))
        .unwrap();
        let scene = model::build(&doc, None, None, None, &model::ViewOpts::default());
        let plain = |key: &str, width: usize| {
            let row = scene.rows.iter().find(|row| row.key == key).unwrap();
            crate::select::plain(&compact_row(row, width, false).0)
        };

        for width in [20, 78] {
            let root = plain("project:ROOT", width);
            let root_task = plain("ROOT-TASK", width);
            let child = plain("project:CHILD", width);
            let child_task = plain("CHILD-TASK", width);
            let gate = plain("ROOT-GATE", width);
            assert!(root.contains("▾ ○ ROOT"), "width={width}: {root:?}");
            assert!(child.contains("▾ ○ CHILD"), "width={width}: {child:?}");

            let column = |line: &str, mark: char| {
                line.chars().position(|c| c == mark).unwrap()
            };
            assert_eq!(
                column(&root, '○'),
                column(&root_task, '├'),
                "width={width}: a later milestone must keep the root rail open"
            );
            assert_eq!(
                column(&root, '○'),
                column(&child, '▾'),
                "width={width}: a child project's fold control stays on the parent rail"
            );
            assert_eq!(
                column(&root, '○'),
                column(&child_task, '│'),
                "width={width}: the parent continuation survives the nested project"
            );
            assert_eq!(
                column(&child, '○'),
                column(&child_task, '╰'),
                "width={width}: nested scope node must feed its task rail"
            );
            assert_eq!(
                column(&root, '○'),
                column(&gate, '╰'),
                "width={width}: the final milestone closes the root rail"
            );
        }
    }

    #[test]
    fn hits_cover_rows_and_queue_in_both_layouts() {
        let doc = sample();
        for w in [150usize, 72] {
            let (f, scene) = frame(&doc, w);
            let rows = f
                .hits
                .iter()
                .filter(|h| matches!(h.target, HitTarget::Row(_)))
                .count();
            assert_eq!(
                rows,
                scene.rows.iter().filter(|r| r.selectable).count(),
                "w={w}: one hit per selectable row"
            );
            let queue = f
                .hits
                .iter()
                .filter(|h| matches!(h.target, HitTarget::Task(_)))
                .count();
            let expect_queue = if w >= FOLD_WIDTH { scene.queue.len() } else { 0 };
            assert_eq!(queue, expect_queue, "w={w}: queue hits only in the sidecar");
            for h in &f.hits {
                assert!(h.line < f.lines.len(), "w={w}: hit line in frame");
                assert!(h.x0 < h.x1 && h.x1 <= w, "w={w}: hit span sane");
            }
        }
    }

    #[test]
    fn full_rows_keep_derived_operator_signals_legible() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "signals"},
            "tasks": [
                {"id": "U", "title": "unowned", "kind": "impl", "state": "queued", "deps": [], "attempts": []},
                {"id": "Q", "title": "choice", "kind": "question", "owner": "operator", "state": "queued", "deps": [], "attempts": []}
            ]
        }))
        .unwrap();
        let scene = model::build(&doc, None, None, None, &model::ViewOpts::default());
        for (key, signal) in [("U", "unassigned"), ("Q", "needs answer")] {
            let row = scene.rows.iter().find(|row| row.key == key).unwrap();
            let rendered = full_row(row, 99, false).0;
            assert!(rendered.contains(signal), "{signal:?} was truncated: {rendered:?}");
        }
    }

    #[test]
    fn queued_retry_focuses_the_current_task_stub() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "retry"},
            "tasks": [{
                "id": "R", "title": "retry", "kind": "impl", "owner": "dev",
                "state": "queued", "deps": [], "attempts": [
                    {"id": "R·a1", "n": 1, "state": "failed"}
                ]
            }]
        }))
        .unwrap();
        let card = focus_card(&doc, "R", 48, None, &[]).join("\n");
        assert!(card.contains("R · READY"), "{card}");
        assert!(!card.contains("attempt 1 · FAILED"), "{card}");
    }

    #[test]
    fn focus_cards_share_derived_signals_and_projects_have_no_task_action() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "focus"},
            "projects": [{"id": "P", "title": "Core", "owner": "lead"}],
            "tasks": [
                {"id": "D", "title": "done", "kind": "impl", "project": "P",
                 "state": "done", "deps": [], "attempts": []},
                {"id": "R", "title": "ready", "kind": "impl", "project": "P",
                 "owner": "dev", "state": "queued", "deps": ["D"], "attempts": []},
                {"id": "W", "title": "waiting", "kind": "impl", "project": "P",
                 "owner": "dev", "state": "queued", "deps": ["R"], "attempts": []},
                {"id": "Q", "title": "question", "kind": "question", "project": "P",
                 "owner": "operator", "state": "queued", "deps": ["D"], "attempts": []}
            ]
        }))
        .unwrap();

        for (key, signal) in [("R", "READY"), ("W", "WAITING"), ("Q", "NEEDS_ANSWER")] {
            let card = focus_card(&doc, key, 48, None, &[]).join("\n");
            assert!(card.contains(signal), "{key}: {card}");
        }
        let project = focus_card(&doc, "project:P", 48, None, &[]).join("\n");
        assert!(project.contains("project P") && project.contains("Core"), "{project}");
        assert!(!project.contains("[m] message"), "projects are not task action targets: {project}");
    }

    #[test]
    fn selected_focus_card_exposes_a_message_hit_in_both_layouts() {
        let doc = sample();
        for w in [150usize, 72] {
            let initial = model::build(&doc, None, None, None, &model::ViewOpts::default());
            let selected = initial
                .rows
                .iter()
                .find(|r| r.selectable)
                .unwrap()
                .key
                .clone();
            let scene = model::build(
                &doc,
                Some(&selected),
                None,
                None,
                &model::ViewOpts::default(),
            );
            let frame = compose(
                &FrameInput {
                    doc: &doc,
                    scene: &scene,
                    selected: Some(&selected),
                    banner: None,
                    flash: None,
                    stale_min: None,
                    watching: false,
                    herdr: None,
                    prompt: None,
                    messages: &[],
                },
                w,
            );
            assert!(
                frame.hits.iter().any(|h| matches!(h.target, HitTarget::Message)),
                "w={w}: focus-card message action must be clickable"
            );
        }
    }

    #[test]
    fn selected_details_dock_full_width_below_the_graph_at_every_breakpoint() {
        let criteria = format!("{}CRITERIA-TAIL", "criterion-word ".repeat(20));
        let receipt = format!("{}RECEIPT-TAIL", "receipt-word ".repeat(20));
        let event = format!("{}EVENT-TAIL", "event-word ".repeat(20));
        let project_note = format!("{}PROJECT-TAIL", "project-word ".repeat(20));
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "detail-dock"},
            "generated_at": "2026-08-21T04:00:00Z",
            "projects": [{
                "id": "P", "title": "project", "note": project_note
            }],
            "tasks": [{
                "id": "LONG", "title": "selected task", "kind": "impl",
                "project": "P", "owner": "dev", "state": "done", "deps": [],
                "criteria": criteria,
                "attempts": [{
                    "id": "LONG·a1", "n": 1, "state": "done", "actor": "dev",
                    "started_at": "2026-08-21T03:55:00Z",
                    "ended_at": "2026-08-21T04:00:00Z",
                    "outcome": {"result": "done", "evidence": "verified", "receipt": receipt}
                }]
            }],
            "events": [{
                "at": "2026-08-21T04:00:00Z", "type": "note",
                "task": "LONG", "detail": event
            }]
        }))
        .unwrap();

        for w in [170usize, 120, 72, 20] {
            let scene = model::build(
                &doc,
                Some("LONG·a1"),
                None,
                None,
                &model::ViewOpts::default(),
            );
            let frame = compose(
                &FrameInput {
                    doc: &doc,
                    scene: &scene,
                    selected: Some("LONG·a1"),
                    banner: None,
                    flash: None,
                    stale_min: None,
                    watching: false,
                    herdr: None,
                    prompt: None,
                    messages: &[],
                },
                w,
            );
            let plain: Vec<String> = frame
                .lines
                .iter()
                .map(|line| crate::select::plain(line))
                .collect();
            let row_line = frame
                .hits
                .iter()
                .find_map(|hit| match &hit.target {
                    HitTarget::Row(key) if key == "LONG·a1" => Some(hit.line),
                    _ => None,
                })
                .expect("selected row hit");
            let card_start = plain
                .iter()
                .position(|line| line.contains("─ LONG·a1 ·"))
                .expect("focus-card heading");
            let card_end = plain[card_start..]
                .iter()
                .position(|line| line.ends_with('┘'))
                .map(|offset| card_start + offset)
                .expect("focus-card footer");
            let card = plain[card_start..=card_end].join("\n");

            assert!(card_start > row_line, "w={w}: detail must follow the graph");
            assert_eq!(frame.sel_line, Some(row_line), "w={w}: row remains the selection anchor");
            assert_eq!(
                card_start,
                frame.graph_end + 1,
                "w={w}: the detail dock starts after its breathing row"
            );
            assert_eq!(
                frame.detail_end,
                card_end + 1,
                "w={w}: the footer must not leak into the detail region"
            );
            assert!(plain[card_start].ends_with('┐'), "w={w}: right border must be present");
            for line in &plain[card_start..=card_end] {
                assert_eq!(
                    line.width(),
                    w - 1,
                    "w={w}: card reserves exactly the terminal auto-wrap cell: {line:?}"
                );
            }
            for tail in ["CRITERIA-TAIL", "RECEIPT-TAIL", "EVENT-TAIL"] {
                assert!(card.contains(tail), "w={w}: wrapped card lost {tail}:\n{card}");
            }
            assert!(
                !plain[..card_start].iter().any(|line| line.contains("[m] message")),
                "w={w}: focus content must not remain in the sidecar"
            );
            let message_hit = frame
                .hits
                .iter()
                .find(|hit| matches!(hit.target, HitTarget::Message))
                .expect("message action hit");
            assert!(message_hit.line > card_start, "w={w}: message action belongs to card");
            assert_eq!((message_hit.x0, message_hit.x1), (0, w - 1));
        }

        let project = focus_card(&doc, "project:P", 19, None, &[])
            .iter()
            .map(|line| crate::select::plain(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(project.contains("PROJECT-TAIL"), "project note must wrap completely:\n{project}");
    }

    fn inspector_doc() -> Doc {
        serde_json::from_value(serde_json::json!({
            "dagr": 3,
            "run": {"id": "inspector", "title": "Inspector"},
            "generated_at": "2026-08-21T10:30:00Z",
            "projects": [
                {"id": "APP", "title": "Application"},
                {"id": "API", "title": "API stream", "parent": "APP"}
            ],
            "tasks": [
                {"id": "PLAN", "title": "plan the API", "kind": "plan", "project": "API",
                 "state": "done", "deps": [], "attempts": [{
                    "id": "PLAN·a1", "n": 1, "state": "done", "actor": "planner",
                    "model": "sol5.6·xhigh", "started_at": "2026-08-21T09:00:00Z",
                    "ended_at": "2026-08-21T09:10:00Z",
                    "outcome": {"result": "done", "evidence": "verified", "receipt": "plan.md"}
                 }]},
                {"id": "BUILD", "title": "build the API", "kind": "impl", "project": "API",
                 "owner": "api-dev", "state": "working", "deps": ["PLAN"], "attempts": [{
                    "id": "BUILD·a1", "n": 1, "state": "working", "actor": "api-dev",
                    "model": "sol5.6·max", "started_at": "2026-08-21T10:00:00Z",
                    "progress": {"done": 3, "total": 7, "note": "handlers"}
                 }]},
                {"id": "TEST", "title": "test the API", "kind": "test", "project": "API",
                 "state": "queued", "deps": ["BUILD"], "attempts": []},
                {"id": "SHIP", "title": "ship from another scope", "kind": "ship",
                 "state": "queued", "deps": ["BUILD"], "attempts": []}
            ],
            "events": []
        }))
        .unwrap()
    }

    #[test]
    fn compact_inspector_is_a_fixed_panel_and_reserves_model_effort_without_collision() {
        let doc = inspector_doc();
        let escaped = compact_model_chip("sol\x1b·max", 11);
        assert!(escaped.width() <= 11);
        assert!(escaped.ends_with("·max"));
        for width in [72usize, 20] {
            let lines = compact_inspector(&doc, Some("BUILD·a1"), width, None, &[])
                .iter()
                .map(|line| crate::select::plain(line))
                .collect::<Vec<_>>();
            assert_eq!(lines.len(), 4);
            assert_eq!(
                lines[0],
                format!("╔{}╗", "═".repeat(width.saturating_sub(2))),
                "the top edge uses a distinct, uninterrupted panel grammar"
            );
            assert!(lines[1].starts_with('║') && lines[1].ends_with('║'));
            assert!(lines[2].starts_with('║') && lines[2].ends_with('║'));
            assert!(lines[3].starts_with('╚') && lines[3].ends_with('╝'));
            assert!(lines[1].contains("BUILD·a1"));
            if width >= 32 {
                assert!(lines[1].contains("WORKING"));
            } else {
                assert!(
                    lines[1].contains('◎'),
                    "the state glyph survives narrow mode"
                );
            }
            assert!(lines[2].contains("progress 3/7"));
            if width >= 32 {
                assert!(lines[2].contains("handlers"));
            }
            assert!(lines[3].contains("api-dev"), "width={width}: actor missing: {:?}", lines[3]);
            assert!(
                lines[3].contains("sol5.6·max"),
                "width={width}: model+effort must survive intact: {:?}",
                lines[3]
            );
            if width >= 32 {
                assert!(lines[3].contains("[sol5.6·max]"));
            }
            assert!(
                lines[3].find("api-dev").unwrap() < lines[3].find("sol5.6·max").unwrap(),
                "width={width}: independently anchored fields collided: {:?}",
                lines[3]
            );
            assert!(lines.iter().all(|line| line.width() == width));
        }

        let scene = model::build(
            &doc,
            Some("BUILD·a1"),
            None,
            None,
            &model::ViewOpts::default(),
        );
        let frame = compose_with_inspector(
            &FrameInput {
                doc: &doc,
                scene: &scene,
                selected: Some("BUILD·a1"),
                banner: None,
                flash: None,
                stale_min: None,
                watching: false,
                herdr: None,
                prompt: None,
                messages: &[],
            },
            72,
            InspectorMode::Compact,
        );
        assert_eq!(frame.detail_end - frame.graph_end, 4);
        assert_eq!(
            frame
                .hits
                .iter()
                .filter(|hit| matches!(hit.target, HitTarget::Details))
                .count(),
            4,
            "the whole compact inspector is a drill-down target"
        );
    }

    #[test]
    fn focus_lens_uses_declared_causality_and_project_breadcrumbs() {
        let doc = inspector_doc();
        let scene = model::build(
            &doc,
            Some("BUILD·a1"),
            None,
            None,
            &model::ViewOpts::default(),
        );
        let frame = compose_with_inspector(
            &FrameInput {
                doc: &doc,
                scene: &scene,
                selected: Some("BUILD·a1"),
                banner: None,
                flash: None,
                stale_min: None,
                watching: false,
                herdr: None,
                prompt: None,
                messages: &[],
            },
            78,
            InspectorMode::Focus,
        );
        let graph = frame.lines[..frame.graph_end]
            .iter()
            .map(|line| crate::select::plain(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(graph.contains("Application / API stream"), "scope breadcrumb:\n{graph}");
        assert!(graph.contains("inputs") && graph.contains("PLAN"), "upstream edge:\n{graph}");
        assert!(graph.contains("focus") && graph.contains("BUILD·a1"), "focus:\n{graph}");
        assert!(
            graph.contains("unlocks") && graph.contains("TEST") && graph.contains("SHIP"),
            "all direct dependents, including cross-project edges:\n{graph}"
        );
        assert!(frame.lines[frame.graph_end..].iter().any(|line| line.contains("[m] message")));
    }

    #[test]
    fn focus_wrapping_is_explicitly_bounded() {
        let (lines, clipped) = wrap_focus_text(&"wide-word ".repeat(100), 8, 3);
        assert_eq!(lines.len(), 3);
        assert!(clipped);
        assert!(lines.last().is_some_and(|line| line.ends_with('…')));
        assert!(lines.iter().all(|line| line.width() <= 8));
    }

    #[test]
    fn fold_chip_hit_precedes_the_row_hit_in_both_layouts() {
        let doc = sample();
        let base = model::build(&doc, None, None, None, &model::ViewOpts::default());
        let target = base
            .rows
            .iter()
            .find(|r| r.has_kids)
            .expect("sample has a branch")
            .key
            .clone();
        let mut folded = std::collections::HashSet::new();
        folded.insert(target.clone());
        for w in [150usize, 72] {
            let scene =
                model::build(&doc, None, None, None, &model::ViewOpts { zoom: None, folded: Some(&folded) });
            let f = compose(
                &FrameInput {
                    doc: &doc,
                    scene: &scene,
                    selected: None,
                    banner: None,
                    flash: None,
                    stale_min: None,
                    watching: false,
                    herdr: None,
                    prompt: None,
                    messages: &[],
                },
                w,
            );
            let fold_pos = f
                .hits
                .iter()
                .position(|h| matches!(&h.target, HitTarget::Fold(k) if *k == target))
                .unwrap_or_else(|| panic!("w={w}: folded row must expose a chip hit"));
            let row_pos = f
                .hits
                .iter()
                .position(|h| matches!(&h.target, HitTarget::Row(k) if *k == target))
                .expect("row hit");
            assert!(fold_pos < row_pos, "w={w}: the chip hit must win click resolution");
            assert_eq!(f.hits[fold_pos].line, f.hits[row_pos].line, "w={w}: same line");
        }
    }
}
