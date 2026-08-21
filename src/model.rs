//! Contract → render model. Where a hand-painted mock fixes its scene,
//! here every row is *derived*
//! from contract data — tree shape from primary deps, attempt nesting from
//! cause chains (a fix round nests under the review that sent it back),
//! dotted futures from declared policy, join strips from gate inputs.
//! The renderer invents nothing (CONTRACT.md, non-goals).

use crate::contract::{Attempt, Doc, Project, Task};
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
    /// Compact, derived fan-in state carried by gate rows. The renderer
    /// chooses an expanded strip, counted strip, or total-only mark from
    /// the available width; exact input ids remain in the selected unroll.
    pub join: Option<GateJoin>,
    /// Inline annotations, placed after the title.
    pub chips: Vec<Seg>,
    pub model: String,
    pub status: Vec<Seg>,
    pub agent: String,
    pub selectable: bool,
    pub lit: bool,
    pub tag: Option<String>,
    pub state: String,
    /// A synthetic recursive-project heading rather than a task/attempt.
    pub project: bool,
    /// A gate milestone: render as a scope boundary, not an ordinary leaf.
    pub milestone: bool,
    /// Row key of the parent row in the walk (`None` for roots) — ← jumps here.
    pub parent: Option<String>,
    /// The walk put child rows under this one (foldable / zoomable).
    pub has_kids: bool,
    /// Present when the subtree is folded shut under this row.
    pub fold: Option<FoldChip>,
}

#[derive(Clone)]
pub struct GateJoin {
    /// One effective task state per direct gate input, in the producer's
    /// declared `inputs` / `deps` order.
    pub states: Vec<String>,
}

/// The composition a folded aggregate carries — folding compresses history,
/// it must not hide an alarm.
#[derive(Clone)]
pub struct FoldChip {
    /// Strongest attention color inside the fold (compact rows tint the
    /// ▸ marker with it).
    pub hot: Option<u8>,
    pub segs: Vec<Seg>,
}

/// A failed/rejected attempt is immutable history, while the task may be
/// queued again before its next attempt exists. That valid contract shape
/// needs a current task stub so readiness can be shown without repainting
/// the prior attempt.
pub(crate) fn needs_queued_stub(task: &Task) -> bool {
    task.state.as_deref() == Some("queued")
        && task
            .attempts
            .iter()
            .max_by_key(|a| a.n.unwrap_or(0))
            .and_then(|a| a.state.as_deref())
            .is_some_and(|state| !matches!(state, "queued" | "working"))
}

pub(crate) fn needs_current_stub(task: &Task) -> bool {
    (!task.attempts.is_empty() && task.state.as_deref() == Some("canceled"))
        || needs_queued_stub(task)
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
            join: None,
            chips: Vec::new(),
            model: String::new(),
            status: Vec::new(),
            agent: String::new(),
            selectable: false,
            lit: false,
            tag: None,
            state: state.into(),
            project: false,
            milestone: false,
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
    projects: &'a [Project],
    task_by_id: std::collections::HashMap<&'a str, usize>,
    project_by_id: std::collections::HashMap<&'a str, usize>,
    attempt_by_id: std::collections::HashMap<&'a str, (usize, usize)>,
    task_selectable: Vec<bool>,
    project_selectable: Vec<bool>,
    attempt_selectable: Vec<Vec<bool>>,
    /// One cycle-safe, memoized visual home per task. Gate fan-in can share
    /// large subgraphs, so resolving this on demand would repeat that work.
    task_projects: Vec<Option<usize>>,
    /// attempt order per task, sorted by n
    order: Vec<Vec<usize>>,
    #[cfg(test)]
    scope_resolution_calls: usize,
    #[cfg(test)]
    scope_resolution_evaluations: usize,
}

impl<'a> Ix<'a> {
    fn new(doc: &'a Doc) -> Self {
        let tasks: &[Task] = doc.tasks.as_deref().unwrap_or(&[]);
        let projects = doc.projects.as_slice();
        let mut node_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        let mut project_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for task in tasks {
            if let Some(id) = task.id.as_deref().filter(|id| crate::contract::valid_identity(id)) {
                *node_counts.entry(id).or_default() += 1;
            }
            for attempt in &task.attempts {
                if let Some(id) = attempt
                    .id
                    .as_deref()
                    .filter(|id| crate::contract::valid_identity(id))
                {
                    *node_counts.entry(id).or_default() += 1;
                }
            }
        }
        for project in projects {
            if let Some(id) = project
                .id
                .as_deref()
                .filter(|id| crate::contract::valid_identity(id))
            {
                *project_counts.entry(id).or_default() += 1;
            }
        }
        let project_row_keys: std::collections::HashSet<String> = project_counts
            .keys()
            .map(|id| format!("project:{id}"))
            .collect();
        let mut task_by_id = std::collections::HashMap::new();
        let mut project_by_id = std::collections::HashMap::new();
        let mut attempt_by_id = std::collections::HashMap::new();
        let mut task_selectable = Vec::with_capacity(tasks.len());
        let mut attempt_selectable = Vec::with_capacity(tasks.len());
        let mut order = Vec::new();
        for (ti, t) in tasks.iter().enumerate() {
            let task_ok = t.id.as_deref().is_some_and(|id| {
                crate::contract::valid_identity(id)
                    && node_counts.get(id) == Some(&1)
                    && !project_row_keys.contains(id)
            });
            task_selectable.push(task_ok);
            if task_ok {
                task_by_id.insert(t.id.as_deref().expect("validated task id"), ti);
            }
            let mut idx: Vec<usize> = (0..t.attempts.len()).collect();
            idx.sort_by_key(|&ai| t.attempts[ai].n.unwrap_or(0));
            let mut selectable = vec![false; t.attempts.len()];
            for &ai in &idx {
                let attempt_ok = task_ok
                    && t.attempts[ai].id.as_deref().is_some_and(|id| {
                        crate::contract::valid_identity(id) && node_counts.get(id) == Some(&1)
                            && !project_row_keys.contains(id)
                    });
                selectable[ai] = attempt_ok;
                if attempt_ok {
                    attempt_by_id.insert(
                        t.attempts[ai].id.as_deref().expect("validated attempt id"),
                        (ti, ai),
                    );
                }
            }
            attempt_selectable.push(selectable);
            order.push(idx);
        }
        let mut project_selectable = Vec::with_capacity(projects.len());
        for (pi, p) in projects.iter().enumerate() {
            let project_ok = p.id.as_deref().is_some_and(|id| {
                crate::contract::valid_identity(id)
                    && project_counts.get(id) == Some(&1)
                    && !node_counts.contains_key(format!("project:{id}").as_str())
            });
            project_selectable.push(project_ok);
            if project_ok {
                project_by_id.insert(p.id.as_deref().expect("validated project id"), pi);
            }
        }
        let mut ix = Ix {
            tasks,
            projects,
            task_by_id,
            project_by_id,
            attempt_by_id,
            task_selectable,
            project_selectable,
            attempt_selectable,
            task_projects: vec![None; tasks.len()],
            order,
            #[cfg(test)]
            scope_resolution_calls: 0,
            #[cfg(test)]
            scope_resolution_evaluations: 0,
        };
        let mut states = vec![0u8; tasks.len()];
        let mut memo = vec![None; tasks.len()];
        let mut calls = 0usize;
        let mut evaluations = 0usize;
        for ti in 0..tasks.len() {
            ix.resolve_task_project(ti, &mut states, &mut memo, &mut calls, &mut evaluations);
        }
        ix.task_projects = memo.into_iter().map(|scope| scope.unwrap_or(None)).collect();
        #[cfg(test)]
        {
            ix.scope_resolution_calls = calls;
            ix.scope_resolution_evaluations = evaluations;
        }
        ix
    }

    fn task_index(&self, n: NodeRef) -> usize {
        match n {
            NodeRef::A(ti, _) | NodeRef::T(ti) => ti,
        }
    }

    fn project_parent(&self, pi: usize) -> Option<usize> {
        self.projects
            .get(pi)
            .and_then(|p| p.parent.as_deref())
            .and_then(|id| self.project_by_id.get(id).copied())
    }

    /// Nearest shared project; `None` is the run/root project.
    fn project_lca(&self, a: Option<usize>, b: Option<usize>) -> Option<usize> {
        let (Some(a), Some(b)) = (a, b) else { return None };
        let mut ancestors = std::collections::HashSet::new();
        let mut cur = Some(a);
        while let Some(pi) = cur {
            if !ancestors.insert(pi) {
                break;
            }
            cur = self.project_parent(pi);
        }
        let mut seen = std::collections::HashSet::new();
        let mut cur = Some(b);
        while let Some(pi) = cur {
            if !seen.insert(pi) {
                return None;
            }
            if ancestors.contains(&pi) {
                return Some(pi);
            }
            cur = self.project_parent(pi);
        }
        None
    }

    /// Resolve a task's visual home once. State 1 is the active DFS stack and
    /// fails a malformed cycle closed to the run root; state 2 is memoized.
    fn resolve_task_project(
        &self,
        ti: usize,
        states: &mut [u8],
        memo: &mut [Option<Option<usize>>],
        calls: &mut usize,
        evaluations: &mut usize,
    ) -> Option<usize> {
        *calls += 1;
        match states.get(ti).copied() {
            Some(2) => return memo[ti].unwrap_or(None),
            Some(1) | None => return None,
            Some(_) => {}
        }
        states[ti] = 1;
        *evaluations += 1;
        let task = &self.tasks[ti];
        let explicit = task
            .project
            .as_deref()
            .and_then(|id| self.project_by_id.get(id).copied());
        let scope = if explicit.is_some() || task.kind.as_deref() != Some("gate") {
            explicit
        } else {
            let inputs = task.inputs.as_ref().unwrap_or(&task.deps);
            let mut input_tasks =
                inputs.iter().filter_map(|id| self.task_by_id.get(id.as_str()).copied());
            if let Some(first) = input_tasks.next() {
                let mut common =
                    self.resolve_task_project(first, states, memo, calls, evaluations);
                for input in input_tasks {
                    let input_scope =
                        self.resolve_task_project(input, states, memo, calls, evaluations);
                    common = self.project_lca(common, input_scope);
                }
                common
            } else {
                None
            }
        };
        states[ti] = 2;
        memo[ti] = Some(scope);
        scope
    }

    /// A task has one visual home. A gate without an explicit home lives at
    /// the nearest project shared by every input (the run root if streams
    /// cross top-level projects).
    fn task_project(&self, ti: usize) -> Option<usize> {
        self.task_projects.get(ti).copied().flatten()
    }

    fn same_project(&self, a: usize, b: usize) -> bool {
        self.task_project(a) == self.task_project(b)
    }

    fn project_is_within(&self, child: Option<usize>, ancestor: usize) -> bool {
        let mut cur = child;
        let mut seen = std::collections::HashSet::new();
        while let Some(pi) = cur {
            if pi == ancestor {
                return true;
            }
            if !seen.insert(pi) {
                break;
            }
            cur = self.project_parent(pi);
        }
        false
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

    fn current_node(&self, ti: usize) -> NodeRef {
        if needs_current_stub(&self.tasks[ti]) {
            NodeRef::T(ti)
        } else {
            self.latest_attempt(ti).unwrap_or(NodeRef::T(ti))
        }
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
            NodeRef::A(ti, ai) if self.attempt_selectable[ti][ai] => self.tasks[ti].attempts[ai]
                .id
                .clone()
                .expect("selectable attempt has id"),
            NodeRef::A(ti, ai) => format!("\u{1f}invalid:attempt:{ti}:{ai}"),
            NodeRef::T(ti) if self.task_selectable[ti] => self.tasks[ti]
                .id
                .clone()
                .expect("selectable task has id"),
            NodeRef::T(ti) => format!("\u{1f}invalid:task:{ti}"),
        }
    }

    fn node_selectable(&self, n: NodeRef) -> bool {
        match n {
            NodeRef::A(ti, ai) => self.attempt_selectable[ti][ai],
            NodeRef::T(ti) => self.task_selectable[ti],
        }
    }

    fn project_key(&self, pi: usize) -> String {
        if self.project_selectable[pi] {
            format!(
                "project:{}",
                self.projects[pi].id.as_deref().expect("selectable project has id")
            )
        } else {
            format!("\u{1f}invalid:project:{pi}")
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

    fn first_unmet(&self, task: &Task) -> Option<(String, u8)> {
        let deps = if task.kind.as_deref() == Some("gate") {
            task.inputs.as_deref().unwrap_or(&task.deps)
        } else {
            &task.deps
        };
        deps.iter().find_map(|dep| {
            let state = self
                .task_by_id
                .get(dep.as_str())
                .and_then(|&ti| self.tasks[ti].state.as_deref())
                .unwrap_or("queued");
            (state != "done").then(|| (dep.clone(), style::state_color(state)))
        })
    }

    /// One queued projection shared by rows, projects, folds, focus and the
    /// attention queue. Assignment may come from the task or its current
    /// queued attempt, but never from a settled historical attempt.
    fn queued_signal(&self, ti: usize) -> &'static str {
        let task = &self.tasks[ti];
        if self.first_unmet(task).is_some() {
            return "waiting";
        }
        if task.kind.as_deref() == Some("question") {
            return "needs_answer";
        }
        let assigned = task.owner.as_deref().is_some_and(|owner| !owner.is_empty())
            || self
                .attempt(self.current_node(ti))
                .and_then(|attempt| attempt.actor.as_deref())
                .is_some_and(|actor| !actor.is_empty());
        if assigned { "ready" } else { "unassigned" }
    }

    /// One task-level projection shared by rows, projects, folds, focus and
    /// the attention queue. Readiness remains derived from declared facts.
    fn task_signal(&self, ti: usize) -> &'static str {
        let task = &self.tasks[ti];
        let raw = task.state.as_deref().unwrap_or("");
        if raw == "canceled" {
            if task.attempts.iter().any(|a| a.state.as_deref() == Some("lost")) {
                return "lost";
            }
            if task
                .attempts
                .iter()
                .any(|a| a.state.as_deref() == Some("working"))
            {
                return "working";
            }
            return "canceled";
        }
        if matches!(raw, "blocked" | "review") {
            return if raw == "blocked" { "blocked" } else { "review" };
        }
        if let Some(state) = self
            .latest_attempt(ti)
            .and_then(|n| self.attempt(n))
            .and_then(|a| a.state.as_deref())
        {
            match state {
                "lost" => return "lost",
                "settled_unverified" => return "settled_unverified",
                _ => {}
            }
        }
        if raw == "working"
            || self
                .latest_attempt(ti)
                .and_then(|n| self.attempt(n))
                .and_then(|a| a.state.as_deref())
                == Some("working")
        {
            return "working";
        }
        if raw == "queued" {
            return self.queued_signal(ti);
        }
        match raw {
            "done" => "done",
            "failed" | "rejected" => "failed",
            "settled_unverified" => "settled_unverified",
            _ => "invalid",
        }
    }

    /// Parent node: cause chain first, then primary dep. Gates are scope
    /// milestones, never children of an arbitrary input. Their own retry
    /// attempts may still follow earlier attempts of the same gate.
    fn parent(&self, n: NodeRef) -> Option<NodeRef> {
        self.parent_guarded(n, &mut std::collections::HashSet::new())
    }

    /// Parent resolution is cycle-safe even for a malformed document that
    /// the validator will reject.
    fn parent_guarded(
        &self,
        n: NodeRef,
        resolving: &mut std::collections::HashSet<NodeRef>,
    ) -> Option<NodeRef> {
        if !resolving.insert(n) {
            return None;
        }
        let parent = self.parent_inner(n, resolving);
        resolving.remove(&n);
        parent
    }

    fn parent_inner(
        &self,
        n: NodeRef,
        _resolving: &mut std::collections::HashSet<NodeRef>,
    ) -> Option<NodeRef> {
        let task = self.task_of(n);
        let this_ti = self.task_index(n);
        let is_gate = task.kind.as_deref() == Some("gate");
        if let Some(a) = self.attempt(n) {
            if let Some(cause) = &a.cause {
                if let Some(r) = cause.reference.as_deref() {
                    if let Some(&(ti, ai)) = self.attempt_by_id.get(r) {
                        if self.same_project(this_ti, ti) && (!is_gate || ti == this_ti) {
                            return Some(NodeRef::A(ti, ai));
                        }
                    }
                    if let Some(&ti) = self.task_by_id.get(r) {
                        if self.same_project(this_ti, ti) && (!is_gate || ti == this_ti) {
                            return self.latest_attempt(ti);
                        }
                    }
                }
            }
            if is_gate {
                return None;
            }
            let started = a.started_at.as_deref().and_then(parse_min);
            if let Some(dep) = task.deps.first() {
                if let Some(&ti) = self.task_by_id.get(dep.as_str()) {
                    if self.same_project(this_ti, ti) {
                        return self.dep_attempt_at(ti, started);
                    }
                }
            }
            return None;
        }
        // task stub
        if needs_queued_stub(task) {
            return self.latest_attempt(this_ti);
        }
        if is_gate {
            return None;
        }
        if let Some(dep) = task.deps.first() {
            if let Some(&ti) = self.task_by_id.get(dep.as_str()) {
                if self.same_project(this_ti, ti) {
                    return Some(self.current_node(ti));
                }
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
            if needs_current_stub(t) {
                all.push(NodeRef::T(ti));
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
    let cmp = |a: &NodeRef, b: &NodeRef| match (*a, *b) {
        // Attempt-less siblings have no time signal. Preserve the producer's
        // task-array order instead of making ids carry a hidden sort contract.
        (NodeRef::T(ta), NodeRef::T(tb)) => ta.cmp(&tb),
        _ => (ix.start_of(*a), ix.key_of(*a)).cmp(&(ix.start_of(*b), ix.key_of(*b))),
    };
    for kids in &mut children {
        kids.sort_by(cmp);
    }
    roots.sort_by(cmp);
    Forest { all, children, index, roots }
}

/// The state a row DISPLAYS for attention purposes: the attempt's state,
/// with the task's blocked/review outranking a live working/queued — the
/// same projection `node_row` draws. Fold summaries and the settled check
/// ride this, so a fold can never hide what the trace would have shown.
fn effective_state(ix: &Ix, n: NodeRef) -> String {
    let ti = ix.task_index(n);
    let task = ix.task_of(n);
    match ix.attempt(n) {
        Some(a) => {
            let st = a.state.as_deref().unwrap_or("queued");
            if ix.current_node(ti) == n && matches!(st, "working" | "queued") {
                if let Some(ts @ ("blocked" | "review")) = ix.task_of(n).state.as_deref() {
                    return ts.to_string();
                }
                if st == "queued" && ix.task_of(n).state.as_deref() == Some("queued") {
                    return ix.queued_signal(ti).to_string();
                }
            }
            st.to_string()
        }
        None if task.state.as_deref() == Some("canceled") => "canceled".to_string(),
        None => ix.task_signal(ti).to_string(),
    }
}

/// First declared prerequisite that has not completed successfully. A
/// terminal failure or cancellation remains unmet: downstream work does not
/// become ready merely because its blocker stopped.
/// A queued row's operator-facing signal. This is derived entirely from
/// existing task fields, so producers declare no second readiness state.
fn queued_status(ix: &Ix, ti: usize) -> Vec<Seg> {
    match ix.queued_signal(ti) {
        "waiting" => {
            let (id, col) = ix.first_unmet(&ix.tasks[ti]).expect("waiting has an unmet input");
            vec![
                Seg("waits ".into(), Style::dim(style::QUEUED)),
                Seg(id, Style::bold(col)),
            ]
        }
        "needs_answer" => vec![Seg("needs answer".into(), Style::bold(style::REVIEW))],
        "ready" => vec![Seg("ready".into(), Style::bold(style::DONE))],
        _ => vec![Seg("unassigned".into(), Style::bold(style::QUEUED))],
    }
}

const SIGNAL_KINDS: usize = 12;

fn tally_signal(state: &str, states: &mut [usize; SIGNAL_KINDS]) {
    let index = match state {
        "blocked" => 0,
        "lost" => 1,
        "review" => 2,
        "needs_answer" => 3,
        "settled_unverified" => 4,
        "working" => 5,
        "waiting" => 6,
        "ready" => 7,
        "unassigned" => 8,
        "failed" | "rejected" => 9,
        "canceled" => 10,
        _ => 11,
    };
    states[index] += 1;
}

/// states: blocked, lost, review, needs-answer, unverified, working,
/// waiting, ready, unassigned, failed/rejected, canceled, done. The row
/// these are the composition, not a second rendering of the folded root task.
fn fold_chip(states: &[usize; SIGNAL_KINDS]) -> FoldChip {
    let mut segs = Vec::new();
    let mut hot = None;
    let cats = [
        ("blocked", states[0]),
        ("lost", states[1]),
        ("review", states[2]),
        ("needs_answer", states[3]),
        ("settled_unverified", states[4]),
        ("working", states[5]),
        ("waiting", states[6]),
        ("ready", states[7]),
        ("unassigned", states[8]),
        ("failed", states[9]),
        ("canceled", states[10]),
        ("done", states[11]),
    ];
    for (state, count) in cats {
        if count == 0 {
            continue;
        }
        let col = style::state_color(state);
        let label = match state {
            "settled_unverified" => "unverified",
            "needs_answer" => "needs answer",
            other => other,
        };
        // activity/settled categories show but do not make the fold an alarm
        let live = matches!(
            state,
            "blocked" | "lost" | "review" | "needs_answer" | "settled_unverified" | "failed"
        );
        if live && hot.is_none() {
            hot = Some(col);
        }
        segs.push(Seg(
            format!(" · {} {count} {label}", style::state_glyph(state)),
            if live { Style::bold(col) } else { Style::dim(col) },
        ));
    }
    FoldChip { hot, segs }
}

/// Row keys of the topmost all-settled subtrees — branches where the node
/// and every descendant is done/failed/rejected/canceled: history, nothing
/// live or waiting. These are what `z` folds; attention states never fold away.
pub fn settled_roots(doc: &Doc) -> Vec<String> {
    fn settled(ix: &Ix, f: &Forest, n: NodeRef) -> bool {
        matches!(
            effective_state(ix, n).as_str(),
            "done" | "failed" | "rejected" | "canceled"
        )
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
    let mut project_cur = key
        .strip_prefix("project:")
        .and_then(|id| ix.project_by_id.get(id).copied());
    let mut cur = ix
        .attempt_by_id
        .get(key)
        .map(|&(ti, ai)| NodeRef::A(ti, ai))
        .or_else(|| {
            ix.task_by_id
                .get(key)
                .map(|&ti| ix.current_node(ti))
        });
    let task_project = cur.and_then(|n| ix.task_project(ix.task_index(n)));
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = cur {
        if !seen.insert(n) {
            break;
        }
        match ix.parent(n) {
            Some(p) if p != n => {
                out.push(ix.key_of(p));
                cur = Some(p);
            }
            _ => {
                project_cur = task_project;
                break;
            }
        }
    }
    let mut seen_projects = std::collections::HashSet::new();
    while let Some(pi) = project_cur {
        if !seen_projects.insert(pi) {
            break;
        }
        if key != ix.project_key(pi) {
            out.push(ix.project_key(pi));
        }
        project_cur = ix.project_parent(pi);
    }
    out
}

/// Does this row key still name a task or attempt in the document?
/// Zoom stacks and fold sets are pruned with this across reloads.
pub fn key_exists(doc: &Doc, key: &str) -> bool {
    let ix = Ix::new(doc);
    ix.attempt_by_id.contains_key(key)
        || ix.task_by_id.contains_key(key)
        || key
            .strip_prefix("project:")
            .is_some_and(|id| ix.project_by_id.contains_key(id))
}

/// Effective state for one unambiguous interactive identity. Focus cards use
/// this rather than re-deriving lifecycle semantics independently.
pub fn selection_state(doc: &Doc, key: &str) -> Option<String> {
    let ix = Ix::new(doc);
    if let Some(&(ti, ai)) = ix.attempt_by_id.get(key) {
        return Some(effective_state(&ix, NodeRef::A(ti, ai)));
    }
    if let Some(&ti) = ix.task_by_id.get(key) {
        return Some(effective_state(&ix, ix.current_node(ti)));
    }
    key.strip_prefix("project:")
        .and_then(|id| ix.project_by_id.get(id).copied())
        .map(|pi| project_row(&ix, pi, 0).state)
}

/// Does a row answer this search? Matched fields: row key, display name,
/// title, agent, and owning task id.
pub fn row_matches(r: &Row, q: &str) -> bool {
    let q = q.to_lowercase();
    [&r.key, &r.name, &r.title, &r.agent, &r.task_id]
        .into_iter()
        .any(|s| s.to_lowercase().contains(&q))
}

fn project_row(ix: &Ix, pi: usize, depth: usize) -> Row {
    let project = &ix.projects[pi];
    let id = project.id.as_deref().unwrap_or("?");
    let mut row = Row::blank(&ix.project_key(pi), "", "queued");
    row.project = true;
    row.selectable = ix.project_selectable[pi];
    row.rail = "  ".repeat(depth);
    row.glyph = '▾';
    row.name = id.to_string();
    row.title = project.title.clone().unwrap_or_default();
    row.agent = project.owner.clone().unwrap_or_default();
    row.chips = project
        .note
        .as_deref()
        .map(|n| vec![Seg(format!("{n} "), Style::dim(style::MUTED))])
        .unwrap_or_default();

    // Match the fold aggregate's vocabulary. A project summary is where a
    // collapsed organizational scope reports its health, so it must not
    // turn failures, lost attempts, or unverified settlements into the
    // reassuring word "settled".
    let mut counts = [0usize; SIGNAL_KINDS];
    for ti in 0..ix.tasks.len() {
        if !ix.project_is_within(ix.task_project(ti), pi) {
            continue;
        }
        tally_signal(ix.task_signal(ti), &mut counts);
    }
    let specs = [
        (0, "blocked", style::BLOCKED),
        (1, "lost", style::BLOCKED),
        (2, "review", style::REVIEW),
        (3, "needs answer", style::REVIEW),
        (4, "unverified", style::EV_HEURISTIC),
        (5, "working", style::WORKING),
        (6, "waiting", style::QUEUED),
        (7, "ready", style::DONE),
        (8, "unassigned", style::QUEUED),
        (9, "failed", style::FAILED),
        (10, "canceled", style::MUTED),
        (11, "done", style::DONE),
    ];
    for (idx, label, color) in specs {
        if counts[idx] > 0 {
            if !row.status.is_empty() {
                row.status.push(Seg(" · ".into(), Style::dim(style::MUTED)));
            }
            row.status.push(Seg(
                format!("{} {label}", counts[idx]),
                if matches!(idx, 0 | 1 | 2 | 3 | 4 | 9) {
                    Style::bold(color)
                } else {
                    Style::dim(color)
                },
            ));
        }
    }
    let strongest = if counts[0] > 0 {
        "blocked"
    } else if counts[1] > 0 {
        "lost"
    } else if counts[2] > 0 {
        "review"
    } else if counts[3] > 0 {
        "needs_answer"
    } else if counts[4] > 0 {
        "settled_unverified"
    } else if counts[9] > 0 {
        "failed"
    } else if counts[5] > 0 {
        "working"
    } else if counts[6] > 0 {
        "waiting"
    } else if counts[7] > 0 {
        "ready"
    } else if counts[8] > 0 {
        "unassigned"
    } else if counts[10] > 0 {
        "canceled"
    } else {
        "done"
    };
    row.state = strongest.into();
    row.glyph_color = style::state_color(strongest);
    row.hot = matches!(
        strongest,
        "blocked" | "lost" | "review" | "needs_answer" | "settled_unverified" | "failed"
    );
    row
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
    let zoom_project = opts
        .zoom
        .and_then(|z| z.strip_prefix("project:"))
        .and_then(|id| ix.project_by_id.get(id).copied());
    let zoom_node = if zoom_project.is_some() {
        None
    } else {
        opts.zoom.and_then(|z| all.iter().copied().find(|&n| ix.key_of(n) == z))
    };
    let walk_roots: Vec<NodeRef> = match zoom_node {
        Some(n) => vec![n],
        None => roots.clone(),
    };

    let no_folds = std::collections::HashSet::new();
    let folded = opts.folded.unwrap_or(&no_folds);
    // every node the walk accounted for — emitted as a row OR hidden
    // inside a fold. This is also the cycle guard for malformed graphs,
    // and keeps the unreachable-node fallback below from resurrecting
    // folded work as flat rows.
    let mut covered: std::collections::HashSet<NodeRef> = std::collections::HashSet::new();

    let mut rows: Vec<Row> = Vec::new();
    struct Walker<'a, 'b> {
        ix: &'b Ix<'a>,
        all: &'b [NodeRef],
        children: &'b [Vec<NodeRef>],
        index: &'b std::collections::HashMap<NodeRef, usize>,
        rows: &'b mut Vec<Row>,
        selected_task: Option<&'b str>,
        now_min: Option<i64>,
        hints: Option<&'b Hints>,
        folded: &'b std::collections::HashSet<String>,
        covered: &'b mut std::collections::HashSet<NodeRef>,
    }
    impl<'a, 'b> Walker<'a, 'b> {
        fn pos(&self, n: NodeRef) -> usize {
            self.index[&n]
        }
        /// Mark a folded-away subtree as covered while tallying what the
        /// fold hides — count and attention states, for the ▸ chip.
        fn tally(state: &str, states: &mut [usize; SIGNAL_KINDS]) {
            tally_signal(state, states);
        }
        fn consume(
            &mut self,
            n: NodeRef,
            hidden: &mut usize,
            states: &mut [usize; SIGNAL_KINDS],
            seen_tasks: &mut std::collections::HashSet<usize>,
        ) {
            for k in self.children[self.pos(n)].clone() {
                if !self.covered.insert(k) {
                    continue;
                }
                *hidden += 1;
                let ti = self.ix.task_index(k);
                if seen_tasks.insert(ti) {
                    Self::tally(self.ix.task_signal(ti), states);
                }
                self.consume(k, hidden, states, seen_tasks);
            }
        }
        fn walk(&mut self, n: NodeRef, prefix: &str, is_root: bool, is_last: bool, parent: Option<&str>) {
            let key = self.ix.key_of(n);
            if !self.covered.insert(n) {
                return;
            }
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
                let mut states = [0usize; SIGNAL_KINDS];
                let mut seen_tasks = std::collections::HashSet::new();
                let ti = self.ix.task_index(n);
                seen_tasks.insert(ti);
                Self::tally(self.ix.task_signal(ti), &mut states);
                self.consume(n, &mut hidden, &mut states, &mut seen_tasks);
                let row = self.rows.last_mut().expect("row just pushed");
                let chip = fold_chip(&states);
                row.glyph = '▸';
                row.glyph_color = chip.hot.unwrap_or(style::ACCENT);
                row.hot = chip.hot.is_some();
                row.title = format!("folded branch · {} items", hidden + 1);
                row.title_dim = false;
                row.join = None;
                row.chips.clear();
                row.model.clear();
                row.agent.clear();
                row.status = vec![Seg("folded".into(), Style::dim(style::MUTED))];
                row.milestone = false;
                row.fold = Some(chip);
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

    fn walk_project<'a, 'b>(
        w: &mut Walker<'a, 'b>,
        pi: usize,
        depth: usize,
        parent: Option<&str>,
        scoped: &std::collections::HashMap<Option<usize>, Vec<NodeRef>>,
        project_children: &[Vec<usize>],
        project_seen: &mut std::collections::HashSet<usize>,
    ) {
        if !project_seen.insert(pi) {
            return;
        }
        let roots = scoped.get(&Some(pi)).cloned().unwrap_or_default();
        let (mut ordinary, mut gates): (Vec<_>, Vec<_>) = roots
            .into_iter()
            .partition(|&n| w.ix.task_of(n).kind.as_deref() != Some("gate"));
        let children = project_children.get(pi).map(Vec::as_slice).unwrap_or(&[]);
        let key = w.ix.project_key(pi);
        let mut row = project_row(w.ix, pi, depth);
        row.parent = parent.map(str::to_string);
        row.has_kids = !ordinary.is_empty() || !gates.is_empty() || !children.is_empty();
        w.rows.push(row);

        if w.rows.last().is_some_and(|row| row.has_kids) && w.folded.contains(&key) {
            let mut states = [0usize; SIGNAL_KINDS];
            let mut hidden = 0usize;
            for ti in 0..w.ix.tasks.len() {
                if w.ix.project_is_within(w.ix.task_project(ti), pi) {
                    tally_signal(w.ix.task_signal(ti), &mut states);
                }
            }
            for &n in w.all {
                if w
                    .ix
                    .project_is_within(w.ix.task_project(w.ix.task_index(n)), pi)
                    && w.covered.insert(n)
                {
                    hidden += 1;
                }
            }
            let mut stack = children.to_vec();
            while let Some(child) = stack.pop() {
                if project_seen.insert(child) {
                    hidden += 1;
                    stack.extend(
                        project_children
                            .get(child)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                    );
                }
            }
            let row = w.rows.last_mut().expect("project row just pushed");
            let chip = fold_chip(&states);
            row.glyph = '▸';
            row.glyph_color = chip.hot.unwrap_or(style::ACCENT);
            row.hot = chip.hot.is_some();
            row.title = format!("folded project · {} hidden", hidden);
            row.fold = Some(chip);
            return;
        }

        let prefix = format!("{}  ", "  ".repeat(depth));
        let ordinary_n = ordinary.len();
        for (i, n) in ordinary.drain(..).enumerate() {
            w.walk(n, &prefix, false, i + 1 == ordinary_n, Some(&key));
        }
        for &child in children {
            walk_project(
                w,
                child,
                depth + 1,
                Some(&key),
                scoped,
                project_children,
                project_seen,
            );
        }
        let gates_n = gates.len();
        for (i, n) in gates.drain(..).enumerate() {
            w.walk(n, &prefix, false, i + 1 == gates_n, Some(&key));
        }
    }

    let mut scoped: std::collections::HashMap<Option<usize>, Vec<NodeRef>> =
        std::collections::HashMap::new();
    for &root in &roots {
        scoped.entry(ix.task_project(ix.task_index(root))).or_default().push(root);
    }
    let mut project_children = vec![Vec::new(); ix.projects.len()];
    let mut top_projects = Vec::new();
    for pi in 0..ix.projects.len() {
        match ix.project_parent(pi) {
            Some(parent) if parent < project_children.len() => project_children[parent].push(pi),
            _ => top_projects.push(pi),
        }
    }
    let mut project_seen = std::collections::HashSet::new();
    {
        let mut w = Walker {
            ix: &ix,
            all: &all,
            children: &children,
            index: &index,
            rows: &mut rows,
            selected_task: selected_task.as_deref(),
            now_min,
            hints,
            folded,
            covered: &mut covered,
        };
        if zoom_node.is_some() {
            for (i, &r) in walk_roots.iter().enumerate() {
                w.walk(r, " ", true, i == walk_roots.len() - 1, None);
            }
        } else if let Some(pi) = zoom_project {
            let parent = ix.project_parent(pi).map(|parent| ix.project_key(parent));
            walk_project(
                &mut w,
                pi,
                0,
                parent.as_deref(),
                &scoped,
                &project_children,
                &mut project_seen,
            );
        } else if ix.projects.is_empty() {
            let (ordinary, gates): (Vec<_>, Vec<_>) = walk_roots
                .iter()
                .copied()
                .partition(|&n| ix.task_of(n).kind.as_deref() != Some("gate"));
            for (i, &r) in ordinary.iter().enumerate() {
                w.walk(r, " ", true, i + 1 == ordinary.len(), None);
            }
            for (i, &r) in gates.iter().enumerate() {
                w.walk(r, " ", true, i + 1 == gates.len(), None);
            }
        } else {
            let root_nodes = scoped.get(&None).cloned().unwrap_or_default();
            let (ordinary, gates): (Vec<_>, Vec<_>) = root_nodes
                .into_iter()
                .partition(|&n| ix.task_of(n).kind.as_deref() != Some("gate"));
            for (i, &r) in ordinary.iter().enumerate() {
                w.walk(r, " ", true, i + 1 == ordinary.len(), None);
            }
            for pi in top_projects {
                walk_project(
                    &mut w,
                    pi,
                    0,
                    None,
                    &scoped,
                    &project_children,
                    &mut project_seen,
                );
            }
            // A malformed project cycle has no top-level member. Keep every
            // project visible exactly once while the validator names the cycle.
            for pi in 0..ix.projects.len() {
                if !project_seen.contains(&pi) {
                    walk_project(
                        &mut w,
                        pi,
                        0,
                        None,
                        &scoped,
                        &project_children,
                        &mut project_seen,
                    );
                }
            }
            for (i, &r) in gates.iter().enumerate() {
                w.walk(r, " ", true, i + 1 == gates.len(), None);
            }
        }
    }
    // Malformed graphs (parent cycles) leave nodes unreachable from any
    // root. They still exist: emit them flat so the contract-error banner
    // has visible rows to explain, instead of silently dropping work.
    // (Skipped under zoom: out-of-subtree nodes are excluded on purpose.)
    if zoom_node.is_none() && zoom_project.is_none() {
        for &n in &all {
            if !covered.contains(&n) {
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
                let unmet = !matches!(t.state.as_deref(), Some("done" | "canceled"));
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

    // attention queue: blocked → lost/unverified → review/question → working.
    // Readiness and assignment stay on rows; only a human question joins the
    // attention queue, otherwise a large unstarted plan would become noise.
    let rank = |s: &str| match s {
        "blocked" => 0,
        "lost" => 1,
        "settled_unverified" => 2,
        "review" => 3,
        "needs_answer" => 4,
        "working" => 5,
        _ => 9,
    };
    let mut queue: Vec<QueueItem> = tasks
        .iter()
        .enumerate()
        .filter_map(|(ti, t)| {
            if !ix.task_selectable[ti] {
                return None;
            }
            let last = ix.order[ti].last().map(|&ai| &t.attempts[ai]);
            let state = ix.task_signal(ti).to_string();
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
                "review" | "needs_answer" => {
                    format!("→ {}", t.owner.as_deref().unwrap_or("?"))
                }
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
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![zn];
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            if let Some(id) = ix.task_of(n).id.clone() {
                keep.insert(id);
            }
            stack.extend(children[index[&n]].iter().copied());
        }
        let outside = queue
            .iter()
            .filter(|q| !keep.contains(&q.task_id))
            .filter(|q| {
                matches!(
                    q.state.as_str(),
                    "blocked" | "lost" | "review" | "needs_answer" | "settled_unverified"
                )
            })
            .count();
        queue.retain(|q| keep.contains(&q.task_id));
        ZoomNote { root: ix.key_of(zn), outside }
    }).or_else(|| {
        zoom_project.map(|pi| {
            let keep: std::collections::HashSet<String> = tasks
                .iter()
                .enumerate()
                .filter(|(ti, _)| ix.project_is_within(ix.task_project(*ti), pi))
                .filter_map(|(_, task)| task.id.clone())
                .collect();
            let outside = queue
                .iter()
                .filter(|item| !keep.contains(&item.task_id))
                .filter(|item| {
                    matches!(
                        item.state.as_str(),
                        "blocked"
                            | "lost"
                            | "review"
                            | "needs_answer"
                            | "settled_unverified"
                    )
                })
                .count();
            queue.retain(|item| keep.contains(&item.task_id));
            ZoomNote { root: ix.project_key(pi), outside }
        })
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
    row.selectable = ix.node_selectable(n);
    row.name = ix.display_name(n);
    row.title = task.title.clone().unwrap_or_default();
    row.milestone = kind == "gate";

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
            "queued" => {
                row.status = queued_status(ix, ix.task_index(n));
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
        let projected = effective_state(ix, n);
        if matches!(st, "queued" | "working") && projected != st {
            row.state = projected.clone();
            row.glyph = style::state_glyph(&projected);
            row.glyph_color = style::state_color(&projected);
            row.hot = matches!(projected.as_str(), "blocked" | "review" | "needs_answer");
            if matches!(
                projected.as_str(),
                "waiting" | "ready" | "unassigned" | "needs_answer"
            ) {
                row.status = queued_status(ix, ix.task_index(n));
            }
        }
    } else {
        // task stub (no attempts yet)
        let st = if task.state.as_deref() == Some("canceled") {
            "canceled"
        } else {
            ix.task_signal(ix.task_index(n))
        };
        row.state = st.to_string();
        row.glyph = style::state_glyph(st);
        row.glyph_color = style::state_color(st);
        row.agent = task.owner.clone().unwrap_or_default();
        row.hot = matches!(st, "blocked" | "review");
        row.title_dim = matches!(st, "queued" | "canceled");
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
            "canceled" => {
                row.status = vec![Seg("canceled".into(), Style::dim(style::MUTED))];
            }
            "queued" | "waiting" | "ready" | "unassigned" | "needs_answer" => {
                row.status = queued_status(ix, ix.task_index(n));
            }
            _ => {
                row.status = vec![Seg("queued".into(), Style::dim(style::QUEUED))];
            }
        }
    }
    // Gate: a state-bearing join strip on every gate row, attempted or not
    // (F2). Exact input ids unroll on selection; at rest the ordered marks
    // make the many-to-one topology and stream state visible at a glance.
    // waits/ready only replaces the state word before the gate has an attempt.
    if kind == "gate" {
        let inputs = task.inputs.clone().unwrap_or_else(|| task.deps.clone());
        if !inputs.is_empty() {
            let mut states = Vec::with_capacity(inputs.len());
            for input in &inputs {
                let ist = ix
                    .task_by_id
                    .get(input.as_str())
                    .map(|&ti| ix.task_signal(ti))
                    .unwrap_or("invalid");
                states.push(ist.to_string());
            }
            row.join = Some(GateJoin { states });
            row.glyph = style::state_glyph("working");
            if ix.attempt(n).is_none() && row.state == "queued" {
                row.status = queued_status(ix, ix.task_index(n));
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
        if kind != "gate" {
            let this_ti = ix.task_index(n);
            for (di, d) in task.deps.iter().enumerate() {
                let cross_project = ix
                    .task_by_id
                    .get(d.as_str())
                    .is_some_and(|&dep_ti| !ix.same_project(this_ti, dep_ti));
                if di > 0 || cross_project {
                    row.chips.push(Seg(format!("⇠ {d} "), Style::dim(style::MUTED)));
                }
            }
        }
        if let Some(note) = task.note.as_deref() {
            // producers sometimes write the ⇠ themselves; one is enough
            let note = note.trim_start_matches("⇠ ");
            row.chips.push(Seg(format!("⇠ {note} "), Style::dim(style::MUTED)));
        }
        if let Some(criteria) = task.criteria.as_deref() {
            row.chips.push(Seg(format!("✓ {criteria} "), Style::dim(style::MUTED)));
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
        let current = ix.current_node(ti);
        let name = ix.display_name(current);
        let ev = ix
            .attempt(current)
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
                {"id": "C", "title": "impl: c", "state": "blocked", "unblock": "operator", "deps": ["A"],
                 "attempts": [{"id": "C·a1", "n": 1, "state": "working",
                               "started_at": "2026-08-16T10:25:00Z"}]},
                {"id": "D", "title": "impl: d", "state": "done", "deps": ["B"], "attempts": []}
              ]
            }"#,
        )
        .expect("mini doc parses")
    }

    fn queued_tasks(tasks: serde_json::Value) -> Doc {
        serde_json::from_value(serde_json::json!({
            "dagr": 1,
            "run": {"id": "r", "title": "queued"},
            "tasks": tasks
        }))
        .expect("queued doc parses")
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
    fn all_queued_dependencies_render_as_a_connected_spine() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "a", "state": "queued", "deps": [], "attempts": []},
            {"id": "B", "title": "b", "state": "queued", "deps": ["A"], "attempts": []},
            {"id": "C", "title": "c", "state": "queued", "deps": ["B"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["A", "B", "C"]);
        assert_eq!(scene.rows.iter().map(|r| r.parent.as_deref()).collect::<Vec<_>>(), [
            None,
            Some("A"),
            Some("B"),
        ]);
        assert_eq!(scene.rows.iter().map(|r| r.rail.as_str()).collect::<Vec<_>>(), [
            " ",
            " ╰─",
            "   ╰─",
        ]);
    }

    #[test]
    fn cyclic_queued_dependencies_do_not_loop_in_full_zoomed_or_folded_walks() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "a", "state": "queued", "deps": ["C"], "attempts": []},
            {"id": "B", "title": "b", "state": "queued", "deps": ["A"], "attempts": []},
            {"id": "C", "title": "c", "state": "queued", "deps": ["B"], "attempts": []}
        ]));

        let full = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&full), ["A", "B", "C"], "rootless cycles fall back to flat rows");
        assert_eq!(ancestors(&doc, "A"), ["C", "B", "A"], "ancestor walk stops at the cycle");

        let zoomed = build(
            &doc,
            None,
            None,
            None,
            &ViewOpts { zoom: Some("A"), folded: None },
        );
        assert_eq!(keys(&zoomed), ["A", "B", "C"], "zoomed cycle visits each node once");

        let fold_set: std::collections::HashSet<String> = ["A".to_string()].into();
        let folded_scene = build(
            &doc,
            None,
            None,
            None,
            &ViewOpts { zoom: Some("A"), folded: Some(&fold_set) },
        );
        assert_eq!(keys(&folded_scene), ["A"]);
        assert_eq!(folded_scene.rows[0].name, "A");
        assert!(folded_scene.rows[0].title.contains("3 items"));
    }

    #[test]
    fn attempted_dependency_remains_the_parent_of_a_queued_stub() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "a", "state": "done", "deps": [], "attempts": [
                {"id": "A·a1", "n": 1, "state": "done", "started_at": "2026-08-16T10:00:00Z"}
            ]},
            {"id": "B", "title": "b", "state": "queued", "deps": ["A"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["A·a1", "B"]);
        assert_eq!(scene.rows[1].parent.as_deref(), Some("A·a1"));
        assert_eq!(scene.rows[1].rail, " ╰─");
    }

    #[test]
    fn all_stub_fanin_is_a_scope_milestone_not_a_lane_child() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "R", "title": "root", "state": "queued", "deps": [], "attempts": []},
            {"id": "A1", "title": "lane a plan", "state": "queued", "deps": ["R"], "attempts": []},
            {"id": "A2", "title": "lane a review", "state": "queued", "deps": ["A1"], "attempts": []},
            {"id": "B1", "title": "lane b plan", "state": "queued", "deps": ["R"], "attempts": []},
            {"id": "B2", "title": "lane b review", "state": "queued", "deps": ["B1"], "attempts": []},
            {"id": "G", "title": "join lanes", "kind": "gate", "state": "queued",
             "deps": ["A2", "B2"], "attempts": []},
            {"id": "OUT", "title": "integration", "state": "queued", "deps": ["G"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["R", "A1", "A2", "B1", "B2", "G", "OUT"]);
        let gate = scene.rows.iter().find(|r| r.key == "G").expect("gate row");
        assert_eq!(gate.parent, None);
        assert!(gate.milestone);
        assert_eq!(
            gate.join.as_ref().map(|j| j.states.as_slice()),
            Some(["waiting".to_string(), "waiting".to_string()].as_slice())
        );
        assert_eq!(
            scene.rows.iter().find(|r| r.key == "OUT").unwrap().parent.as_deref(),
            Some("G")
        );
    }

    #[test]
    fn all_stub_fanin_without_a_shared_ancestor_stays_at_root() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "a", "state": "queued", "deps": [], "attempts": []},
            {"id": "B", "title": "b", "state": "queued", "deps": [], "attempts": []},
            {"id": "G", "title": "join", "kind": "gate", "state": "queued",
             "deps": ["A", "B"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(
            scene.rows.iter().find(|r| r.key == "G").unwrap().parent,
            None
        );
    }

    #[test]
    fn deep_retry_history_never_claims_ownership_of_a_gate() {
        let retries = |task: &str| {
            (1..=10)
                .map(|n| {
                    let mut attempt = serde_json::json!({
                        "id": format!("{task}·a{n}"),
                        "n": n,
                        "state": "failed"
                    });
                    if n > 1 {
                        attempt["cause"] = serde_json::json!({
                            "type": "followup",
                            "ref": format!("{task}·a{}", n - 1)
                        });
                    }
                    attempt
                })
                .collect::<Vec<_>>()
        };
        let doc = queued_tasks(serde_json::json!([
            {"id": "R", "title": "root", "state": "done", "deps": [], "attempts": [
                {"id": "R·a1", "n": 1, "state": "done"}
            ]},
            {"id": "AW", "title": "lane a history", "state": "failed", "deps": ["R"],
             "attempts": retries("AW")},
            {"id": "A", "title": "lane a input", "state": "queued", "deps": ["AW"], "attempts": []},
            {"id": "BW", "title": "lane b history", "state": "failed", "deps": ["R"],
             "attempts": retries("BW")},
            {"id": "B", "title": "lane b input", "state": "queued", "deps": ["BW"], "attempts": []},
            {"id": "G", "title": "join", "kind": "gate", "state": "queued",
             "deps": ["A", "B"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(scene.rows.iter().find(|r| r.key == "G").unwrap().parent, None);
    }

    #[test]
    fn attempted_inputs_do_not_change_gate_placement() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "a", "state": "done", "deps": [], "attempts": [
                {"id": "A·a1", "n": 1, "state": "done", "started_at": "2026-08-16T10:00:00Z"}
            ]},
            {"id": "B", "title": "b", "state": "done", "deps": [], "attempts": [
                {"id": "B·a1", "n": 1, "state": "done", "started_at": "2026-08-16T10:10:00Z"}
            ]},
            {"id": "G", "title": "join", "kind": "gate", "state": "queued",
             "deps": ["A", "B"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["A·a1", "B·a1", "G"]);
        assert_eq!(scene.rows.iter().find(|r| r.key == "G").unwrap().parent, None);
    }

    #[test]
    fn recursive_projects_place_local_shared_and_global_gates_at_their_scope() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "r", "title": "projects"},
            "projects": [
                {"id": "P", "title": "Product"},
                {"id": "A", "title": "Stream A", "parent": "P"},
                {"id": "B", "title": "Stream B", "parent": "P"},
                {"id": "C", "title": "Independent"}
            ],
            "tasks": [
                {"id": "A1", "title": "a", "project": "A", "state": "queued", "deps": [], "attempts": []},
                {"id": "A2", "title": "a2", "project": "A", "state": "queued", "deps": ["A1"], "attempts": []},
                {"id": "AG", "title": "local gate", "kind": "gate", "state": "queued", "deps": ["A1", "A2"], "attempts": []},
                {"id": "B1", "title": "b", "project": "B", "state": "queued", "deps": [], "attempts": []},
                {"id": "PG", "title": "product gate", "kind": "gate", "state": "queued", "deps": ["A2", "B1"], "attempts": []},
                {"id": "C1", "title": "c", "project": "C", "state": "queued", "deps": [], "attempts": []},
                {"id": "RG", "title": "run gate", "kind": "gate", "state": "queued", "deps": ["PG", "C1"], "attempts": []}
            ]
        }))
        .unwrap();

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let pos = |key: &str| scene.rows.iter().position(|r| r.key == key).unwrap();
        assert!(pos("project:A") < pos("AG"), "local gate stays in stream A");
        assert!(pos("project:B") < pos("PG"), "shared gate follows both child streams");
        assert!(pos("PG") < pos("project:C"), "product gate remains inside product project");
        assert!(pos("project:C") < pos("RG"), "cross-project gate is a run-level milestone");
        assert_eq!(scene.rows.iter().find(|r| r.key == "AG").unwrap().rail, "    ╰─");
        assert_eq!(scene.rows.iter().find(|r| r.key == "PG").unwrap().rail, "  ╰─");
        assert_eq!(scene.rows.iter().find(|r| r.key == "RG").unwrap().rail, " ");
    }

    #[test]
    fn shared_fanin_scope_resolution_is_memoized_per_task() {
        let mut tasks = vec![
            serde_json::json!({
                "id": "F0", "kind": "impl", "project": "P", "state": "done",
                "deps": [], "attempts": []
            }),
            serde_json::json!({
                "id": "F1", "kind": "impl", "project": "P", "state": "done",
                "deps": [], "attempts": []
            }),
        ];
        for i in 2..32 {
            tasks.push(serde_json::json!({
                "id": format!("F{i}"), "kind": "gate", "state": "queued", "deps": [],
                "inputs": [format!("F{}", i - 1), format!("F{}", i - 2)], "attempts": []
            }));
        }
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 3,
            "run": {"id": "shared-fanin"},
            "projects": [{"id": "P", "title": "Core"}],
            "tasks": tasks
        }))
        .unwrap();

        let ix = Ix::new(&doc);
        assert_eq!(ix.scope_resolution_evaluations, 32, "each task body resolves once");
        assert!(
            ix.scope_resolution_calls <= 32 * 3,
            "one outer lookup plus at most two fan-in edges per task: {} calls",
            ix.scope_resolution_calls
        );
        assert!((0..32).all(|ti| ix.task_project(ti) == Some(0)));
    }

    #[test]
    fn project_summary_does_not_hide_lost_failed_or_unverified_work() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "r"},
            "projects": [{"id": "P", "title": "Product"}],
            "tasks": [
                {"id": "L", "title": "lost", "project": "P", "state": "failed", "deps": [],
                 "attempts": [{"id": "L·a1", "n": 1, "state": "lost"}]},
                {"id": "F", "title": "failed", "project": "P", "state": "failed", "deps": [], "attempts": []},
                {"id": "U", "title": "unverified", "project": "P", "state": "settled_unverified", "deps": [],
                 "attempts": [{"id": "U·a1", "n": 1, "state": "settled_unverified"}]},
                {"id": "C", "title": "canceled", "project": "P", "state": "canceled", "deps": [], "attempts": []},
                {"id": "D", "title": "done", "project": "P", "state": "done", "deps": [], "attempts": []}
            ]
        }))
        .unwrap();

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let project = scene.rows.iter().find(|r| r.key == "project:P").unwrap();
        let status = project.status.iter().map(|seg| seg.0.as_str()).collect::<String>();
        assert!(status.contains("1 lost"), "{status}");
        assert!(status.contains("1 failed"), "{status}");
        assert!(status.contains("1 unverified"), "{status}");
        assert!(status.contains("1 canceled"), "{status}");
        assert!(status.contains("1 done"), "{status}");
        assert_eq!(project.state, "lost");
        assert!(project.hot);
    }

    #[test]
    fn cross_project_dependency_is_an_edge_chip_not_a_false_visual_home() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "r"},
            "projects": [{"id": "A", "title": "A"}, {"id": "B", "title": "B"}],
            "tasks": [
                {"id": "A1", "title": "a", "project": "A", "state": "queued", "deps": [], "attempts": []},
                {"id": "B1", "title": "b", "project": "B", "state": "queued", "deps": ["A1"], "attempts": []}
            ]
        }))
        .unwrap();
        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let b = scene.rows.iter().find(|r| r.key == "B1").unwrap();
        assert_eq!(b.parent.as_deref(), Some("project:B"));
        assert!(b.chips.iter().any(|s| s.0.contains("⇠ A1")));
    }

    #[test]
    fn gate_join_glyph_keeps_blocked_and_review_attention_overlays() {
        for (task_state, expected_color, status_word) in [
            ("blocked", style::BLOCKED, "BLOCKED "),
            ("review", style::REVIEW, "review "),
        ] {
            let doc = queued_tasks(serde_json::json!([
                {"id": "A", "title": "input", "state": "done", "deps": [], "attempts": [
                    {"id": "A·a1", "n": 1, "state": "done"}
                ]},
                {"id": "G", "title": "join", "kind": "gate", "owner": "reviewer",
                 "unblock": "operator", "state": task_state, "deps": ["A"], "attempts": [
                    {"id": "G·a1", "n": 1, "state": "working"}
                 ]}
            ]));

            let scene = build(&doc, None, None, None, &ViewOpts::default());
            let gate = scene.rows.iter().find(|r| r.name == "G").expect("gate row");
            assert_eq!(gate.glyph, style::state_glyph("working"));
            assert_eq!(gate.glyph_color, expected_color);
            assert_eq!(gate.status.first().map(|s| s.0.as_str()), Some(status_word));
        }

        let canceled = queued_tasks(serde_json::json!([
            {"id": "A", "title": "input", "state": "done", "deps": [], "attempts": [
                {"id": "A·a1", "n": 1, "state": "done"}
            ]},
            {"id": "G", "title": "withdrawn join", "kind": "gate",
             "state": "canceled", "deps": ["A"], "attempts": []}
        ]));
        let scene = build(&canceled, None, None, None, &ViewOpts::default());
        let gate = scene.rows.iter().find(|r| r.name == "G").expect("gate row");
        assert_eq!(gate.glyph, style::state_glyph("working"));
        assert_eq!(gate.glyph_color, style::MUTED);
        assert_eq!(gate.status.first().map(|s| s.0.as_str()), Some("canceled"));
    }

    #[test]
    fn cyclic_all_stub_gates_do_not_recurse_forever() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "G1", "title": "one", "kind": "gate", "state": "queued",
             "deps": ["G2"], "attempts": []},
            {"id": "G2", "title": "two", "kind": "gate", "state": "queued",
             "deps": ["G1"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["G1", "G2"]);
    }

    #[test]
    fn attempt_less_siblings_follow_task_declaration_order() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "R", "title": "root", "state": "queued", "deps": [], "attempts": []},
            {"id": "Z-first", "title": "declared first", "state": "queued", "deps": ["R"], "attempts": []},
            {"id": "A-second", "title": "declared second", "state": "queued", "deps": ["R"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["R", "Z-first", "A-second"]);
    }

    #[test]
    fn queued_signals_are_derived_from_existing_task_facts() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "D", "title": "done", "kind": "impl", "state": "done", "deps": [],
             "attempts": [{"id": "D·a1", "n": 1, "state": "done"}]},
            {"id": "R", "title": "ready", "kind": "impl", "owner": "dev",
             "state": "queued", "deps": ["D"], "attempts": []},
            {"id": "W", "title": "waiting", "kind": "impl", "owner": "dev",
             "state": "queued", "deps": ["R"], "attempts": []},
            {"id": "U", "title": "unassigned", "kind": "impl",
             "state": "queued", "deps": ["D"], "attempts": []},
            {"id": "Q", "title": "choose", "kind": "question", "owner": "operator",
             "state": "queued", "deps": ["D"], "attempts": []},
            {"id": "C", "title": "withdrawn", "kind": "impl",
             "state": "canceled", "deps": ["D"], "attempts": []},
            {"id": "X", "title": "still blocked", "kind": "impl", "owner": "dev",
             "state": "queued", "deps": ["C"], "attempts": []},
            {"id": "G", "title": "unowned join", "kind": "gate",
             "state": "queued", "deps": [], "inputs": ["D"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let status = |key: &str| {
            scene.rows
                .iter()
                .find(|row| row.key == key)
                .unwrap()
                .status
                .iter()
                .map(|seg| seg.0.as_str())
                .collect::<String>()
        };
        assert_eq!(status("R"), "ready");
        assert_eq!(status("W"), "waits R");
        assert_eq!(status("U"), "unassigned");
        assert_eq!(status("Q"), "needs answer");
        assert_eq!(status("X"), "waits C", "cancellation is not success");
        assert_eq!(status("G"), "unassigned", "gates use the same assignment rule");
        assert_eq!(scene.queue.len(), 1, "only questions enter attention");
        assert_eq!(scene.queue[0].task_id, "Q");
        assert_eq!(scene.queue[0].state, "needs_answer");
    }

    #[test]
    fn queued_attempt_actor_drives_every_task_projection() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 3,
            "run": {"id": "attempt-owner"},
            "projects": [{"id": "P", "title": "Core"}],
            "tasks": [
                {"id": "ROOT", "title": "root", "kind": "impl", "project": "P",
                 "state": "done", "deps": [], "attempts": [
                    {"id": "ROOT·a1", "n": 1, "state": "done"}
                 ]},
                {"id": "A", "title": "assigned by attempt", "kind": "impl", "project": "P",
                 "state": "queued", "deps": ["ROOT"], "attempts": [
                    {"id": "A·a1", "n": 1, "state": "queued", "actor": "dev"}
                 ]}
            ]
        }))
        .unwrap();

        let open = build(&doc, None, None, None, &ViewOpts::default());
        let task = open.rows.iter().find(|row| row.task_id == "A").unwrap();
        let task_status = task.status.iter().map(|seg| seg.0.as_str()).collect::<String>();
        assert_eq!((task.state.as_str(), task_status.as_str()), ("ready", "ready"));
        assert_eq!(selection_state(&doc, "A").as_deref(), Some("ready"));
        assert_eq!(selection_state(&doc, "A·a1").as_deref(), Some("ready"));
        let project = open.rows.iter().find(|row| row.key == "project:P").unwrap();
        let project_status = project.status.iter().map(|seg| seg.0.as_str()).collect::<String>();
        assert!(project_status.contains("1 ready"), "{project_status}");
        assert!(!project_status.contains("unassigned"), "{project_status}");
        assert!(open.queue.iter().all(|item| item.task_id != "A"));

        let folded: std::collections::HashSet<String> = ["ROOT·a1".to_string()].into();
        let closed = build(
            &doc,
            None,
            None,
            None,
            &ViewOpts { zoom: None, folded: Some(&folded) },
        );
        let aggregate = closed
            .rows
            .iter()
            .find(|row| row.key == "ROOT·a1")
            .and_then(|row| row.fold.as_ref())
            .unwrap()
            .segs
            .iter()
            .map(|seg| seg.0.as_str())
            .collect::<String>();
        assert!(aggregate.contains("1 ready"), "{aggregate}");
        assert!(!aggregate.contains("unassigned"), "{aggregate}");

        let focus = crate::render::focus_card(&doc, "A", 48, None, &[]).join("\n");
        assert!(focus.contains("· READY"), "{focus}");
        assert!(!focus.contains("QUEUED"), "{focus}");
    }

    #[test]
    fn cancellation_adds_a_stub_without_repainting_attempt_history() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "C", "title": "withdrawn", "kind": "impl", "state": "canceled",
             "deps": [], "attempts": [
                {"id": "C·a1", "n": 1, "state": "done"},
                {"id": "C·a2", "n": 2, "state": "lost",
                 "cause": {"type": "followup", "ref": "C·a1"}}
             ]}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let first = scene.rows.iter().find(|row| row.key == "C·a1").unwrap();
        let latest = scene.rows.iter().find(|row| row.key == "C·a2").unwrap();
        let current = scene.rows.iter().find(|row| row.key == "C").unwrap();
        assert_eq!((first.state.as_str(), first.glyph), ("done", '●'));
        assert_eq!((latest.state.as_str(), latest.glyph), ("lost", '?'));
        assert_eq!((current.state.as_str(), current.glyph), ("canceled", '×'));
        assert_eq!(scene.queue.iter().map(|item| item.state.as_str()).collect::<Vec<_>>(), ["lost"]);
        assert!(settled_roots(&doc).is_empty());
    }

    #[test]
    fn a_queued_retry_gets_a_current_stub_without_repainting_history() {
        let doc = queued_tasks(serde_json::json!([
            {"id": "D", "title": "foundation", "kind": "impl", "state": "done",
             "deps": [], "attempts": [{"id": "D·a1", "n": 1, "state": "done"}]},
            {"id": "R", "title": "retry", "kind": "impl", "owner": "dev",
             "state": "queued", "deps": ["D"], "attempts": [
                {"id": "R·a1", "n": 1, "state": "failed"}
             ]},
            {"id": "X", "title": "downstream", "kind": "impl", "owner": "dev",
             "state": "queued", "deps": ["R"], "attempts": []}
        ]));

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(keys(&scene), ["D·a1", "R·a1", "R", "X"]);
        let failed = scene.rows.iter().find(|row| row.key == "R·a1").unwrap();
        let current = scene.rows.iter().find(|row| row.key == "R").unwrap();
        let downstream = scene.rows.iter().find(|row| row.key == "X").unwrap();
        assert_eq!((failed.state.as_str(), failed.glyph), ("failed", '✗'));
        assert_eq!((current.state.as_str(), current.glyph), ("ready", '○'));
        assert_eq!(current.parent.as_deref(), Some("R·a1"));
        assert_eq!(downstream.parent.as_deref(), Some("R"));
        assert_eq!(
            current.status.iter().map(|seg| seg.0.as_str()).collect::<String>(),
            "ready"
        );
        assert_eq!(
            downstream.status.iter().map(|seg| seg.0.as_str()).collect::<String>(),
            "waits R"
        );
        assert!(settled_roots(&doc).is_empty(), "queued retry prevents auto-fold");
    }

    #[test]
    fn a_fold_hides_the_subtree_but_not_the_alarm() {
        let doc = mini();
        let folded: std::collections::HashSet<String> = ["A·a1".to_string()].into();
        let scene = build(&doc, None, None, None, &ViewOpts { zoom: None, folded: Some(&folded) });
        assert_eq!(keys(&scene), ["A·a1"], "fallback must not resurrect folded rows");
        let chip = scene.rows[0].fold.as_ref().expect("fold chip");
        // C's working attempt displays as BLOCKED (task state outranks) —
        // the chip must carry that, in blocked ink
        assert_eq!(chip.hot, Some(crate::style::BLOCKED));
        let text: String = chip.segs.iter().map(|Seg(s, _)| s.as_str()).collect();
        assert_eq!(scene.rows[0].glyph, '▸');
        assert_eq!(scene.rows[0].name, "A");
        assert!(scene.rows[0].title.contains("4 items"));
        assert!(text.contains("1 blocked") && text.contains("3 done"), "{text}");
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
    fn ancestor_walk_is_bounded_by_identity_not_an_arbitrary_depth_cap() {
        let attempts = (1..=80)
            .map(|n| {
                let mut attempt = serde_json::json!({
                    "id": format!("A·a{n}"),
                    "n": n,
                    "state": "failed"
                });
                if n > 1 {
                    attempt["cause"] = serde_json::json!({
                        "type": "followup",
                        "ref": format!("A·a{}", n - 1)
                    });
                }
                attempt
            })
            .collect::<Vec<_>>();
        let doc = queued_tasks(serde_json::json!([
            {"id": "A", "title": "many retries", "state": "failed", "deps": [], "attempts": attempts}
        ]));

        let chain = ancestors(&doc, "A·a80");
        assert_eq!(chain.len(), 79);
        assert_eq!(chain.first().map(String::as_str), Some("A·a79"));
        assert_eq!(chain.last().map(String::as_str), Some("A·a1"));
    }

    #[test]
    fn search_matches_ids_titles_and_agents() {
        let doc = mini();
        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let c = scene.rows.iter().find(|r| r.key == "C·a1").unwrap();
        assert!(row_matches(c, "c·a1") && row_matches(c, "impl: c") && row_matches(c, "C"));
        assert!(!row_matches(c, "impl: d"));
    }

    fn project_navigation_doc() -> Doc {
        serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "projects", "title": "project navigation"},
            "projects": [
                {"id": "ROOT", "title": "Recovery"},
                {"id": "CHILD", "title": "Core", "parent": "ROOT"},
                {"id": "EMPTY", "title": "Empty", "parent": "ROOT"},
                {"id": "OUT", "title": "Outside"}
            ],
            "tasks": [
                {"id": "PLAN", "title": "plan", "kind": "plan", "project": "CHILD",
                 "owner": "lead", "state": "done", "deps": [], "attempts": [
                    {"id": "PLAN·a1", "n": 1, "state": "done"}
                 ]},
                {"id": "DEV", "title": "develop", "kind": "impl", "project": "CHILD",
                 "owner": "dev", "state": "queued", "deps": ["PLAN"], "attempts": []},
                {"id": "OUTSIDE", "title": "outside", "kind": "impl", "project": "OUT",
                 "state": "queued", "deps": [], "attempts": []}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn projects_are_selectable_foldable_nodes_with_connected_navigation() {
        let doc = project_navigation_doc();
        let open = build(&doc, None, None, None, &ViewOpts::default());
        let row = |key: &str| open.rows.iter().find(|row| row.key == key).unwrap();

        assert!(row("project:ROOT").selectable);
        assert!(row("project:ROOT").has_kids);
        assert_eq!(row("project:CHILD").parent.as_deref(), Some("project:ROOT"));
        assert_eq!(row("PLAN·a1").parent.as_deref(), Some("project:CHILD"));
        assert_eq!(row("DEV").parent.as_deref(), Some("PLAN·a1"));
        assert_eq!(
            ancestors(&doc, "DEV"),
            ["PLAN·a1", "project:CHILD", "project:ROOT"]
        );
        assert!(key_exists(&doc, "project:CHILD"));

        let folded: std::collections::HashSet<String> = ["project:ROOT".to_string()].into();
        let closed = build(
            &doc,
            Some("project:ROOT"),
            None,
            None,
            &ViewOpts {
                zoom: None,
                folded: Some(&folded),
            },
        );
        assert_eq!(
            closed.rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            ["project:ROOT", "project:OUT", "OUTSIDE"]
        );
        let root = closed.rows.iter().find(|row| row.key == "project:ROOT").unwrap();
        assert_eq!(root.glyph, '▸');
        assert!(root.fold.is_some());
        assert_eq!(root.name, "ROOT", "a project fold preserves its identity");

        let zoomed = build(
            &doc,
            Some("project:CHILD"),
            None,
            None,
            &ViewOpts {
                zoom: Some("project:CHILD"),
                folded: None,
            },
        );
        assert_eq!(
            zoomed.rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>(),
            ["project:CHILD", "PLAN·a1", "DEV"]
        );
    }

    #[test]
    fn local_and_cross_project_gates_have_truthful_structural_parents() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "gates"},
            "projects": [
                {"id": "P", "title": "Product"},
                {"id": "A", "title": "A", "parent": "P"},
                {"id": "B", "title": "B", "parent": "P"},
                {"id": "C", "title": "C"}
            ],
            "tasks": [
                {"id": "A1", "title": "a1", "kind": "impl", "project": "A",
                 "state": "queued", "deps": [], "attempts": []},
                {"id": "A2", "title": "a2", "kind": "impl", "project": "A",
                 "state": "queued", "deps": ["A1"], "attempts": []},
                {"id": "LOCAL", "title": "local", "kind": "gate", "state": "queued",
                 "inputs": ["A1", "A2"], "deps": [], "attempts": []},
                {"id": "B1", "title": "b1", "kind": "impl", "project": "B",
                 "state": "queued", "deps": [], "attempts": []},
                {"id": "SHARED", "title": "shared", "kind": "gate", "state": "queued",
                 "inputs": ["A2", "B1"], "deps": [], "attempts": []},
                {"id": "C1", "title": "c1", "kind": "impl", "project": "C",
                 "state": "queued", "deps": [], "attempts": []},
                {"id": "CROSS", "title": "cross", "kind": "gate", "state": "queued",
                 "inputs": ["SHARED", "C1"], "deps": [], "attempts": []}
            ]
        }))
        .unwrap();

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        let parent = |key: &str| {
            scene
                .rows
                .iter()
                .find(|row| row.key == key)
                .unwrap()
                .parent
                .as_deref()
        };
        assert_eq!(parent("LOCAL"), Some("project:A"));
        assert_eq!(parent("SHARED"), Some("project:P"));
        assert_eq!(parent("CROSS"), None, "cross-top-level gates live at the run root");
    }

    #[test]
    fn folded_and_project_rollups_share_ready_waiting_and_needs_answer_signals() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "signals"},
            "projects": [{"id": "P", "title": "Signals"}],
            "tasks": [
                {"id": "ROOT", "title": "root", "kind": "impl", "project": "P",
                 "state": "done", "deps": [], "attempts": [
                    {"id": "ROOT·a1", "n": 1, "state": "done"}
                 ]},
                {"id": "READY", "title": "ready", "kind": "impl", "project": "P",
                 "owner": "dev", "state": "queued", "deps": ["ROOT"], "attempts": []},
                {"id": "WAIT", "title": "waiting", "kind": "impl", "project": "P",
                 "owner": "dev", "state": "queued", "deps": ["READY"], "attempts": []},
                {"id": "QUESTION", "title": "choose", "kind": "question", "project": "P",
                 "owner": "operator", "state": "queued", "deps": ["ROOT"], "attempts": []}
            ]
        }))
        .unwrap();

        let open = build(&doc, None, None, None, &ViewOpts::default());
        let status = |key: &str| {
            open.rows
                .iter()
                .find(|row| row.key == key)
                .unwrap()
                .status
                .iter()
                .map(|seg| seg.0.as_str())
                .collect::<String>()
        };
        assert_eq!(status("READY"), "ready");
        assert_eq!(status("WAIT"), "waits READY");
        assert_eq!(status("QUESTION"), "needs answer");
        let project = status("project:P");
        assert!(project.contains("1 ready"), "{project}");
        assert!(project.contains("1 waiting"), "{project}");
        assert!(project.contains("1 needs answer"), "{project}");

        let folded: std::collections::HashSet<String> = ["ROOT·a1".to_string()].into();
        let closed = build(
            &doc,
            None,
            None,
            None,
            &ViewOpts {
                zoom: None,
                folded: Some(&folded),
            },
        );
        let aggregate = closed
            .rows
            .iter()
            .find(|row| row.key == "ROOT·a1")
            .unwrap()
            .fold
            .as_ref()
            .unwrap()
            .segs
            .iter()
            .map(|seg| seg.0.as_str())
            .collect::<String>();
        assert!(aggregate.contains("1 ready"), "{aggregate}");
        assert!(aggregate.contains("1 waiting"), "{aggregate}");
        assert!(aggregate.contains("1 needs answer"), "{aggregate}");
    }

    #[test]
    fn canceled_tasks_keep_history_but_only_live_or_lost_work_alarms_rollups() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "canceled"},
            "projects": [{"id": "P", "title": "Canceled"}],
            "tasks": [
                {"id": "QUIET", "title": "quiet", "kind": "impl", "project": "P",
                 "state": "canceled", "deps": [], "attempts": [
                    {"id": "QUIET·a1", "n": 1, "state": "failed"}
                 ]},
                {"id": "LIVE", "title": "live", "kind": "impl", "project": "P",
                 "state": "canceled", "deps": [], "attempts": [
                    {"id": "LIVE·a1", "n": 1, "state": "working"}
                 ]},
                {"id": "LOST", "title": "lost", "kind": "impl", "project": "P",
                 "state": "canceled", "deps": [], "attempts": [
                    {"id": "LOST·a1", "n": 1, "state": "lost"}
                 ]}
            ]
        }))
        .unwrap();

        let scene = build(&doc, None, None, None, &ViewOpts::default());
        for (key, state) in [
            ("QUIET·a1", "failed"),
            ("QUIET", "canceled"),
            ("LIVE·a1", "working"),
            ("LIVE", "canceled"),
            ("LOST·a1", "lost"),
            ("LOST", "canceled"),
        ] {
            assert_eq!(
                scene.rows.iter().find(|row| row.key == key).unwrap().state,
                state,
                "{key}"
            );
        }
        let project = scene.rows.iter().find(|row| row.key == "project:P").unwrap();
        let summary = project.status.iter().map(|seg| seg.0.as_str()).collect::<String>();
        assert!(summary.contains("1 canceled"), "quiet cancellation is counted: {summary}");
        assert!(summary.contains("1 working"), "live exception survives: {summary}");
        assert!(summary.contains("1 lost"), "lost exception survives: {summary}");
        assert!(!summary.contains("failed"), "discarded failure is not project truth: {summary}");
        assert_eq!(
            scene.queue.iter().map(|item| item.state.as_str()).collect::<Vec<_>>(),
            ["lost", "working"]
        );

        let folded: std::collections::HashSet<String> = ["project:P".to_string()].into();
        let closed = build(
            &doc,
            None,
            None,
            None,
            &ViewOpts { zoom: None, folded: Some(&folded) },
        );
        let aggregate = closed.rows[0]
            .fold
            .as_ref()
            .unwrap()
            .segs
            .iter()
            .map(|seg| seg.0.as_str())
            .collect::<String>();
        assert!(
            aggregate.contains("1 canceled")
                && aggregate.contains("1 working")
                && aggregate.contains("1 lost"),
            "{aggregate}"
        );
        assert!(!aggregate.contains("failed"), "discarded history stays out: {aggregate}");
    }

    #[test]
    fn malformed_row_identities_render_but_are_never_interactive() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "bad-identities"},
            "projects": [
                {"id": "DUP", "title": "left"},
                {"id": "DUP", "title": "right"},
                {"id": "", "title": "empty"},
                {"id": "X", "title": "project-key-collision"}
            ],
            "tasks": [
                {"id": "T", "title": "left", "kind": "impl", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "T", "title": "right", "kind": "impl", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "", "title": "empty", "kind": "impl", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "A", "title": "attempt-left", "kind": "impl", "state": "working",
                 "deps": [], "attempts": [{"id": "ATT", "n": 1, "state": "working"}]},
                {"id": "B", "title": "attempt-right", "kind": "impl", "state": "working",
                 "deps": [], "attempts": [{"id": "ATT", "n": 1, "state": "working"}]},
                {"id": "BAD\u{001b}", "title": "control-id", "kind": "impl", "state": "queued",
                 "deps": [], "attempts": []},
                {"id": "project:X", "title": "task-key-collision", "kind": "impl", "state": "queued",
                 "deps": [], "attempts": []}
            ]
        }))
        .unwrap();

        let scene = build(&doc, Some("T"), None, None, &ViewOpts::default());
        assert_eq!(scene.rows.iter().filter(|row| row.title == "left").count(), 2);
        assert_eq!(scene.rows.iter().filter(|row| row.title == "right").count(), 2);
        assert!(scene
            .rows
            .iter()
            .filter(|row| matches!(row.title.as_str(), "left" | "right" | "empty"))
            .all(|row| !row.selectable));
        assert!(scene
            .rows
            .iter()
            .filter(|row| matches!(row.title.as_str(), "attempt-left" | "attempt-right" | "control-id"))
            .all(|row| !row.selectable));
        assert!(scene
            .rows
            .iter()
            .filter(|row| row.title.ends_with("key-collision"))
            .all(|row| !row.selectable));
        assert!(scene.selected_task.is_none());
        assert!(!key_exists(&doc, "T"));
        assert!(!key_exists(&doc, "project:DUP"));
        assert!(!key_exists(&doc, "ATT"));
        assert!(!key_exists(&doc, "BAD\u{1b}"));
    }

    #[test]
    fn malformed_project_cycles_render_once_and_stop() {
        let doc: Doc = serde_json::from_value(serde_json::json!({
            "dagr": 2,
            "run": {"id": "cycle"},
            "projects": [
                {"id": "A", "title": "A", "parent": "B"},
                {"id": "B", "title": "B", "parent": "A"}
            ],
            "tasks": [
                {"id": "T", "title": "task", "kind": "impl", "project": "A",
                 "state": "queued", "deps": [], "attempts": []}
            ]
        }))
        .unwrap();
        let scene = build(&doc, None, None, None, &ViewOpts::default());
        assert_eq!(scene.rows.iter().filter(|row| row.project).count(), 2);
        assert_eq!(scene.rows.iter().filter(|row| row.task_id == "T").count(), 1);
    }
}
