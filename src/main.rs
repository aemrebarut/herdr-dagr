//! dagr — a live DAG of your agent swarm, as a herdr plugin.
//!
//! `dagr check` lints a run-state document against CONTRACT.md v1,
//! `dagr view` draws it, `dagr stats` reports flow analytics over it.

mod action;
mod check;
mod contract;
mod herdr;
mod model;
mod picker;
mod render;
mod select;
mod stats;
mod style;
mod view;

use std::process::ExitCode;

const USAGE: &str = "\
dagr — a live DAG of your agent swarm (herdr plugin)

USAGE:
  dagr check <run.json> [--json] [--strict]
      lint a run-state document against the contract
  dagr view [run.json] [--snapshot] [--width N] [--select ID]
      draw the run: interactive pane (j/k · tab · enter · u/a/o/x · ? · q),
      watches the file for changes; --snapshot prints one frame to stdout.
      Without a path: $DAGR_RUN, then .dagr/run.json / run.json under the
      herdr context cwd (waits for the file if none exists yet)
      Set DAGR_WORKING_GLYPH=* for the ASCII working-state fallback.
  dagr stats <run.json> [--json]
      flow analytics over per-attempt timestamps: age, time-in-state,
      rework rate, WIP, critical path + naive ETA
  dagr pane-cwd
      plumbing for scripts/open-dagr.sh: resolve the user's cwd from
      $HERDR_PLUGIN_CONTEXT_JSON (prints nothing if unavailable)
  dagr --skill
      print the producer skill — the agent-facing instructions for
      writing and maintaining a run file. Bundled at build time, so the
      printed copy always matches this binary; the companion examples
      live beside it in skills/dagr-producer/examples/ under the plugin
      root ($HERDR_PLUGIN_ROOT inside a herdr pane)

check exit codes: 0 clean (warnings allowed unless --strict), 1 findings, 2 usage/IO
";

/// Bundled at build time so the printed skill always matches this binary's
/// release — and so a tree that shipped the binary without the skill fails
/// to compile rather than shipping an agent-facing hole.
const SKILL: &str = include_str!("../skills/dagr-producer/SKILL.md");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => cmd_check(&args[1..]),
        Some("view") => cmd_view(&args[1..]),
        Some("stats") => stats::run(&args[1..]),
        Some("pane-cwd") => cmd_pane_cwd(),
        Some("--skill") => {
            print!("{SKILL}");
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("dagr {} (contract v1)", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Resolve the *user's* cwd from the action context, for the launcher.
/// Key order differs from `discover_run_file` on purpose: at action time
/// the focused pane is the user's own pane (focus has not moved yet), so
/// `focused_pane_cwd` is the most specific answer; at pane-launch time it
/// may already name the dagr pane itself, which is why the viewer's own
/// fallback prefers `workspace_cwd`. Prints nothing (exit 0) when the
/// context is absent or unparseable — the launcher treats that as "no
/// --cwd".
fn cmd_pane_cwd() -> ExitCode {
    if let Some(cwd) = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| {
            ["focused_pane_cwd", "workspace_cwd", "cwd"].iter().find_map(|k| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
        })
    {
        println!("{cwd}");
    }
    ExitCode::SUCCESS
}

fn cmd_view(args: &[String]) -> ExitCode {
    let mut path: Option<String> = None;
    let mut snapshot = false;
    let mut width: Option<usize> = None;
    let mut select: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--snapshot" => snapshot = true,
            "--width" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => width = Some(v),
                None => {
                    eprintln!("dagr view: --width needs a number");
                    return ExitCode::from(2);
                }
            },
            "--select" => select = it.next().cloned(),
            other if !other.starts_with('-') && path.is_none() => path = Some(other.into()),
            other => {
                eprintln!("dagr view: unexpected argument {other:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let path = path.unwrap_or_else(discover_run_file);
    view::run(view::ViewArgs { path, snapshot, width, select })
}

/// Where is the run file? Explicit beats implicit: `$DAGR_RUN`, then
/// `.dagr/run.json` / `run.json` under the herdr-provided context cwd
/// (`HERDR_PLUGIN_CONTEXT_JSON`: the pane process itself starts in the
/// plugin's install dir, not where the user is). If nothing exists yet the
/// default target is returned anyway — the viewer waits for it to appear.
fn discover_run_file() -> String {
    // workspace_cwd first: focused_pane_cwd is computed at pane-launch
    // time and may already name the (just-focused) dagr pane itself,
    // whose cwd is the plugin install dir. The launcher resolves the
    // user's cwd deterministically and passes it via --cwd; this env
    // path is the fallback.
    let ctx_cwd = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| {
            ["workspace_cwd", "focused_pane_cwd", "cwd"].iter().find_map(|k| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            })
        });
    if let Ok(p) = std::env::var("DAGR_RUN") {
        if !p.is_empty() {
            // A relative DAGR_RUN means "under the user's context", not
            // under the plugin install dir the pane process starts in.
            return match &ctx_cwd {
                Some(cwd) if !std::path::Path::new(&p).is_absolute() => {
                    format!("{cwd}/{p}")
                }
                _ => p,
            };
        }
    }
    // Absolute base always: inside herdr the process cwd is the plugin
    // install dir, and a relative fallback would silently search there.
    // An absolute path also makes the waiting banner name the directory
    // actually searched.
    let base = ctx_cwd.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|_| ".".into())
    });
    for cand in [".dagr/run.json", "run.json"] {
        let p = format!("{base}/{cand}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    format!("{base}/.dagr/run.json")
}

fn cmd_check(args: &[String]) -> ExitCode {
    let mut path: Option<&str> = None;
    let mut json_out = false;
    let mut strict = false;
    for a in args {
        match a.as_str() {
            "--json" => json_out = true,
            "--strict" => strict = true,
            other if !other.starts_with('-') && path.is_none() => path = Some(other),
            other => {
                eprintln!("dagr check: unexpected argument {other:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(path) = path else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };

    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("dagr check: cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let doc: contract::Doc = match serde_json::from_str(&raw) {
        Ok(d) => d,
        Err(e) => {
            // A type-level parse failure is itself a finding: report it in
            // both output modes with serde's line:column location.
            if json_out {
                println!(
                    "{}",
                    serde_json::json!([{
                        "level": "error", "code": "E001",
                        "path": format!("{}:{}:{}", path, e.line(), e.column()),
                        "msg": format!("not a contract document: {e}"),
                    }])
                );
            } else {
                println!("ERROR   E001 {path}:{}:{} — not a contract document: {e}", e.line(), e.column());
            }
            return ExitCode::from(1);
        }
    };

    let report = check::check(&doc);

    if json_out {
        println!("{}", serde_json::to_string_pretty(&report.findings).expect("findings serialize"));
    } else {
        for f in &report.findings {
            let level = match f.level {
                check::Level::Error => "ERROR  ",
                check::Level::Warning => "warning",
            };
            println!("{level} {} {} — {}", f.code, f.path, f.msg);
        }
        let (e, w) = (report.errors(), report.warnings());
        if e == 0 && w == 0 {
            println!(
                "✓ clean — {} tasks, {} attempts, {} events",
                report.tasks, report.attempts, report.events
            );
        } else {
            println!(
                "{} error{}, {} warning{} — {} tasks, {} attempts, {} events",
                e, if e == 1 { "" } else { "s" },
                w, if w == 1 { "" } else { "s" },
                report.tasks, report.attempts, report.events
            );
        }
    }

    let failed = report.errors() > 0 || (strict && report.warnings() > 0);
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
