//! Flow analytics as queries over per-attempt timestamps.
//! Everything here derives from contract data and the document's own
//! clock (`generated_at`) — never the wall clock, so a snapshot of a
//! finished run reports the same numbers forever. Both baseline studies
//! hand-rolled exactly these numbers out of scrollback; the contract
//! makes them queries.

use crate::contract::{Doc, Task};
use crate::model::parse_min;
use std::process::ExitCode;

pub struct TaskFlow {
    pub id: String,
    pub state: String,
    pub attempts: usize,
    /// minutes since the first attempt started
    pub age_min: Option<i64>,
    /// LIVE tasks only: minutes since the latest attempt started. This is
    /// NOT "time in current state" — the contract records no state-
    /// transition timestamps, so that number does not exist here.
    pub since_latest_start_min: Option<i64>,
    /// sum of terminal-attempt durations (started→ended of attempts that
    /// actually settled; a live attempt contributes nothing)
    pub worked_min: Option<i64>,
}

pub struct Flow {
    pub now_min: Option<i64>,
    /// which timestamp backs `now_min`: "generated_at",
    /// "latest_timestamp" (fallback scan), or absent
    pub clock: Option<&'static str>,
    pub tasks: Vec<TaskFlow>,
    pub wip: usize,
    pub blocked: usize,
    pub review: usize,
    pub queued: usize,
    pub settled: usize,
    /// tasks with ≥1 attempt caused by sent_back/gate_failed — rework in
    /// the "bounced and redone" sense; followup/superseded retries are
    /// deliberate iteration, not rework — over tasks attempted
    pub rework_num: usize,
    pub rework_den: usize,
    /// mean terminal-attempt duration, minutes
    pub avg_attempt_min: Option<i64>,
    /// longest dependency chain of unfinished tasks in which every
    /// consecutive pair is a real edge (`deps` ∪ gate `inputs`); settled
    /// tasks are boundaries, never traversed through
    pub critical_path: Vec<String>,
    /// naive ETA: |critical_path| × avg_attempt_min
    pub eta_min: Option<i64>,
}

fn is_settled(state: &str) -> bool {
    matches!(state, "done" | "failed" | "rejected" | "settled_unverified")
}

/// Attempt states whose started→ended span is a real work duration.
/// `lost` is terminal for scheduling but NOT a settlement (CONTRACT:
/// "the runtime vanished") — its span produced nothing and must not
/// inflate the mean or the ETA built on it.
fn attempt_is_settled(state: Option<&str>) -> bool {
    matches!(
        state,
        Some("done") | Some("failed") | Some("rejected") | Some("settled_unverified")
    )
}

const REWORK_CAUSES: &[&str] = &["sent_back", "gate_failed"];

fn attempt_span(t: &Task) -> (Option<i64>, Option<i64>) {
    let mut first: Option<i64> = None;
    let mut last_start: Option<i64> = None;
    for a in &t.attempts {
        if let Some(s) = a.started_at.as_deref().and_then(parse_min) {
            first = Some(first.map_or(s, |f: i64| f.min(s)));
            last_start = Some(last_start.map_or(s, |l: i64| l.max(s)));
        }
    }
    (first, last_start)
}

pub fn compute(doc: &Doc) -> Flow {
    let tasks: &[Task] = doc.tasks.as_deref().unwrap_or(&[]);
    // the document's clock; a doc without one falls back to the latest
    // timestamp it contains anywhere (still its own clock, reproducible)
    let (now_min, clock) = match doc.generated_at.as_deref().and_then(parse_min) {
        Some(n) => (Some(n), Some("generated_at")),
        None => {
            let latest = tasks
                .iter()
                .flat_map(|t| t.attempts.iter())
                .flat_map(|a| {
                    [
                        a.started_at.as_deref(),
                        a.ended_at.as_deref(),
                        a.liveness.as_ref().and_then(|l| l.last_output_at.as_deref()),
                    ]
                })
                .chain(doc.run.as_ref().map(|r| r.started_at.as_deref()))
                .chain(doc.events.iter().map(|e| e.at.as_deref()))
                .flatten()
                .filter_map(parse_min)
                .max();
            (latest, latest.map(|_| "latest_timestamp"))
        }
    };

    let mut out = Vec::new();
    let (mut wip, mut blocked, mut review, mut queued, mut settled) = (0, 0, 0, 0, 0);
    let (mut rework_num, mut rework_den) = (0, 0);
    let mut durs: Vec<i64> = Vec::new();

    for t in tasks {
        let state = t.state.as_deref().unwrap_or("queued").to_string();
        match state.as_str() {
            "working" => wip += 1,
            "blocked" => blocked += 1,
            "review" => review += 1,
            "queued" => queued += 1,
            s if is_settled(s) => settled += 1,
            _ => {}
        }
        if !t.attempts.is_empty() {
            rework_den += 1;
            let reworked = t.attempts.iter().any(|a| {
                a.cause
                    .as_ref()
                    .and_then(|c| c.cause_type.as_deref())
                    .is_some_and(|ct| REWORK_CAUSES.contains(&ct))
            });
            if reworked {
                rework_num += 1;
            }
        }
        let mut worked = 0i64;
        let mut any_worked = false;
        for a in &t.attempts {
            // only attempts that actually settled: a live attempt's
            // started→now span is not a duration, and counting it would
            // shrink the mean every time someone runs stats mid-flight
            if !attempt_is_settled(a.state.as_deref()) {
                continue;
            }
            if let (Some(s), Some(e)) = (
                a.started_at.as_deref().and_then(parse_min),
                a.ended_at.as_deref().and_then(parse_min),
            ) {
                if e >= s {
                    worked += e - s;
                    any_worked = true;
                    durs.push(e - s);
                }
            }
        }
        let (first, last_start) = attempt_span(t);
        let age_min = match (now_min, first) {
            (Some(n), Some(f)) if n >= f => Some(n - f),
            _ => None,
        };
        let since_latest_start_min = if is_settled(&state) {
            None
        } else {
            match (now_min, last_start) {
                (Some(n), Some(l)) if n >= l => Some(n - l),
                _ => None,
            }
        };
        out.push(TaskFlow {
            id: t.id.clone().unwrap_or_default(),
            state,
            attempts: t.attempts.len(),
            age_min,
            since_latest_start_min,
            worked_min: if any_worked { Some(worked) } else { None },
        });
    }

    let avg_attempt_min = if durs.is_empty() {
        None
    } else {
        Some(durs.iter().sum::<i64>() / durs.len() as i64)
    };

    // critical path: longest chain through unfinished tasks (deps edges).
    // The run is a DAG when clean (E122); a cycle in dirty input is broken
    // by the visiting set so this stays total.
    let idx: std::collections::HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.id.as_deref().map(|id| (id, i)))
        .collect();
    let unfinished =
        |i: usize| !is_settled(tasks[i].state.as_deref().unwrap_or("queued"));
    let mut memo: Vec<Option<Vec<usize>>> = vec![None; tasks.len()];
    fn chain(
        i: usize,
        tasks: &[Task],
        idx: &std::collections::HashMap<&str, usize>,
        unfinished: &dyn Fn(usize) -> bool,
        memo: &mut Vec<Option<Vec<usize>>>,
        visiting: &mut Vec<usize>,
    ) -> Vec<usize> {
        if let Some(m) = &memo[i] {
            return m.clone();
        }
        if visiting.contains(&i) {
            return Vec::new();
        }
        visiting.push(i);
        let mut best: Vec<usize> = Vec::new();
        // edges are `deps` ∪ gate `inputs` — the same edge set the E122
        // cycle check walks; a gate that declares its fan-in in `inputs`
        // must not truncate the path
        let edges = tasks[i].deps.iter().chain(tasks[i].inputs.iter().flatten());
        for d in edges {
            if let Some(&j) = idx.get(d.as_str()) {
                // a settled dep is a boundary: it gates nothing anymore,
                // so a chain that "passes through" it is two chains
                if !unfinished(j) {
                    continue;
                }
                let c = chain(j, tasks, idx, unfinished, memo, visiting);
                if c.len() > best.len() {
                    best = c;
                }
            }
        }
        visiting.pop();
        let mut path = best;
        if unfinished(i) {
            path.push(i);
        } else {
            path = Vec::new();
        }
        memo[i] = Some(path.clone());
        path
    }
    let mut critical: Vec<usize> = Vec::new();
    for i in 0..tasks.len() {
        let mut visiting = Vec::new();
        let c = chain(i, tasks, &idx, &unfinished, &mut memo, &mut visiting);
        if c.len() > critical.len() {
            critical = c;
        }
    }
    let critical_path: Vec<String> = critical
        .iter()
        .map(|&i| tasks[i].id.clone().unwrap_or_default())
        .collect();
    let eta_min = match (avg_attempt_min, critical_path.len()) {
        (_, 0) => Some(0),
        (Some(avg), n) => Some(avg * n as i64),
        (None, _) => None,
    };

    Flow {
        now_min,
        clock,
        tasks: out,
        wip,
        blocked,
        review,
        queued,
        settled,
        rework_num,
        rework_den,
        avg_attempt_min,
        critical_path,
        eta_min,
    }
}

/// One-line flow summary for the pane header.
pub fn header_chip(doc: &Doc) -> Option<String> {
    let f = compute(doc);
    if f.tasks.is_empty() {
        return None;
    }
    let mut s = format!("wip {} · blk {}", f.wip, f.blocked);
    if f.rework_den > 0 {
        s.push_str(&format!(" · rework {}%", 100 * f.rework_num / f.rework_den));
    }
    if let (Some(eta), false) = (f.eta_min, f.critical_path.is_empty()) {
        s.push_str(&format!(" · eta ~{eta}m"));
    }
    Some(s)
}

fn fmt_min(v: Option<i64>) -> String {
    match v {
        Some(m) => format!("{m}m"),
        None => "—".into(),
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut json_out = false;
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            other if !other.starts_with('-') && path.is_none() => path = Some(other),
            other => {
                eprintln!("dagr stats: unexpected argument {other:?}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("dagr stats: usage: dagr stats <run.json> [--json]");
        return ExitCode::from(2);
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("dagr stats: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let doc: Doc = match serde_json::from_str(&raw) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dagr stats: not a contract document: {e}");
            return ExitCode::from(1);
        }
    };
    let f = compute(&doc);

    if json_out {
        let tasks: Vec<serde_json::Value> = f
            .tasks
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id, "state": t.state, "attempts": t.attempts,
                    "age_min": t.age_min,
                    "since_latest_start_min": t.since_latest_start_min,
                    "worked_min": t.worked_min,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "now_min": f.now_min,
                "clock": f.clock,
                "wip": f.wip, "blocked": f.blocked, "review": f.review,
                "queued": f.queued, "settled": f.settled,
                "rework": {"reworked": f.rework_num, "attempted": f.rework_den},
                "avg_attempt_min": f.avg_attempt_min,
                "critical_path": f.critical_path,
                "eta_min": f.eta_min,
                "eta_note": "naive: unfinished critical-path length × mean settled-attempt duration",
                "tasks": tasks,
            }))
            .expect("stats serialize")
        );
    } else {
        if let Some(c) = f.clock {
            println!("clock: {c}");
            println!();
        }
        println!(
            "{:<14} {:<20} {:>3}  {:>7} {:>11} {:>8}",
            "task", "state", "att", "age", "since-start", "worked"
        );
        for t in &f.tasks {
            println!(
                "{:<14} {:<20} {:>3}  {:>7} {:>11} {:>8}",
                t.id,
                t.state,
                t.attempts,
                fmt_min(t.age_min),
                fmt_min(t.since_latest_start_min),
                fmt_min(t.worked_min)
            );
        }
        println!();
        println!(
            "wip {} · blocked {} · review {} · queued {} · settled {}",
            f.wip, f.blocked, f.review, f.queued, f.settled
        );
        if f.rework_den > 0 {
            println!(
                "rework: {}/{} attempted tasks bounced back (sent_back/gate_failed)",
                f.rework_num, f.rework_den
            );
        }
        if let Some(avg) = f.avg_attempt_min {
            println!("mean settled attempt: {avg}m");
        }
        if f.critical_path.is_empty() {
            println!("critical path: (nothing unfinished)");
        } else {
            println!(
                "critical path ({}): {}{}",
                f.critical_path.len(),
                f.critical_path.join(" → "),
                match f.eta_min {
                    Some(e) => format!(" · eta ~{e}m (naive: length × mean attempt)"),
                    None => " · eta unknown (no settled attempts to average)".into(),
                }
            );
        }
    }
    ExitCode::SUCCESS
}
