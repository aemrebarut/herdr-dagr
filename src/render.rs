//! Rows → ANSI frames. Two layouts, both established terminal grammars:
//! `sidecar` (≥ ~146 cols: trace left, attention queue right) folding to
//! `cockpit` (full-width trace). In both, the selected item's detail is
//! docked full-width below the graph. Renderers are pure functions of
//! (state, width).

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
            for (text, text_style) in [
                (project.title.as_deref(), Style::fg(style::TEXT)),
                (project.owner.as_deref(), Style::dim(style::MUTED)),
                (project.note.as_deref(), Style::dim(style::MUTED)),
            ] {
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    let mut line = Line::new(cw);
                    line.put(0, "│ ", Style::fg(col));
                    line.put(2, &trunc(text, cw.saturating_sub(4)), text_style);
                    line.put(cw.saturating_sub(2), " │", Style::fg(col));
                    lines.push(line.render(None, true));
                }
            }
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
    let inner = cw.saturating_sub(4);
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

    body.push(("[m] message orchestrator  [enter] focus pane".into(), Style::fg(style::ACCENT)));

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
}

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
    /// First line of the modal prompt block, when one was composed. The
    /// confirm gate only counts as shown if these lines were actually
    /// inside the viewport on the last draw.
    pub prompt_line: Option<usize>,
    /// Clickable regions of this frame (mouse support).
    pub hits: Vec<Hit>,
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
    if w >= FOLD_WIDTH {
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

    // A selection is explanatory content, not an attention-queue field.
    // Give it the complete frame at every breakpoint so adding terminal
    // width always reveals more criteria, receipts, and operator messages.
    if let Some(k) = sel {
        out.push(String::new());
        let card = focus_card(input.doc, k, w, input.herdr, input.messages);
        let card_start = out.len();
        if let Some(i) = card.iter().position(|line| line.contains("[m] message")) {
            hits.push(Hit {
                line: card_start + i,
                x0: 0,
                x1: w,
                target: HitTarget::Message,
            });
        }
        out.extend(card);
    } else if w < FOLD_WIDTH {
        // Preserve the cockpit's breathing room when nothing is selected.
        out.push(String::new());
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
        let full = "j/k move · ←/→ fold/zoom · tab queue · enter focus · m message · f open · ? help · q quit";
        let mid = "j/k · ←/→ · tab · enter · m message · f · ? · q";
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
    Frame { lines: out, sel_line, prompt_line, hits }
}

pub fn help_lines() -> Vec<String> {
    let rows = [
        ("j / k", "move the cursor through the trace"),
        ("→ / l", "zoom the trace to the selected branch (a folded row opens first)"),
        ("← / h", "fold the branch · on a folded or leaf row, jump to its parent"),
        ("z", "fold every settled branch — the trace shows what still needs you · again unfolds"),
        ("g / G", "top · bottom of the trace"),
        ("ctrl-d / ctrl-u", "half a screen down · up"),
        ("tab", "cycle attention (blocked → review/questions → working)"),
        ("enter", "focus the selected attempt's herdr pane (zoom-cycle)"),
        ("m", "message the orchestrator · Tab picks a starter · text stays editable"),
        ("ctrl-t in message", "toggle explicit authority: return to me ↔ may decide + continue"),
        ("f", "open another run file (type to filter · recent files first)"),
        ("/", "find a row by id, title, or agent · n/N cycle the matches"),
        ("y", "copy the selected row id to the clipboard"),
        ("r", "reload the run file now"),
        ("mouse", "click selects · double-click zooms · a ▸ chip click unfolds · wheel moves"),
        ("drag", "select text · copied to the clipboard when you let go"),
        ("?", "toggle this help"),
        ("q / esc", "quit (esc backs out of help and zoom first)"),
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
        let criteria = format!("{}DETAIL-TAIL-VISIBLE", "x".repeat(82));
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "detail-dock"},
            "tasks": [{
                "id": "LONG", "title": "selected task", "kind": "impl",
                "owner": "dev", "state": "done", "deps": [],
                "criteria": criteria, "attempts": []
            }]
        }))
        .unwrap();

        for w in [170usize, 120, 72] {
            let scene = model::build(
                &doc,
                Some("LONG"),
                None,
                None,
                &model::ViewOpts::default(),
            );
            let frame = compose(
                &FrameInput {
                    doc: &doc,
                    scene: &scene,
                    selected: Some("LONG"),
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
                    HitTarget::Row(key) if key == "LONG" => Some(hit.line),
                    _ => None,
                })
                .expect("selected row hit");
            let card_start = plain
                .iter()
                .position(|line| line.contains("─ LONG ·"))
                .expect("focus-card heading");

            assert!(card_start > row_line, "w={w}: detail must follow the graph");
            assert_eq!(frame.sel_line, Some(row_line), "w={w}: row remains the selection anchor");
            assert_eq!(
                plain[card_start].width(),
                w,
                "w={w}: focus card must claim the complete frame"
            );
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
            assert_eq!((message_hit.x0, message_hit.x1), (0, w));

            if w >= 120 {
                assert!(
                    plain.iter().any(|line| line.contains("DETAIL-TAIL-VISIBLE")),
                    "w={w}: width beyond the former card cap must reveal the criteria tail"
                );
            }
        }
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
