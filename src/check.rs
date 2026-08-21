//! `dagr check` — lint a run-state document against contract v3.
//!
//! Errors mean the document misdescribes itself: dangling references,
//! duplicate identity, task states that contradict the attempt record.
//! Warnings mean representable-but-suspect: missing evidence tiers,
//! timestamps, locators, unblock owners. The producing agent's loop is
//! write → check → fix until clean.

use crate::contract::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

#[derive(Serialize)]
pub struct Finding {
    pub level: Level,
    pub code: &'static str,
    pub path: String,
    pub msg: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
    pub tasks: usize,
    pub attempts: usize,
    pub events: usize,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Error).count()
    }
    pub fn warnings(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Warning).count()
    }
}

/// Permissive ISO-8601 shape check: `YYYY-MM-DDTHH:MM[:SS[.frac]](Z|±HH:MM)?`.
/// Timestamp validity == "the renderer's parser accepts it". One parser
/// (model::parse_min: calendar ranges, seconds, offsets) serves both, so
/// check and view can never disagree about what a timestamp means.
fn is_iso(ts: &str) -> bool {
    crate::model::parse_min(ts).is_some()
}

fn project_lca<'a>(
    a: Option<&'a str>,
    b: Option<&'a str>,
    parents: &HashMap<&'a str, Option<&'a str>>,
) -> Option<&'a str> {
    let (Some(a), Some(b)) = (a, b) else { return None };
    let mut ancestors = HashSet::new();
    let mut cur = Some(a);
    while let Some(id) = cur {
        if !ancestors.insert(id) {
            break;
        }
        cur = parents.get(id).copied().flatten();
    }
    let mut seen = HashSet::new();
    let mut cur = Some(b);
    while let Some(id) = cur {
        if !seen.insert(id) {
            break;
        }
        if ancestors.contains(id) {
            return Some(id);
        }
        cur = parents.get(id).copied().flatten();
    }
    None
}

fn task_project<'a>(
    id: &'a str,
    tasks: &HashMap<&'a str, &'a Task>,
    parents: &HashMap<&'a str, Option<&'a str>>,
    resolving: &mut HashSet<&'a str>,
) -> Option<&'a str> {
    let task = tasks.get(id)?;
    if let Some(project) = task.project.as_deref() {
        return Some(project);
    }
    if task.kind.as_deref() != Some("gate") || !resolving.insert(id) {
        return None;
    }
    let inputs = task.inputs.as_ref().unwrap_or(&task.deps);
    let mut iter = inputs.iter().filter(|input| tasks.contains_key(input.as_str()));
    let scope = iter.next().and_then(|first| {
        iter.fold(task_project(first, tasks, parents, resolving), |common, input| {
            project_lca(common, task_project(input, tasks, parents, resolving), parents)
        })
    });
    resolving.remove(id);
    scope
}

fn project_contains<'a>(
    ancestor: &'a str,
    child: Option<&'a str>,
    parents: &HashMap<&'a str, Option<&'a str>>,
) -> bool {
    let mut cur = child;
    let mut seen = HashSet::new();
    while let Some(id) = cur {
        if id == ancestor {
            return true;
        }
        if !seen.insert(id) {
            break;
        }
        cur = parents.get(id).copied().flatten();
    }
    false
}

pub fn check(doc: &Doc) -> Report {
    let mut f: Vec<Finding> = Vec::new();
    macro_rules! err {
        ($code:expr, $path:expr, $($msg:tt)*) => {
            f.push(Finding { level: Level::Error, code: $code, path: $path.to_string(), msg: format!($($msg)*) })
        };
    }
    macro_rules! warn {
        ($code:expr, $path:expr, $($msg:tt)*) => {
            f.push(Finding { level: Level::Warning, code: $code, path: $path.to_string(), msg: format!($($msg)*) })
        };
    }
    macro_rules! ts {
        ($val:expr, $path:expr) => {
            if let Some(t) = $val {
                if !is_iso(t) {
                    err!("E180", $path, "unparseable timestamp {:?} (want ISO-8601, e.g. 2026-08-13T14:02:00Z)", t);
                }
            }
        };
    }

    // ── document head ────────────────────────────────────────────────
    match doc.dagr {
        None => err!("E100", "dagr", "missing contract version field \"dagr\" (expected {})", CONTRACT_VERSION),
        Some(v) if !CONTRACT_VERSIONS.contains(&v) => {
            err!("E100", "dagr", "unsupported contract version {} (this dagr speaks {})", v, CONTRACT_VERSIONS.iter().map(u64::to_string).collect::<Vec<_>>().join("|"))
        }
        _ => {}
    }
    match &doc.run {
        None => err!("E101", "run", "missing run block"),
        Some(r) => {
            if !r.id.as_deref().is_some_and(crate::contract::valid_identity) {
                err!("E101", "run.id", "run.id must be nonblank and terminal-safe");
            }
            ts!(&r.started_at, "run.started_at");
            if let Some(loc) = &r.orchestrator {
                if loc.pane.as_deref().unwrap_or("").is_empty()
                    && loc.agent.as_deref().unwrap_or("").is_empty()
                {
                    err!("E103", "run.orchestrator", "orchestrator locator names neither pane nor agent");
                }
            }
        }
    }
    if doc.generated_at.is_none() {
        warn!("W100", "generated_at", "missing generated_at — the pane cannot show staleness without it");
    }
    ts!(&doc.generated_at, "generated_at");

    let tasks: &[Task] = match &doc.tasks {
        None => {
            err!("E102", "tasks", "missing tasks array");
            &[]
        }
        Some(t) => t,
    };

    // ── recursive project scopes (v2) ───────────────────────────────
    let mut project_ids: HashSet<&str> = HashSet::new();
    let mut project_parent: HashMap<&str, Option<&str>> = HashMap::new();
    for (pi, p) in doc.projects.iter().enumerate() {
        let pp = format!("projects[{pi}]");
        let Some(id) = p.id.as_deref().filter(|id| crate::contract::valid_identity(id)) else {
            err!("E104", &pp, "project id must be nonblank and terminal-safe");
            continue;
        };
        if !project_ids.insert(id) {
            err!("E104", &pp, "duplicate project id {:?}", id);
        }
        if p.title.as_deref().unwrap_or("").is_empty() {
            err!("E104", format!("{pp}.title"), "project {id} missing title");
        }
        project_parent.insert(id, p.parent.as_deref());
    }
    for (pi, p) in doc.projects.iter().enumerate() {
        let Some(id) = p.id.as_deref().filter(|id| crate::contract::valid_identity(id)) else { continue };
        if let Some(parent) = p.parent.as_deref() {
            if !project_ids.contains(parent) {
                err!("E105", format!("projects[{pi}].parent"), "project {id} has unknown parent {:?}", parent);
                continue;
            }
        }
        let mut seen = HashSet::new();
        let mut cur = Some(id);
        while let Some(pid) = cur {
            if !seen.insert(pid) {
                err!("E106", format!("projects[{pi}].parent"), "project hierarchy cycles through {pid:?}");
                break;
            }
            cur = project_parent.get(pid).copied().flatten();
        }
    }

    // ── pass 1: identity indexes ─────────────────────────────────────
    let mut task_ids: HashSet<&str> = HashSet::new();
    let mut task_by_id: HashMap<&str, &Task> = HashMap::new();
    let mut attempt_ids: HashMap<&str, String> = HashMap::new(); // id -> path
    let mut future_node_ids: HashSet<&str> = HashSet::new();
    for (ti, t) in tasks.iter().enumerate() {
        if let Some(id) = t.id.as_deref().filter(|id| crate::contract::valid_identity(id)) {
            if !task_ids.insert(id) {
                err!("E110", format!("tasks[{ti}]"), "duplicate task id {:?}", id);
            }
            task_by_id.entry(id).or_insert(t);
        }
        for (ai, a) in t.attempts.iter().enumerate() {
            if let Some(id) = a.id.as_deref().filter(|id| crate::contract::valid_identity(id)) {
                let path = format!("tasks[{ti}].attempts[{ai}]");
                if let Some(prev) = attempt_ids.insert(id, path.clone()) {
                    err!("E130", path, "duplicate attempt id {:?} (also at {})", id, prev);
                }
            }
        }
    }
    // one namespace: cause.ref and event refs resolve across both, so a
    // shared id is ambiguous
    for (ti, t) in tasks.iter().enumerate() {
        if let Some(id) = t.id.as_deref() {
            if id
                .strip_prefix("project:")
                .is_some_and(|project| project_ids.contains(project))
            {
                err!(
                    "E113",
                    format!("tasks[{ti}].id"),
                    "task id {:?} collides with the selectable project row key",
                    id
                );
            }
        }
        for (ai, a) in t.attempts.iter().enumerate() {
            if let Some(id) = a.id.as_deref().filter(|id| crate::contract::valid_identity(id)) {
                if id
                    .strip_prefix("project:")
                    .is_some_and(|project| project_ids.contains(project))
                {
                    err!(
                        "E113",
                        format!("tasks[{ti}].attempts[{ai}].id"),
                        "attempt id {:?} collides with the selectable project row key",
                        id
                    );
                }
                if task_ids.contains(id) {
                    err!(
                        "E113",
                        format!("tasks[{ti}].attempts[{ai}]"),
                        "attempt id {:?} collides with a task id — refs would resolve ambiguously",
                        id
                    );
                }
            }
        }
        if let Some(p) = &t.policy {
            for (fi, fu) in p.futures.iter().enumerate() {
                if let Some(n) = &fu.node {
                    if let Some(id) = n.id.as_deref().filter(|id| crate::contract::valid_identity(id)) {
                        if !future_node_ids.insert(id) {
                            err!(
                                "E164",
                                format!("tasks[{ti}].policy.futures[{fi}]"),
                                "duplicate future node id {:?} — futures are identities too",
                                id
                            );
                        }
                    }
                }
            }
        }
    }

    // ── graph invariants: deps form a DAG, causes flow backward ──────
    {
        // dependency cycles (E122): DFS with colors over known task ids.
        // Gate `inputs` are edges too — a fan-in override that loops back
        // through a dependent is just as much a deadlock as a dep cycle,
        // and gates authored with inputs-only fan-ins must not route
        // around the only cycle check in the system.
        let dep_of: HashMap<&str, Vec<&str>> = tasks
            .iter()
            .filter_map(|t| t.id.as_deref().map(|id| {
                (id, t.deps.iter()
                    .chain(t.inputs.iter().flatten())
                    .map(String::as_str)
                    .filter(|d| task_ids.contains(d))
                    .collect())
            }))
            .collect();
        let mut color: HashMap<&str, u8> = HashMap::new(); // 1 visiting, 2 done
        let mut reported: HashSet<&str> = HashSet::new();
        fn dfs<'a>(
            id: &'a str,
            dep_of: &HashMap<&'a str, Vec<&'a str>>,
            color: &mut HashMap<&'a str, u8>,
            stack: &mut Vec<&'a str>,
        ) -> Option<Vec<String>> {
            match color.get(id) {
                Some(1) => {
                    let from = stack.iter().position(|s| *s == id).unwrap_or(0);
                    let mut cyc: Vec<String> = stack[from..].iter().map(|s| s.to_string()).collect();
                    cyc.push(id.to_string());
                    return Some(cyc);
                }
                Some(_) => return None,
                None => {}
            }
            color.insert(id, 1);
            stack.push(id);
            let mut found = None;
            for d in dep_of.get(id).map(Vec::as_slice).unwrap_or(&[]) {
                if let Some(c) = dfs(d, dep_of, color, stack) {
                    found = Some(c);
                    break;
                }
            }
            stack.pop();
            color.insert(id, 2);
            found
        }
        for t in tasks {
            let Some(id) = t.id.as_deref() else { continue };
            let mut stack = Vec::new();
            if let Some(cyc) = dfs(id, &dep_of, &mut color, &mut stack) {
                if cyc.iter().all(|c| !reported.contains(c.as_str())) {
                    for c in &cyc {
                        if let Some(known) = task_ids.get(c.as_str()) {
                            reported.insert(known);
                        }
                    }
                    err!(
                        "E122",
                        "tasks",
                        "dependency cycle: {} — a run is a DAG, not an ouroboros",
                        cyc.join(" → ")
                    );
                }
            }
        }

        // cause cycles (E135) + causes from the future (E136)
        let attempt_by_id: HashMap<&str, &crate::contract::Attempt> = tasks
            .iter()
            .flat_map(|t| t.attempts.iter())
            .filter_map(|a| a.id.as_deref().map(|id| (id, a)))
            .collect();
        for (ti, t) in tasks.iter().enumerate() {
            for (ai, a) in t.attempts.iter().enumerate() {
                let ap = format!("tasks[{ti}].attempts[{ai}].cause.ref");
                let aid = a.id.as_deref().unwrap_or("?");
                let Some(r) = a.cause.as_ref().and_then(|c| c.reference.as_deref()) else {
                    continue;
                };
                // walk the ref chain from here; a revisit of `aid` is a cycle
                let mut seen: HashSet<&str> = HashSet::new();
                let mut cur = Some(r);
                while let Some(c) = cur {
                    if c == aid || !seen.insert(c) {
                        err!("E135", &ap, "cause cycle through attempt {aid} — causes must point at the past");
                        break;
                    }
                    cur = attempt_by_id
                        .get(c)
                        .and_then(|x| x.cause.as_ref())
                        .and_then(|x| x.reference.as_deref());
                }
                if let (Some(mine), Some(theirs)) = (
                    a.started_at.as_deref().and_then(crate::model::parse_min),
                    attempt_by_id
                        .get(r)
                        .and_then(|x| x.started_at.as_deref())
                        .and_then(crate::model::parse_min),
                ) {
                    if theirs > mine {
                        err!(
                            "E136",
                            &ap,
                            "attempt {aid} is caused by {r:?}, which starts after it — causes point backward in time"
                        );
                    }
                }
            }
        }
    }

    // ── pass 2: per-task validation ──────────────────────────────────
    for (ti, t) in tasks.iter().enumerate() {
        let tid = t.id.as_deref().unwrap_or("?");
        let tp = format!("tasks[{ti}]");
        if !t.id.as_deref().is_some_and(crate::contract::valid_identity) {
            err!("E111", &tp, "task id must be nonblank and terminal-safe");
        }
        if t.title.as_deref().unwrap_or("").is_empty() {
            err!("E111", format!("{tp}.title"), "task {tid} missing title");
        }
        if t.kind.as_deref().unwrap_or("").is_empty() {
            err!("E111", format!("{tp}.kind"), "task {tid} missing kind");
        }
        if let Some(project) = t.project.as_deref() {
            if !project_ids.contains(project) {
                err!("E107", format!("{tp}.project"), "task {tid} names unknown project {:?}", project);
            }
        }
        let state = t.state.as_deref().unwrap_or("");
        if state.is_empty() {
            err!("E111", format!("{tp}.state"), "task {tid} missing state");
        } else if !TASK_STATES.contains(&state) {
            err!("E112", format!("{tp}.state"), "task {tid} has unknown state {:?} (want one of {})", state, TASK_STATES.join("|"));
        }

        for d in &t.deps {
            if !task_ids.contains(d.as_str()) {
                err!("E120", format!("{tp}.deps"), "task {tid} depends on unknown task {:?}", d);
            }
        }
        if let Some(inputs) = &t.inputs {
            for i in inputs {
                if !task_ids.contains(i.as_str()) {
                    err!("E121", format!("{tp}.inputs"), "gate {tid} has unknown input {:?}", i);
                }
            }
        }
        if t.kind.as_deref() == Some("gate") {
            if let Some(gate_project) = t.project.as_deref() {
                let inputs = t.inputs.as_ref().unwrap_or(&t.deps);
                for input in inputs {
                    if !task_by_id.contains_key(input.as_str()) {
                        continue;
                    }
                    let input_project = task_project(
                        input,
                        &task_by_id,
                        &project_parent,
                        &mut HashSet::new(),
                    );
                    if !project_contains(gate_project, input_project, &project_parent) {
                        err!(
                            "E108",
                            format!("{tp}.project"),
                            "gate {tid} is in project {gate_project:?}, but input {input:?} lives outside that scope"
                        );
                    }
                }
            }
        }
        // the fan-in set is `inputs`, defaulting to `deps`; empty either
        // way means the gate gates nothing
        if t.kind.as_deref() == Some("gate")
            && t.inputs.as_deref().map(<[String]>::is_empty).unwrap_or(t.deps.is_empty())
        {
            warn!("W202", &tp, "gate {tid} has an empty fan-in set — nothing to fan in");
        }
        if state == "blocked" && t.unblock.as_deref().unwrap_or("").is_empty() {
            warn!("W205", &tp, "blocked task {tid} names no unblock owner — never just a red mark");
        }

        // policy futures
        if let Some(p) = &t.policy {
            // `after` chains live inside ONE policy: a future may only wait
            // on a sibling node of the same task's policy
            let local_nodes: HashSet<&str> = p
                .futures
                .iter()
                .filter_map(|f| f.node.as_ref().and_then(|n| n.id.as_deref()))
                .collect();
            let after_of: HashMap<&str, &str> = p
                .futures
                .iter()
                .filter_map(|f| {
                    let id = f.node.as_ref().and_then(|n| n.id.as_deref())?;
                    Some((id, f.after.as_deref()?))
                })
                .collect();
            for (fi, fu) in p.futures.iter().enumerate() {
                let fp = format!("{tp}.policy.futures[{fi}]");
                match fu.on.as_deref() {
                    None => err!("E163", &fp, "future in task {tid} missing \"on\""),
                    Some(on) if !FUTURE_ON.contains(&on) => {
                        err!("E163", &fp, "future in task {tid} has unknown \"on\" {:?} (want {})", on, FUTURE_ON.join("|"))
                    }
                    _ => {}
                }
                match (&fu.reference, &fu.node) {
                    (None, None) => err!("E160", &fp, "future in task {tid} has neither \"ref\" nor \"node\""),
                    (Some(_), Some(_)) => err!("E160", &fp, "future in task {tid} has both \"ref\" and \"node\" — pick one"),
                    (Some(r), None) => {
                        if !task_ids.contains(r.as_str()) {
                            err!("E161", &fp, "future in task {tid} references unknown task {:?}", r);
                        }
                    }
                    (None, Some(n)) => {
                        match n.id.as_deref() {
                            None => err!("E164", &fp, "future node in task {tid} missing id"),
                            Some(id) if !crate::contract::valid_identity(id) => err!("E164", &fp, "future node in task {tid} has a blank or terminal-unsafe id"),
                            Some(id) => {
                                if task_ids.contains(id) || attempt_ids.contains_key(id) {
                                    err!("E164", &fp, "future node id {:?} collides with an existing task/attempt — a future is not yet real", id);
                                }
                            }
                        }
                        if let Some(attr) = n.attribution.as_deref() {
                            if !ATTRIBUTIONS.contains(&attr) {
                                err!("E164", &fp, "future node in task {tid} has unknown attribution {:?} (want {})", attr, ATTRIBUTIONS.join("|"));
                            }
                        }
                    }
                }
                if let Some(after) = fu.after.as_deref() {
                    if !local_nodes.contains(after) {
                        err!("E162", &fp, "future in task {tid} chains after {:?}, which is not a future node of this task's own policy", after);
                    } else {
                        // after-chain must terminate (no ouroboros here either)
                        let mut seen: HashSet<&str> = HashSet::new();
                        let mut cur = Some(after);
                        while let Some(c) = cur {
                            if !seen.insert(c) {
                                err!("E162", &fp, "future after-chain in task {tid} cycles through {:?}", c);
                                break;
                            }
                            cur = after_of.get(c).copied();
                        }
                    }
                }
            }
        }

        // attempts
        let mut ns_seen: HashSet<u64> = HashSet::new();
        for (ai, a) in t.attempts.iter().enumerate() {
            let ap = format!("{tp}.attempts[{ai}]");
            let aid = a.id.as_deref().unwrap_or("?");
            if !a.id.as_deref().is_some_and(crate::contract::valid_identity) {
                err!("E131", &ap, "attempt in task {tid} needs a nonblank terminal-safe id");
            }
            match a.n {
                None | Some(0) => err!("E131", &ap, "attempt {aid} missing n (1-based attempt number)"),
                Some(n) => {
                    if !ns_seen.insert(n) {
                        err!("E131", &ap, "attempt {aid}: duplicate attempt number n={n} within task {tid}");
                    }
                }
            }
            let astate = a.state.as_deref().unwrap_or("");
            if astate.is_empty() {
                err!("E131", format!("{ap}.state"), "attempt {aid} missing state");
            } else if !ATTEMPT_STATES.contains(&astate) {
                err!("E132", format!("{ap}.state"), "attempt {aid} has unknown state {:?} (want one of {})", astate, ATTEMPT_STATES.join("|"));
            }

            // cause
            match &a.cause {
                None => {
                    if a.n.unwrap_or(1) > 1 {
                        warn!("W206", &ap, "attempt {aid} (n>1) has no cause — why does it exist?");
                    }
                }
                Some(c) => {
                    match c.cause_type.as_deref() {
                        None => err!("E133", format!("{ap}.cause"), "attempt {aid} cause missing type"),
                        Some(ct) if !CAUSE_TYPES.contains(&ct) => {
                            err!("E133", format!("{ap}.cause"), "attempt {aid} has unknown cause type {:?} (want one of {})", ct, CAUSE_TYPES.join("|"))
                        }
                        _ => {}
                    }
                    if let Some(r) = c.reference.as_deref() {
                        if !attempt_ids.contains_key(r) && !task_ids.contains(r) {
                            err!("E134", format!("{ap}.cause.ref"), "attempt {aid} cause references unknown attempt/task {:?}", r);
                        }
                    }
                }
            }

            ts!(&a.started_at, format!("{ap}.started_at"));
            ts!(&a.ended_at, format!("{ap}.ended_at"));
            if let (Some(s), Some(e)) = (
                a.started_at.as_deref().and_then(crate::model::parse_min),
                a.ended_at.as_deref().and_then(crate::model::parse_min),
            ) {
                if e < s {
                    err!("E181", &ap, "attempt {aid} ends before it starts");
                }
            }

            let terminal = TERMINAL_STATES.contains(&astate);
            match &a.outcome {
                None if terminal => {
                    err!("E140", &ap, "attempt {aid} is terminal ({astate}) but has no outcome");
                }
                Some(o) => {
                    let result = o.result.as_deref().unwrap_or("");
                    if terminal && result != astate {
                        err!("E141", format!("{ap}.outcome"), "attempt {aid}: outcome.result {:?} contradicts state {:?}", result, astate);
                    }
                    match o.evidence.as_deref() {
                        None => warn!("W201", format!("{ap}.outcome"), "attempt {aid} outcome has no evidence tier — will render as ! asserted"),
                        Some(e) if !EVIDENCE_TIERS.contains(&e) => {
                            err!("E142", format!("{ap}.outcome"), "attempt {aid} has unknown evidence tier {:?} (want one of {})", e, EVIDENCE_TIERS.join("|"))
                        }
                        _ => {}
                    }
                }
                None => {}
            }
            if terminal && (a.started_at.is_none() || a.ended_at.is_none()) {
                warn!("W203", &ap, "settled attempt {aid} missing started_at/ended_at — durations become guesswork");
            }
            if astate == "working" {
                if a.locator.is_none() {
                    warn!("W204", &ap, "working attempt {aid} has no locator — [enter] focus cannot work");
                }
                let liveness_populated = a.liveness.as_ref().is_some_and(|l| {
                    l.prompt_acknowledged.is_some()
                        || l.last_output_at.is_some()
                        || l.queued_input.is_some()
                });
                if !liveness_populated {
                    warn!("W208", &ap, "working attempt {aid} has no liveness facts — a silent stall will look like work");
                }
                if let Some(l) = &a.liveness {
                    ts!(&l.last_output_at, format!("{ap}.liveness.last_output_at"));
                }
            }
        }

        // state vs attempt record (E150) — the CONTRACT projection table
        if !state.is_empty() && TASK_STATES.contains(&state) {
            let last = t.attempts.iter().max_by_key(|a| a.n.unwrap_or(0));
            let last_state = last.and_then(|a| a.state.as_deref());
            match state {
                "done" | "rejected" | "settled_unverified" => {
                    match last {
                        None => err!("E150", &tp, "task {tid} is {state} but has no attempts — nothing settled it"),
                        Some(a) => {
                            if last_state != Some(state) {
                                err!(
                                    "E150", &tp,
                                    "task {tid} is {state} but its latest attempt {} is {:?}",
                                    a.id.as_deref().unwrap_or("?"), last_state.unwrap_or("?")
                                );
                            }
                        }
                    }
                }
                "failed" => {
                    // a lost latest attempt fails the task too (dead pane)
                    match last {
                        None => err!("E150", &tp, "task {tid} is failed but has no attempts — nothing failed it"),
                        Some(a) => {
                            if !matches!(last_state, Some("failed") | Some("lost")) {
                                err!(
                                    "E150", &tp,
                                    "task {tid} is failed but its latest attempt {} is {:?}",
                                    a.id.as_deref().unwrap_or("?"), last_state.unwrap_or("?")
                                );
                            }
                        }
                    }
                }
                "working" => {
                    if !t.attempts.iter().any(|a| a.state.as_deref() == Some("working")) {
                        err!("E150", &tp, "task {tid} is working but no attempt is working");
                    }
                }
                "queued" => {
                    if t.attempts.iter().any(|a| a.state.as_deref() == Some("working")) {
                        err!("E150", &tp, "task {tid} is queued but has a working attempt");
                    }
                    if matches!(last_state, Some("done") | Some("settled_unverified")) {
                        err!("E150", &tp, "task {tid} is queued but its latest attempt already settled ({}) — a retry opens a new attempt, it never moves a task backward", last_state.unwrap_or("?"));
                    }
                }
                "review" => {
                    if t.attempts.is_empty() {
                        warn!("W210", &tp, "task {tid} is in review with no attempts — reviewing what?");
                    }
                }
                _ => {} // blocked/canceled: task-level facts, no attempt constraint
            }
            if matches!(state, "done" | "failed" | "rejected" | "settled_unverified") {
                for a in &t.attempts {
                    if a.state.as_deref() == Some("working") {
                        warn!(
                            "W209", &tp,
                            "task {tid} is settled ({state}) but attempt {} is still working — stale attempt left unfenced?",
                            a.id.as_deref().unwrap_or("?")
                        );
                    }
                }
            }
        }
    }

    // ── events ───────────────────────────────────────────────────────
    let mut prev_at: Option<&str> = None;
    let mut disordered = false;
    for (ei, e) in doc.events.iter().enumerate() {
        let ep = format!("events[{ei}]");
        match e.at.as_deref() {
            None => err!("E170", &ep, "event missing \"at\" timestamp"),
            Some(at) => {
                if !is_iso(at) {
                    err!("E180", format!("{ep}.at"), "unparseable timestamp {:?}", at);
                } else {
                    if let Some(p) = prev_at {
                        if at < p {
                            disordered = true;
                        }
                    }
                    prev_at = Some(at);
                }
            }
        }
        match e.event_type.as_deref() {
            None => err!("E170", &ep, "event missing type"),
            Some(et) if !EVENT_TYPES.contains(&et) => {
                err!("E170", &ep, "unknown event type {:?} (want one of {})", et, EVENT_TYPES.join("|"))
            }
            _ => {}
        }
        if let Some(tref) = e.task.as_deref() {
            if !task_ids.contains(tref) {
                err!("E171", &ep, "event references unknown task {:?}", tref);
            }
        }
        if let Some(aref) = e.attempt.as_deref() {
            if !attempt_ids.contains_key(aref) {
                err!("E171", &ep, "event references unknown attempt {:?}", aref);
            }
        }
        // per-type required fields (E172): an event that names no subject
        // is a claim about nothing
        match e.event_type.as_deref() {
            Some("attempt_started") | Some("attempt_settled") => {
                if e.attempt.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "{} event names no attempt", e.event_type.as_deref().unwrap_or(""));
                }
            }
            Some("promoted") => {
                if e.task.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "promoted event names no task");
                }
            }
            Some("directive") => {
                match e.verb.as_deref() {
                    None | Some("") => err!("E172", &ep, "directive event missing verb"),
                    Some(v) if !DIRECTIVE_VERBS.contains(&v) => {
                        err!("E172", &ep, "directive has unknown verb {:?} (want one of {})", v, DIRECTIVE_VERBS.join("|"))
                    }
                    _ => {}
                }
                if e.by.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "directive event names no \"by\" — authority must be attributable");
                }
            }
            Some("message_resolved") => {
                if e.task.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "message_resolved event names no task");
                }
                if e.message_id.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "message_resolved event names no message_id");
                }
                if e.detail.as_deref().unwrap_or("").is_empty() {
                    err!("E172", &ep, "message_resolved event carries no resolution detail");
                }
            }
            _ => {}
        }
        if e.message_id.as_deref() == Some("")
            || e.source_messages.iter().any(|id| id.is_empty())
        {
            err!("E172", &ep, "message correlations must be nonempty ids");
        }
    }
    if disordered {
        warn!("W207", "events", "events are not in ascending time order — the log should append, not shuffle");
    }

    Report {
        findings: f,
        tasks: tasks.len(),
        attempts: tasks.iter().map(|t| t.attempts.len()).sum(),
        events: doc.events.len(),
    }
}
