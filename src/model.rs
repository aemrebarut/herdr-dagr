//! Contract → render model. Where a hand-painted mock fixes its scene,
//! here every row is *derived*
//! from contract data — tree shape from primary deps, attempt nesting from
//! cause chains (a fix round nests under the review that sent it back),
//! dotted futures from declared policy, fan-in chips from gate inputs.
//! The renderer invents nothing (CONTRACT.md, non-goals).

use crate::contract::{Attempt, Doc, Task};
use crate::herdr::Hints;
use crate::style::{self, Style};

#[derive(Clone)]
pub struct Seg(pub String, pub Style);

#[derive(Clone)]
pub struct Row {
    /// Selection identity: attempt id, task id, or future node id.
    pub key: String,
    pub task_id: String,
    pub rail: String,
    pub dotted: bool,
    /// Paint ↩ over the rail's branch position: loop re-entry.
    pub reentry: bool,
    pub glyph: char,
    pub glyph_color: u8,
    pub hot: bool,
    pub name: String,
    pub title: String,
    pub title_dim: bool,
    /// Fan-in chips / inline annotations, placed after the title.
    pub chips: Vec<Seg>,
    pub model: String,
    pub status: Vec<Seg>,
    pub agent: String,
    pub selectable: bool,
    pub lit: bool,
    pub tag: Option<String>,
    pub state: String,
    /// Row key of the parent row in the walk (`None` for roots) — ← jumps here.
    pub parent: Option<String>,
    /// The walk put child rows under this one (foldable / zoomable).
    pub has_kids: bool,
    /// Present when the subtree is folded shut under this row.
    pub fold: Option<FoldChip>,
}

/// The chip a folded row carries: how much is hidden, and any attention
/// states inside — folding compresses history, it must not hide an alarm.
#[derive(Clone)]
pub struct FoldChip {
    pub hidden: usize,
    /// Strongest attention color inside the fold (compact rows tint the
    /// ▸ marker with it).
    pub hot: Option<u8>,
    pub segs: Vec<Seg>,
}

impl Row {
    fn blank(key: &str, task_id: &str, state: &str) -> Self {
        Row {
            key: key.into(),
            task_id: task_id.into(),
            rail: String::new(),
            dotted: false,
            reentry: false,
            glyph: '·',
            glyph_color: style::MUTED,
            hot: false,
            name: String::new(),
            title: String::new(),
            title_dim: false,
            chips: Vec::new(),
            model: String::new(),
            status: Vec::new(),
            agent: String::new(),
            selectable: false,
            lit: false,
            tag: None,
            state: state.into(),
            parent: None,
            has_kids: false,
            fold: None,
        }
    }
}

pub struct QueueItem {
    pub task_id: String,
    pub state: String,
    pub label: String,
    pub minutes: i64,
    pub who: String,
}

pub struct Scene {
    pub rows: Vec<Row>,
    pub queue: Vec<QueueItem>,
    pub run_title: String,
    pub run_meta: String,
    /// The selection normalized to its owning task id — attempt keys and
    /// future-node keys resolve to the task, so panels that think in tasks
    /// (the attention queue) highlight correctly (gpt F18).
    pub selected_task: Option<String>,
    /// Present while the trace is re-rooted at one branch.
    pub zoom: Option<ZoomNote>,
}

/// Zoom state the renderer surfaces: which row the trace is re-rooted at,
/// and how many attention items live OUTSIDE the zoom — a zoom narrows the
/// view, it must not hide an alarm silently.
pub struct ZoomNote {
    pub root: String,
    pub outside: usize,
}

/// Interactive view state that shapes the scene: zoom re-roots the walk at
/// one row, folds collapse subtrees into their top row. Snapshots and
/// tests pass `&ViewOpts::default()`.
#[derive(Default)]
pub struct ViewOpts<'a> {
    pub zoom: Option<&'a str>,
    pub folded: Option<&'a std::collections::HashSet<String>>,
}

/// Minutes since epoch from an ISO-8601 timestamp. Real validation, not a
/// shape check: calendar ranges (leap years included), optional seconds
/// and fraction, and a `Z` / `±HH:MM` offset that is normalized into UTC
/// before any arithmetic. `None` for anything invalid — the renderer then
/// claims no duration rather than a wrong one, and `dagr check` E180
/// rides the same parser so the two can never disagree.
pub fn parse_min(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 16 {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = ts.get(r)?;
        if s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None }
    };
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' {
        return None;
    }
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hh, mm) = (digits(11..13)?, digits(14..16)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if !(1..=mdays).contains(&d) || hh > 23 || mm > 59 {
        return None;
    }
    // optional :SS(.fraction), then Z / ±HH:MM / nothing (bare = UTC)
    let mut i = 16;
    if b.get(i) == Some(&b':') {
        let ss = digits(i + 1..i + 3)?;
        if ss > 60 {
            return None; // leap second tolerated, 61+ is not
        }
        i += 3;
        if b.get(i) == Some(&b'.') {
            i += 1;
            let frac = i;
            while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
            if i == frac {
                return None;
            }
        }
    }
    let off_min: i64 = match b.get(i) {
        None => 0,
        Some(b'Z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = digits(i + 1..i + 3)?;
            if b.get(i + 3) != Some(&b':') || i + 6 != b.len() {
                return None;
            }
            let om = digits(i + 4..i + 6)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let v = oh * 60 + om;
            if *sign == b'+' { v } else { -v }
        }
        _ => return None,
    };
    // days_from_civil (Howard Hinnant), no external deps
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 1440 + hh * 60 + mm - off_min)
}

fn clock(ts: &str) -> &str {
    ts.get(11..16).unwrap_or(ts)
}

/// Bounded streak marks. Streaks are small in practice; a hostile value
/// must render as ink, not as a terminal-width (or memory-width) of glyphs.
pub fn streak_marks(streak: Option<u64>) -> String {
    let s = streak.unwrap_or(1).max(1);
    if s <= 3 {
        "✗".repeat(s as usize)
    } else {
        format!("✗×{s}")
    }
}

// ── node graph over attempts ────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NodeRef {
    A(usize, usize), // task index, attempt index
    T(usize),        // task with no attempts (stub row)
}

struct Ix<'a> {
    tasks: &'a [Task],
    task_by_id: std::collections::HashMap<&'a str, usize>,
    attempt_by_id: std::collections::HashMap<&'a str, (usize, usize)>,
    /// attempt order per task, sorted by n
    order: Vec<Vec<usize>>,
}

impl<'a> Ix<'a> {
    fn new(doc: &'a Doc) -> Self {
        let tasks: &[Task] = doc.tasks.as_deref().unwrap_or(&[]);
        let mut task_by_id = std::collections::HashMap::new();
        let mut attempt_by_id = std::collections::HashMap::new();
        let mut order = Vec::new();
        for (ti, t) in tasks.iter().enumerate() {
            if let Some(id) = t.id.as_deref() {
                task_by_id.entry(id).or_insert(ti);
            }
            let mut idx: Vec<usize> = (0..t.attempts.len()).collect();
            idx.sort_by_key(|&ai| t.attempts[ai].n.unwrap_or(0));
            for &ai in &idx {
                if let Some(id) = t.attempts[ai].id.as_deref() {
                    attempt_by_id.entry(id).or_insert((ti, ai));
                }
            }
            order.push(idx);
        }
        Ix { tasks, task_by_id, attempt_by_id, order }
    }

    fn attempt(&self, n: NodeRef) -> Option<&'a Attempt> {
        match n {
            NodeRef::A(ti, ai) => Some(&self.tasks[ti].attempts[ai]),
            NodeRef::T(_) => None,
        }
    }

    fn task_of(&self, n: NodeRef) -> &'a Task {
        match n {
            NodeRef::A(ti, _) | NodeRef::T(ti) => &self.tasks[ti],
        }
    }

    fn latest_attempt(&self, ti: usize) -> Option<NodeRef> {
        self.order[ti].last().map(|&ai| NodeRef::A(ti, ai))
    }

    /// The dep task's attempt that was current when `started` — the row a
    /// first attempt hangs from.
    fn dep_attempt_at(&self, ti: usize, started: Option<i64>) -> Option<NodeRef> {
        let ord = &self.order[ti];
        if ord.is_empty() {
            return None;
        }
        if let Some(s) = started {
            let mut best = ord[0];
            for &ai in ord {
                let a_start = self.tasks[ti].attempts[ai].started_at.as_deref().and_then(parse_min);
                match a_start {
                    Some(t0) if t0 <= s => best = ai,
                    _ => {}
                }
            }
            return Some(NodeRef::A(ti, best));
        }
        self.latest_attempt(ti)
    }

    fn start_of(&self, n: NodeRef) -> i64 {
        match n {
            NodeRef::A(ti, ai) => self.tasks[ti].attempts[ai]
                .started_at
                .as_deref()
                .and_then(parse_min)
                .unwrap_or(i64::MAX - 1),
            NodeRef::T(_) => i64::MAX, // queued stubs sort last
        }
    }

    fn key_of(&self, n: NodeRef) -> String {
        match n {
            NodeRef::A(ti, ai) => self.tasks[ti].attempts[ai]
                .id
                .clone()
                .unwrap_or_else(|| format!("?a{ti}.{ai}")),
            NodeRef::T(ti) => self.tasks[ti].id.clone().unwrap_or_else(|| format!("?t{ti}")),
        }
    }

    /// Display name: first attempt shows the bare task id, later attempts
    /// their `T·aN` id (the reference grammar).
    fn display_name(&self, n: NodeRef) -> String {
        match n {
            NodeRef::A(ti, ai) => {
                let a = &self.tasks[ti].attempts[ai];
                if a.n.unwrap_or(1) <= 1 {
                    self.tasks[ti].id.clone().unwrap_or_default()
                } else {
                    a.id.clone().unwrap_or_default()
                }
            }
            NodeRef::T(ti) => self.tasks[ti].id.clone().unwrap_or_default(),
        }
    }

    /// A gate hangs from the *latest-started* of its fan-in inputs — the
    /// row nearest it in time, so the ← chips sit below the work they sum
    /// — not from deps[0]. Ties break on key; inputs with no
    /// attempts rank behind attempted ones, and their stub rows are used
    /// only when nothing in the fan-in has run yet.
    fn gate_parent(&self, task: &Task) -> Option<NodeRef> {
        let inputs = task.inputs.as_ref().unwrap_or(&task.deps);
        let mut best: Option<(i64, String, NodeRef)> = None;
        let mut stub: Option<NodeRef> = None;
        for input in inputs {
            let Some(&ti) = self.task_by_id.get(input.as_str()) else { continue };
            match self.latest_attempt(ti) {
                Some(n) => {
                    let cand = (self.start_of(n), self.key_of(n), n);
                    if best.as_ref().map(|b| (cand.0, &cand.1) > (b.0, &b.1)).unwrap_or(true) {
                        best = Some(cand);
                    }
                }
                None => stub = stub.or(Some(NodeRef::T(ti))),
            }
        }
        best.map(|(_, _, n)| n).or(stub)
    }

    /// Parent node: cause chain first, then fan-in (gates), then primary dep.
    fn parent(&self, n: NodeRef) -> Option<NodeRef> {
        let task = self.task_of(n);
        let is_gate = task.kind.as_deref() == Some("gate");
        if let Some(a) = self.attempt(n) {
            if let Some(cause) = &a.cause {
                if let Some(r) = cause.reference.as_deref() {
                    if let Some(&(ti, ai)) = self.attempt_by_id.get(r) {
                        return Some(NodeRef::A(ti, ai));
                    }
                    if let Some(&ti) = self.task_by_id.get(r) {
                        return self.latest_attempt(ti);
                    }
                }
            }
            if is_gate {
                if let Some(p) = self.gate_parent(task) {
                    return Some(p);
                }
            }
            let started = a.started_at.as_deref().and_then(parse_min);
            if let Some(dep) = task.deps.first() {
                if let Some(&ti) = self.task_by_id.get(dep.as_str()) {
                    return self.dep_attempt_at(ti, started);
                }
            }
            return None;
        }
        // task stub
        if is_gate {
            if let Some(p) = self.gate_parent(task) {
                return Some(p);
            }
        }
        if let Some(dep) = task.deps.first() {
            if let Some(&ti) = self.task_by_id.get(dep.as_str()) {
                return self.latest_attempt(ti);
            }
        }
        None
    }
}

// ── scene build ─────────────────────────────────────────────────────

/// The node forest the walk draws: every attempt (or task stub), its
/// children, and the roots — shared by `build`, `settled_roots`, and
/// nothing else, so the two can never disagree about tree shape.
struct Forest {
    all: Vec<NodeRef>,
    children: Vec<Vec<NodeRef>>,
    index: std::collections::HashMap<NodeRef, usize>,
    roots: Vec<NodeRef>,
}

fn forest(ix: &Ix) -> Forest {
    let mut all: Vec<NodeRef> = Vec::new();
    for (ti, t) in ix.tasks.iter().enumerate() {
        if t.attempts.is_empty() {
            all.push(NodeRef::T(ti));
        } else {
            for &ai in &ix.order[ti] {
                all.push(NodeRef::A(ti, ai));
            }
        }
    }
    let mut children: Vec<Vec<NodeRef>> = vec![Vec::new(); all.len()];
    let index: std::collections::HashMap<NodeRef, usize> =
        all.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut roots: Vec<NodeRef> = Vec::new();
    for &n in &all {
        match ix.parent(n) {
            Some(p) if p != n => children[index[&p]].push(n),
            _ => roots.push(n),
        }
    }
    for kids in &mut children {
        kids.sort_by_key(|&k| (ix.start_of(k), ix.key_of(k)));
    }
    roots.sort_by_key(|&k| (ix.start_of(k), ix.key_of(k)));
    Forest { all, children, index, roots }
}

/// The state a row DISPLAYS for attention purposes: the attempt's state,
/// with the task's blocked/review outranking a live working/queued — the
/// same projection `node_row` draws. Fold summaries and the settled check
/// ride this, so a fold can never hide what the trace would have shown.
fn effective_state(ix: &Ix, n: NodeRef) -> String {
    match ix.attempt(n) {
        Some(a) => {
            let st = a.state.as_deref().unwrap_or("queued");
            if matches!(st, "working" | "queued") {
                if let Some(ts @ ("blocked" | "review")) = ix.task_of(n).state.as_deref() {
                    return ts.to_string();
                }
            }
            st.to_string()
        }
        None => ix.task_of(n).state.as_deref().unwrap_or("queued").to_string(),
    }
}

/// alarms: [blocked, lost, review, settled_unverified, working]
fn fold_chip(hidden: usize, alarms: &[usize; 5]) -> FoldChip {
    let mut segs = vec![
        Seg("▸ ".into(), Style::bold(style::ACCENT)),
        Seg(format!("{hidden} hidden"), Style::dim(style::MUTED)),
    ];
    let mut hot = None;
    let cats = [
        ("blocked", alarms[0]),
        ("lost", alarms[1]),
        ("review", alarms[2]),
        ("settled_unverified", alarms[3]),
        ("working", alarms[4]),
    ];
    for (state, count) in cats {
        if count == 0 {
            continue;
        }
        let col = style::state_color(state);
        let label = if state == "settled_unverified" { "unverified" } else { state };
        // working is activity, not an alarm — it shows, but dim
        let live = state != "working";
        if live && hot.is_none() {
            hot = Some(col);
        }
        segs.push(Seg(
            format!(" · {} {count} {label}", style::state_glyph(state)),
            if live { Style::bold(col) } else { Style::dim(col) },
        ));
    }
    FoldChip { hidden, hot, segs }
}

/// Row keys of the topmost all-settled subtrees — branches where the node
/// and every descendant is done/failed/rejected: history, nothing live or
/// waiting. These are what `z` folds; attention states never fold away.
pub fn settled_roots(doc: &Doc) -> Vec<String> {
    fn settled(ix: &Ix, f: &Forest, n: NodeRef) -> bool {
        matches!(effective_state(ix, n).as_str(), "done" | "failed" | "rejected")
            && f.children[f.index[&n]].iter().all(|&k| settled(ix, f, k))
    }
    fn collect(ix: &Ix, f: &Forest, n: NodeRef, out: &mut Vec<String>) {
        if settled(ix, f, n) {
            // a settled leaf has nothing to fold — only branches count
            if !f.children[f.index[&n]].is_empty() {
                out.push(ix.key_of(n));
            }
        } else {
            for &k in &f.children[f.index[&n]] {
                collect(ix, f, k, out);
            }
        }
    }
    let ix = Ix::new(doc);
    let f = forest(&ix);
    let mut out = Vec::new();
    for &r in &f.roots {
        collect(&ix, &f, r, &mut out);
    }
    out
}

/// Row keys of `key`'s ancestors, nearest first — the fold path that must
/// open for the row to be visible. Cycle-guarded against malformed docs.
pub fn ancestors(doc: &Doc, key: &str) -> Vec<String> {
    let ix = Ix::new(doc);
    let mut cur = ix
        .attempt_by_id
        .get(key)
        .map(|&(ti, ai)| NodeRef::A(ti, ai))
        .or_else(|| {
            ix.task_by_id
                .get(key)
                .map(|&ti| ix.latest_attempt(ti).unwrap_or(NodeRef::T(ti)))
        });
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = cur {
        if !seen.insert(n) || out.len() > 64 {
            break;
        }
        match ix.parent(n) {
            Some(p) if p != n => {
                out.push(ix.key_of(p));
                cur = Some(p);
            }
            _ => break,
        }
    }
    out
}

/// Does this row key still name a task or attempt in the document?
/// Zoom stacks and fold sets are pruned with this across reloads.
pub fn key_exists(doc: &Doc, key: &str) -> bool {
    let ix = Ix::new(doc);
    ix.attempt_by_id.contains_key(key) || ix.task_by_id.contains_key(key)
}

/// Does a row answer this search? Matched fields: row key, display name,
/// title, agent, and owning task id.
pub fn row_matches(r: &Row, q: &str) -> bool {
    let q = q.to_lowercase();
    [&r.key, &r.name, &r.title, &r.agent, &r.task_id]
        .into_iter()
        .any(|s| s.to_lowercase().contains(&q))
}

/// `flow_chip` is the precomputed `stats::header_chip` for this doc —
/// computed once per (re)load by the caller, not per frame: the full
/// flow DFS has no business running 3×/s for a number that only changes
/// when the document does.
pub fn build(
    doc: &Doc,
    selected: Option<&str>,
    hints: Option<&Hints>,
    flow_chip: Option<&str>,
    opts: &ViewOpts,
) -> Scene {
    let ix = Ix::new(doc);
    let tasks = ix.tasks;

    // header
    let (run_title, run_meta) = match &doc.run {
        Some(r) => {
            let mut meta = r.id.clone().unwrap_or_default();
            if let Some(s) = r.started_at.as_deref() {
                meta.push_str(&format!(" · started {}", clock(s)));
            }
            (r.title.clone().unwrap_or_else(|| "(untitled run)".into()), meta)
        }
        None => ("(no run block)".into(), String::new()),
    };
    let now_min = doc.generated_at.as_deref().and_then(parse_min);
    let started_min = doc
        .run
        .as_ref()
        .and_then(|r| r.started_at.as_deref())
        .and_then(parse_min);
    let run_meta = match (now_min, started_min) {
        (Some(n), Some(s)) if n >= s => format!("{run_meta} · now +{}m", n - s),
        _ => run_meta,
    };
    // flow chip (M4): WIP / blocked / rework / naive ETA, from the same
    // timestamps the focus card uses — queries, not new state
    let run_meta = match flow_chip {
        Some(f) => format!("{run_meta} · {f}"),
        None => run_meta,
    };

    // selection → owning task id (for lighting + future unroll)
    let selected_task: Option<String> = selected.and_then(|k| {
        if let Some(&(ti, _)) = ix.attempt_by_id.get(k) {
            tasks[ti].id.clone()
        } else if ix.task_by_id.contains_key(k) {
            Some(k.to_string())
        } else {
            // future node ids select their owning task
            for t in tasks {
                if let Some(p) = &t.policy {
                    for f in &p.futures {
                        if f.node.as_ref().and_then(|n| n.id.as_deref()) == Some(k) {
                            return t.id.clone();
                        }
                    }
                }
            }
            None
        }
    });

    // nodes + children
    let Forest { all, children, index, roots } = forest(&ix);

    // zoom: re-root the walk at one row; an unknown key draws the full run
    let zoom_node = opts.zoom.and_then(|z| all.iter().copied().find(|&n| ix.key_of(n) == z));
    let walk_roots: Vec<NodeRef> = match zoom_node {
        Some(n) => vec![n],
        None => roots,
    };

    let no_folds = std::collections::HashSet::new();
    let folded = opts.folded.unwrap_or(&no_folds);
    // every node the walk accounted for — emitted as a row OR hidden
    // inside a fold — so the unreachable-node fallback below cannot
    // resurrect folded work as flat rows
    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut rows: Vec<Row> = Vec::new();
    struct Walker<'a, 'b> {
        ix: &'b Ix<'a>,
        children: &'b [Vec<NodeRef>],
        index: &'b std::collections::HashMap<NodeRef, usize>,
        rows: &'b mut Vec<Row>,
        selected_task: Option<&'b str>,
        now_min: Option<i64>,
        hints: Option<&'b Hints>,
        folded: &'b std::collections::HashSet<String>,
        covered: &'b mut std::collections::HashSet<String>,
    }
    impl<'a, 'b> Walker<'a, 'b> {
        fn pos(&self, n: NodeRef) -> usize {
            self.index[&n]
        }
        /// Mark a folded-away subtree as covered while tallying what the
        /// fold hides — count and attention states, for the ▸ chip.
        fn consume(&mut self, n: NodeRef, hidden: &mut usize, alarms: &mut [usize; 5]) {
            for k in self.children[self.pos(n)].clone() {
                self.covered.insert(self.ix.key_of(k));
                *hidden += 1;
                match effective_state(self.ix, k).as_str() {
                    "blocked" => alarms[0] += 1,
                    "lost" => alarms[1] += 1,
                    "review" => alarms[2] += 1,
                    "settled_unverified" => alarms[3] += 1,
                    "working" => alarms[4] += 1,
                    _ => {}
                }
                self.consume(k, hidden, alarms);
            }
        }
        fn walk(&mut self, n: NodeRef, prefix: &str, is_root: bool, is_last: bool, parent: Option<&str>) {
            let rail = if is_root {
                " ".to_string()
            } else {
                format!("{prefix}{}─", if is_last { '╰' } else { '├' })
            };
            let reentry = self
                .ix
                .attempt(n)
                .and_then(|a| a.cause.as_ref())
                .and_then(|c| c.cause_type.as_deref())
                .map(|t| t == "sent_back" || t == "gate_failed")
                .unwrap_or(false);
            let key = self.ix.key_of(n);
            self.covered.insert(key.clone());
            self.rows.push(node_row(self.ix, n, &rail, reentry, self.now_min, self.hints));
            self.rows.last_mut().expect("row just pushed").parent = parent.map(str::to_string);

            let child_prefix = if is_root {
                prefix.to_string()
            } else {
                format!("{prefix}{}", if is_last { "  " } else { "│ " })
            };

            // dotted children first: futures and fan-in unroll. Stubs are
            // earned — a working attempt or an externally-blocked task
            // speculates; queued and settled nodes show no futures (F9).
            // Gates unroll their fan-in when selected whether or not an
            // attempt exists (F2).
            let task = self.ix.task_of(n);
            let tid = task.id.as_deref().unwrap_or("");
            let is_selected_task = self.selected_task == Some(tid);
            let is_gate = task.kind.as_deref() == Some("gate");
            let blocked = task.state.as_deref() == Some("blocked");
            let kids = self.children[self.pos(n)].clone();
            self.rows.last_mut().expect("row just pushed").has_kids = !kids.is_empty();
            // a folded branch collapses into its top row: children and
            // futures stay hidden, the ▸ chip says how much and keeps any
            // alarm visible — a fold must never hide a blocked row silently
            if !kids.is_empty() && self.folded.contains(&key) {
                let mut hidden = 0usize;
                let mut alarms = [0usize; 5];
                self.consume(n, &mut hidden, &mut alarms);
                self.rows.last_mut().expect("row just pushed").fold =
                    Some(fold_chip(hidden, &alarms));
                return;
            }
            let mut dotted: Vec<Row> = Vec::new();
            if let Some(a) = self.ix.attempt(n) {
                let earned = a.state.as_deref() == Some("working") || blocked;
                let is_latest = self.ix.latest_attempt(self.pos_task(n)) == Some(n);
                if earned && is_latest {
                    dotted = future_rows(self.ix, task, &child_prefix, is_selected_task);
                }
                if is_gate && is_selected_task && is_latest {
                    dotted.extend(fanin_rows(self.ix, task, &child_prefix));
                }
            } else {
                if blocked {
                    dotted = future_rows(self.ix, task, &child_prefix, is_selected_task);
                }
                if is_gate && is_selected_task {
                    dotted.extend(fanin_rows(self.ix, task, &child_prefix));
                }
            }

            for (j, &k) in kids.iter().enumerate() {
                let last = j == kids.len() - 1 && dotted.is_empty();
                self.walk(k, &child_prefix, false, last, Some(&key));
            }
            self.rows.extend(dotted);
        }
        fn pos_task(&self, n: NodeRef) -> usize {
            match n {
                NodeRef::A(ti, _) | NodeRef::T(ti) => ti,
            }
        }
    }
    {
        let mut w = Walker {
            ix: &ix,
            children: &children,
            index: &index,
            rows: &mut rows,
            selected_task: selected_task.as_deref(),
            now_min,
            hints,
            folded,
            covered: &mut covered,
        };
        for (i, &r) in walk_roots.iter().enumerate() {
            w.walk(r, " ", true, i == walk_roots.len() - 1, None);
        }
    }
    // Malformed graphs (parent cycles) leave nodes unreachable from any
    // root. They still exist: emit them flat so the contract-error banner
    // has visible rows to explain, instead of silently dropping work.
    // (Skipped under zoom: out-of-subtree nodes are excluded on purpose.)
    if zoom_node.is_none() {
        for &n in &all {
            if !covered.contains(&ix.key_of(n)) {
                rows.push(node_row(&ix, n, " ", false, now_min, hints));
            }
        }
    }

    // lighting: accent ink between the selection and its dependency edges
    if let Some(sel_tid) = selected_task.as_deref() {
        let sel_task = ix.task_by_id.get(sel_tid).map(|&ti| &tasks[ti]);
        let is_gate = sel_task.and_then(|t| t.kind.as_deref()) == Some("gate");
        if is_gate {
            // light each input's last real row, tagged with the gate id
            let inputs: Vec<String> = sel_task
                .map(|t| t.inputs.clone().unwrap_or_else(|| t.deps.clone()))
                .unwrap_or_default();
            for input in &inputs {
                if let Some(row) = rows
                    .iter_mut()
                    .rev()
                    .find(|r| &r.task_id == input && !r.dotted)
                {
                    row.lit = true;
                    row.tag = Some(format!("» {sel_tid}"));
                }
            }
        } else {
            // light gates this selection feeds, and » refs naming it
            for (ti, t) in tasks.iter().enumerate() {
                let gate = t.kind.as_deref() == Some("gate");
                let unmet = t.state.as_deref() != Some("done");
                let inputs = t.inputs.clone().unwrap_or_else(|| t.deps.clone());
                if gate && unmet && inputs.iter().any(|i| i == sel_tid) {
                    let gid = tasks[ti].id.as_deref().unwrap_or("");
                    for row in rows.iter_mut().filter(|r| r.task_id == gid && !r.dotted) {
                        row.lit = true;
                    }
                }
            }
        }
        // any dotted » reference row pointing at the selection lights too
        for row in rows.iter_mut() {
            if row.dotted && row.glyph == '»' && row.name == sel_tid {
                row.lit = true;
            }
        }
    }

    // attention queue: blocked → lost → review → working
    let rank = |s: &str| match s {
        "blocked" => 0,
        "lost" => 1,
        "settled_unverified" => 2,
        "review" => 3,
        "working" => 4,
        _ => 9,
    };
    let mut queue: Vec<QueueItem> = tasks
        .iter()
        .filter_map(|t| {
            let last = ix
                .task_by_id
                .get(t.id.as_deref().unwrap_or(""))
                .and_then(|&ti| ix.order[ti].last().map(|&ai| &t.attempts[ai]));
            // attention state: `lost` and `settled_unverified` are attempt
            // facts — project the latest attempt's alarm over the task state
            let state = match last.and_then(|a| a.state.as_deref()) {
                Some(s @ ("lost" | "settled_unverified")) => s.to_string(),
                _ => t.state.as_deref().unwrap_or("").to_string(),
            };
            if rank(&state) >= 9 {
                return None;
            }
            Some((t, last, state))
        })
        .map(|(t, last, state)| {
            let started = last
                .and_then(|a| a.started_at.as_deref())
                .and_then(parse_min);
            let minutes = match (now_min, started) {
                (Some(n), Some(s)) if n >= s => n - s,
                _ => 0,
            };
            let who = match state.as_str() {
                "blocked" => format!("→ {}", t.unblock.as_deref().unwrap_or("?")),
                "review" => format!("→ {}", t.owner.as_deref().unwrap_or("?")),
                _ => t.owner.clone().unwrap_or_default(),
            };
            QueueItem {
                task_id: t.id.clone().unwrap_or_default(),
                state,
                label: t.title.clone().unwrap_or_default(),
                minutes,
                who,
            }
        })
        .collect();
    queue.sort_by_key(|q| (rank(&q.state), -q.minutes));

    // zoom narrows the queue to the subtree — but attention outside the
    // zoom is counted, never dropped silently
    let zoom = zoom_node.map(|zn| {
        let mut keep = std::collections::HashSet::new();
        let mut stack = vec![zn];
        while let Some(n) = stack.pop() {
            if let Some(id) = ix.task_of(n).id.clone() {
                keep.insert(id);
            }
            stack.extend(children[index[&n]].iter().copied());
        }
        let outside = queue
            .iter()
            .filter(|q| !keep.contains(&q.task_id))
            .filter(|q| {
                matches!(q.state.as_str(), "blocked" | "lost" | "review" | "settled_unverified")
            })
            .count();
        queue.retain(|q| keep.contains(&q.task_id));
        ZoomNote { root: ix.key_of(zn), outside }
    });

    Scene { rows, queue, run_title, run_meta, selected_task, zoom }
}

// ── row constructors ────────────────────────────────────────────────

fn ev_seg(evidence: Option<&str>) -> Seg {
    let (g, c) = style::evidence(evidence.unwrap_or("asserted"));
    Seg(g.to_string(), Style::fg(c))
}

fn node_row(
    ix: &Ix,
    n: NodeRef,
    rail: &str,
    reentry: bool,
    now_min: Option<i64>,
    hints: Option<&Hints>,
) -> Row {
    let task = ix.task_of(n);
    let tid = task.id.as_deref().unwrap_or("?");
    let kind = task.kind.as_deref().unwrap_or("");
    let mut row = Row::blank(&ix.key_of(n), tid, "");
    row.rail = rail.to_string();
    row.reentry = reentry;
    row.selectable = true;
    row.name = ix.display_name(n);
    row.title = task.title.clone().unwrap_or_default();

    if let Some(a) = ix.attempt(n) {
        let st = a.state.as_deref().unwrap_or("queued");
        row.state = st.to_string();
        row.model = a.model.clone().unwrap_or_default();
        row.agent = a.actor.clone().unwrap_or_default();
        row.glyph = style::state_glyph(st);
        row.glyph_color = style::state_color(st);
        row.hot = matches!(st, "working" | "blocked" | "lost");
        let ev = a.outcome.as_ref().and_then(|o| o.evidence.as_deref());
        match st {
            "done" => {
                let word = if matches!(kind, "review" | "test") { "pass " } else { "done " };
                row.status = vec![Seg(word.into(), Style::fg(style::DONE)), ev_seg(ev)];
            }
            "failed" => {
                let col = if kind == "review" { style::REJECTED } else { style::FAILED };
                row.glyph_color = col;
                row.status = vec![Seg("fail ".into(), Style::fg(col)), ev_seg(ev)];
                row.title_dim = true;
                // name the wound inline, dimmed, like the reference
                if let Some(reason) = a.outcome.as_ref().and_then(|o| o.reason.as_deref()) {
                    row.title = format!("{} · {}", row.title, reason);
                }
            }
            "rejected" => {
                row.status = vec![Seg("sent back".into(), Style::bold(style::REJECTED))];
                row.title_dim = true;
            }
            "working" => {
                let mut segs = vec![Seg("working".into(), Style::bold(style::WORKING))];
                if let (Some(nw), Some(lo)) = (
                    now_min,
                    a.liveness
                        .as_ref()
                        .and_then(|l| l.last_output_at.as_deref())
                        .and_then(parse_min),
                ) {
                    if nw - lo >= 5 {
                        segs.push(Seg(
                            format!(" {}m silent", nw - lo),
                            Style::bold(style::BLOCKED),
                        ));
                    }
                }
                row.status = segs; // progress lives in the focus card
            }
            "lost" => {
                row.status = vec![Seg("LOST".into(), Style::bold(style::BLOCKED))];
            }
            "settled_unverified" => {
                row.status = vec![Seg("unverified".into(), Style::fg(style::EV_HEURISTIC))];
            }
            _ => {
                row.status = vec![Seg("queued".into(), Style::dim(style::QUEUED))];
            }
        }
        // the task's blocked/review outranks a live attempt's working/queued
        // for display (CONTRACT §1): the block is the fact that needs eyes,
        // and a review row must say who holds the ball
        if matches!(st, "working" | "queued") {
            match task.state.as_deref() {
                Some("blocked") => {
                    row.glyph = style::state_glyph("blocked");
                    row.glyph_color = style::state_color("blocked");
                    row.hot = true;
                    row.status = vec![
                        Seg("BLOCKED ".into(), Style::bold(style::BLOCKED)),
                        Seg(
                            format!("→ {}", task.unblock.as_deref().unwrap_or("?")),
                            Style::bold(style::BLOCKED),
                        ),
                    ];
                }
                Some("review") => {
                    row.glyph = style::state_glyph("review");
                    row.glyph_color = style::state_color("review");
                    row.hot = true;
                    row.status = vec![
                        Seg("review ".into(), Style::bold(style::REVIEW)),
                        Seg(
                            format!("→ {}", task.owner.as_deref().unwrap_or("?")),
                            Style::bold(style::REVIEW),
                        ),
                    ];
                }
                _ => {}
            }
        }
        // herdr overlay: a live attempt whose locator pane no longer exists.
        // Claimed only while the link is connected — no herdr, no claim.
        if matches!(st, "working" | "queued" | "lost") {
            if let (Some(h), Some(p)) =
                (hints, a.locator.as_ref().and_then(|l| l.pane.as_deref()))
            {
                if h.pane(p) == Some(None) {
                    row.status.push(Seg(" ⚠ pane gone".into(), Style::bold(style::BLOCKED)));
                    row.hot = true;
                }
            }
        }
    } else {
        // task stub (no attempts yet)
        let st = task.state.as_deref().unwrap_or("queued");
        row.state = st.to_string();
        row.glyph = style::state_glyph(st);
        row.glyph_color = style::state_color(st);
        row.agent = task.owner.clone().unwrap_or_default();
        row.hot = matches!(st, "blocked" | "review");
        row.title_dim = st == "queued";
        match st {
            "blocked" => {
                row.status = vec![
                    Seg("BLOCKED ".into(), Style::bold(style::BLOCKED)),
                    Seg(
                        format!("→ {}", task.unblock.as_deref().unwrap_or("?")),
                        Style::bold(style::BLOCKED),
                    ),
                ];
            }
            "review" => {
                row.status = vec![
                    Seg("review ".into(), Style::bold(style::REVIEW)),
                    Seg(
                        format!("→ {}", task.owner.as_deref().unwrap_or("?")),
                        Style::bold(style::REVIEW),
                    ),
                ];
            }
            _ => {
                row.status = vec![Seg("queued".into(), Style::dim(style::QUEUED))];
            }
        }
    }
    // gate: fan-in chips on *every* gate row, attempted or not (F2) — the
    // ← set is the gate's identity. The waits/ready status only replaces
    // the state word while no attempt has claimed the row.
    if kind == "gate" {
        let inputs = task.inputs.clone().unwrap_or_else(|| task.deps.clone());
        if !inputs.is_empty() {
            let mut chips = vec![Seg("← ".into(), Style::fg(style::ACCENT))];
            let mut first_unmet: Option<(String, u8)> = None;
            for input in &inputs {
                let ist = ix
                    .task_by_id
                    .get(input.as_str())
                    .and_then(|&ti| ix.tasks[ti].state.as_deref())
                    .unwrap_or("queued");
                let col = style::state_color(ist);
                let donei = ist == "done";
                if !donei && first_unmet.is_none() {
                    first_unmet = Some((input.clone(), col));
                }
                chips.push(Seg(
                    format!("{input}{} ", style::chip_mark(ist)),
                    Style {
                        fg: Some(col),
                        bold: ist == "working",
                        dim: donei,
                    },
                ));
            }
            row.chips = chips;
            if ix.attempt(n).is_none() {
                row.status = match first_unmet {
                    None => vec![Seg("ready".into(), Style::bold(style::DONE))],
                    Some((id, col)) => vec![
                        Seg("waits ".into(), Style::dim(style::QUEUED)),
                        Seg(id, Style::bold(col)),
                    ],
                };
            }
        }
    }
    // annotations off the primary tree, on the task's first row only:
    // extra deps beyond the one the rail draws (F15) and the task note
    // (F16) — dim ⇠ ink, the reference's off-tree grammar
    let first_row = match n {
        NodeRef::T(_) => true,
        NodeRef::A(..) => ix.attempt(n).map(|a| a.n.unwrap_or(1) <= 1).unwrap_or(true),
    };
    if first_row {
        if kind != "gate" && task.deps.len() > 1 {
            for d in &task.deps[1..] {
                row.chips.push(Seg(format!("⇠ {d} "), Style::dim(style::MUTED)));
            }
        }
        if let Some(note) = task.note.as_deref() {
            // producers sometimes write the ⇠ themselves; one is enough
            let note = note.trim_start_matches("⇠ ");
            row.chips.push(Seg(format!("⇠ {note} "), Style::dim(style::MUTED)));
        }
    }
    row
}

/// Dotted future rows for a live attempt, from declared policy only.
/// At rest: first-level futures (no `after`). Selected: the whole chain,
/// `after`-nesting drawn one rail step deeper each link.
fn future_rows(ix: &Ix, task: &Task, prefix: &str, unroll: bool) -> Vec<Row> {
    let Some(policy) = &task.policy else { return Vec::new() };
    let tid = task.id.as_deref().unwrap_or("");
    let firsts: Vec<usize> = (0..policy.futures.len())
        .filter(|&i| policy.futures[i].after.is_none())
        .collect();
    let mut out = Vec::new();

    // depth of each future via `after` chain
    let depth_of = |i: usize| -> usize {
        let mut d = 0;
        let mut cur = policy.futures[i].after.as_deref();
        while let Some(aid) = cur {
            d += 1;
            cur = policy
                .futures
                .iter()
                .find(|f| f.node.as_ref().and_then(|n| n.id.as_deref()) == Some(aid))
                .and_then(|f| f.after.as_deref());
            if d > 8 {
                break;
            }
        }
        d
    };

    let shown: Vec<usize> = if unroll {
        (0..policy.futures.len()).collect()
    } else {
        firsts.clone()
    };

    for (pos, &i) in shown.iter().enumerate() {
        let f = &policy.futures[i];
        let depth = depth_of(i);
        let last_at_depth = !shown[pos + 1..].iter().any(|&j| depth_of(j) == depth);
        let lead = if last_at_depth { '╰' } else { '├' };
        let rail = format!("{prefix}{}{}┄", "  ".repeat(depth), lead);
        let cond = {
            match f.on.as_deref() {
                Some("pass") => Seg("if ✓".into(), Style::fg(style::DONE)),
                _ => Seg(format!("if {}", streak_marks(f.streak)), Style::fg(style::REJECTED)),
            }
        };
        if let Some(target) = f.reference.as_deref() {
            let ttitle = ix
                .task_by_id
                .get(target)
                .and_then(|&ti| ix.tasks[ti].title.as_deref())
                .unwrap_or("");
            // titles carry a "kind: " prefix; the » row already names its
            // target, so the doubled prefix reads as stutter (F21)
            let ttitle = ttitle.split_once(": ").map(|(_, t)| t).unwrap_or(ttitle);
            let towner = ix
                .task_by_id
                .get(target)
                .and_then(|&ti| ix.tasks[ti].owner.as_deref())
                .unwrap_or("");
            let mut row = Row::blank(&format!("{tid}»{target}"), tid, "future");
            row.rail = rail;
            row.dotted = true;
            row.glyph = '»';
            row.glyph_color = style::ACCENT;
            row.name = target.to_string();
            row.title = format!("pass ⇒ unblocks: {ttitle}");
            row.title_dim = true;
            row.agent = towner.to_string();
            row.status = vec![cond];
            out.push(row);
        } else if let Some(node) = &f.node {
            let nid = node.id.clone().unwrap_or_default();
            let loopy = f.loop_back.unwrap_or(false);
            let mut row = Row::blank(&nid, tid, "future");
            row.rail = rail;
            row.dotted = true;
            row.glyph = if loopy { '⟲' } else { '○' };
            row.glyph_color = if loopy { style::REJECTED } else { style::GHOST };
            row.name = nid;
            row.title = node.title.clone().unwrap_or_default();
            row.title_dim = true;
            row.model = node.model.clone().unwrap_or_default();
            let predicted = node.attribution.as_deref() == Some("predicted");
            let actor = node.actor.clone().unwrap_or_default();
            row.agent = if predicted { format!("≈ {actor}") } else { actor };
            row.status = vec![cond];
            if let Some(src) = f.source.as_deref() {
                let _ = src; // provenance shown in the focus card, not the row
            }
            out.push(row);
        }
    }
    out
}

/// Selected gate: fan-in unrolls as dotted reference rows — inputs with
/// their live glyphs, the blocker marked `holds`.
fn fanin_rows(ix: &Ix, task: &Task, prefix: &str) -> Vec<Row> {
    let tid = task.id.as_deref().unwrap_or("");
    let inputs = task.inputs.clone().unwrap_or_else(|| task.deps.clone());
    let mut out = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        let Some(&ti) = ix.task_by_id.get(input.as_str()) else { continue };
        let it = &ix.tasks[ti];
        let ist = it.state.as_deref().unwrap_or("queued");
        let met = ist == "done";
        let lead = if i == inputs.len() - 1 { '╰' } else { '├' };
        let latest = ix.latest_attempt(ti);
        let name = latest.map(|n| ix.display_name(n)).unwrap_or_else(|| input.clone());
        let ev = latest
            .and_then(|n| ix.attempt(n))
            .and_then(|a| a.outcome.as_ref())
            .and_then(|o| o.evidence.as_deref());
        let mut row = Row::blank(&format!("{tid}←{input}"), tid, "ref");
        row.rail = format!("{prefix}{lead}┄");
        row.dotted = true;
        row.glyph = style::state_glyph(ist);
        row.glyph_color = style::state_color(ist);
        row.hot = !met;
        row.name = name;
        row.title = it.title.clone().unwrap_or_default();
        row.title_dim = met;
        row.model = String::new();
        row.agent = it.owner.clone().unwrap_or_default();
        row.status = if met {
            let (g, c) = style::evidence(ev.unwrap_or("asserted"));
            vec![Seg(format!("in {g}"), Style::dim(c))]
        } else {
            vec![Seg("holds".into(), Style::bold(style::state_color(ist)))]
        };
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A·a1 ── B·a1 ── D (stub, done)
    ///      ╰─ C·a1 (working attempt, task blocked)
    fn mini() -> Doc {
        serde_json::from_str(
            r#"{
              "dagr": 1,
              "run": {"id": "r", "title": "mini"},
              "generated_at": "2026-08-16T12:00:00Z",
              "tasks": [
                {"id": "A", "title": "impl: a", "state": "done", "deps": [],
                 "attempts": [{"id": "A·a1", "n": 1, "state": "done",
                               "started_at": "2026-08-16T10:00:00Z", "ended_at": "2026-08-16T10:10:00Z"}]},
                {"id": "B", "title": "impl: b", "state": "done", "deps": ["A"],
                 "attempts": [{"id": "B·a1", "n": 1, "state": "done",
                               "started_at": "2026-08-16T10:20:00Z", "ended_at": "2026-08-16T10:30:00Z"}]},
                {"id": "C", "title": "impl: c", "state": "blocked", "unblock": "emre", "deps": ["A"],
                 "attempts": [{"id": "C·a1", "n": 1, "state": "working",
                               "started_at": "2026-08-16T10:25:00Z"}]},
                {"id": "D", "title": "impl: d", "state": "done", "deps": ["B"], "attempts": []}
              ]
            }"#,
        )
        .expect("mini doc parses")
    }

    fn keys(scene: &Scene) -> Vec<&str> {
        scene.rows.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn the_full_walk_emits_every_node_with_parents() {
        let doc = mini();
        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["A·a1", "B·a1", "D", "C·a1"]);
        let by_key = |k: &str| scene.rows.iter().find(|r| r.key == k).unwrap();
        assert_eq!(by_key("A·a1").parent, None);
        assert_eq!(by_key("B·a1").parent.as_deref(), Some("A·a1"));
        assert_eq!(by_key("D").parent.as_deref(), Some("B·a1"));
        assert!(by_key("A·a1").has_kids && by_key("B·a1").has_kids);
        assert!(!by_key("D").has_kids);
    }

    #[test]
    fn a_fold_hides_the_subtree_but_not_the_alarm() {
        let doc = mini();
        let folded: std::collections::HashSet<String> = ["A·a1".to_string()].into();
        let scene = build(&doc, None, None, None, &ViewOpts { zoom: None, folded: Some(&folded) });
        assert_eq!(keys(&scene), ["A·a1"], "fallback must not resurrect folded rows");
        let chip = scene.rows[0].fold.as_ref().expect("fold chip");
        assert_eq!(chip.hidden, 3);
        // C's working attempt displays as BLOCKED (task state outranks) —
        // the chip must carry that, in blocked ink
        assert_eq!(chip.hot, Some(crate::style::BLOCKED));
        let text: String = chip.segs.iter().map(|Seg(s, _)| s.as_str()).collect();
        assert!(text.contains("3 hidden") && text.contains("1 blocked"), "{text}");
    }

    #[test]
    fn zoom_reroots_and_counts_attention_left_outside() {
        let doc = mini();
        let scene = build(&doc, None, None, None, &ViewOpts { zoom: Some("B·a1"), folded: None });
        assert_eq!(keys(&scene), ["B·a1", "D"]);
        let z = scene.zoom.as_ref().expect("zoom note");
        assert_eq!(z.root, "B·a1");
        assert_eq!(z.outside, 1, "blocked C is outside the zoom and must be counted");
        assert!(scene.queue.is_empty(), "C leaves the visible queue under this zoom");
    }

    #[test]
    fn unknown_zoom_key_draws_the_full_run() {
        let doc = mini();
        let scene = build(&doc, None, None, None, &ViewOpts { zoom: Some("nope"), folded: None });
        assert_eq!(scene.rows.len(), 4);
        assert!(scene.zoom.is_none());
    }

    #[test]
    fn settled_roots_find_the_topmost_finished_branch_only() {
        let doc = mini();
        // A's subtree holds blocked C → not settled; B·a1's subtree (D) is
        // all done → the one foldable settled branch. C is live, D a leaf.
        assert_eq!(settled_roots(&doc), ["B·a1"]);
    }

    #[test]
    fn ancestors_walk_to_the_root_nearest_first() {
        let doc = mini();
        assert_eq!(ancestors(&doc, "D"), ["B·a1", "A·a1"]);
        assert_eq!(ancestors(&doc, "A·a1"), Vec::<String>::new());
        assert_eq!(ancestors(&doc, "nope"), Vec::<String>::new());
    }

    #[test]
    fn search_matches_ids_titles_and_agents() {
        let doc = mini();
        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let c = scene.rows.iter().find(|r| r.key == "C·a1").unwrap();
        assert!(row_matches(c, "c·a1") && row_matches(c, "impl: c") && row_matches(c, "C"));
        assert!(!row_matches(c, "impl: d"));
    }
}
