//! `dagr view` — the pane. Interactive by default (crossterm raw mode,
//! alternate screen, watch-on-mtime); `--snapshot` prints one frame to
//! stdout for demos, CI capture, and eyeballing against the Python
//! reference. The renderer never crashes on bad input: contract errors
//! become a banner, and the last good scene stays up.

use crate::{check, contract::Doc, herdr, message, model, picker, render, select, stats, style};
use crossterm::{
    cursor, event, execute, queue,
    terminal::{self, ClearType},
};
use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

pub struct ViewArgs {
    pub path: String,
    pub snapshot: bool,
    pub width: Option<usize>,
    pub select: Option<String>,
}

struct Loaded {
    doc: Doc,
    banner: Option<String>,
    generated_min: Option<i64>,
    /// flow chip, computed once per (re)load — never per frame (M4 F17)
    chip: Option<String>,
    message_starters: Vec<message::Starter>,
    message_config_path: std::path::PathBuf,
    message_summaries: Vec<message::Summary>,
}

fn load(path: &str) -> Result<Loaded, String> {
    let raw = crate::scale::read_limited(path)?;
    let doc: Doc =
        serde_json::from_str(&raw).map_err(|e| format!("not a contract document: {e}"))?;
    crate::scale::enforce_document(&doc)?;
    let report = check::check(&doc);
    let mut banner = match report.errors() {
        0 => None,
        n => Some(format!(
            "{n} contract error{} — run `dagr check {path}`; drawing what parses",
            if n == 1 { "" } else { "s" }
        )),
    };
    let generated_min = doc.generated_at.as_deref().and_then(model::parse_min);
    let chip = stats::header_chip(&doc);
    let config = message::load_config(std::path::Path::new(path));
    if let Some(warning) = config.warning {
        banner = Some(match banner {
            Some(existing) => format!("{existing} · actions config: {warning}"),
            None => format!("actions config: {warning}"),
        });
    }
    let run_id = doc.run.as_ref().and_then(|run| run.id.as_deref());
    let message_summaries = match message::read_summaries(std::path::Path::new(path), run_id) {
        Ok(messages) => messages,
        Err(warning) => {
            banner = Some(match banner {
                Some(existing) => format!("{existing} · message journal: {warning}"),
                None => format!("message journal: {warning}"),
            });
            Vec::new()
        }
    };
    Ok(Loaded {
        doc,
        banner,
        generated_min,
        chip,
        message_starters: config.starters,
        message_config_path: config.path,
        message_summaries,
    })
}

/// The permissive contract types make `{}` a valid (empty) document.
fn empty_doc() -> Doc {
    serde_json::from_str("{}").expect("empty doc parses")
}

fn wall_min() -> Option<i64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| (d.as_secs() / 60) as i64)
}

fn stale_min(generated: Option<i64>) -> Option<i64> {
    match (wall_min(), generated) {
        (Some(w), Some(g)) if w > g => Some(w - g),
        _ => None,
    }
}

pub fn run(args: ViewArgs) -> ExitCode {
    if args.snapshot {
        return snapshot(&args);
    }
    match interactive(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dagr view: {e}");
            ExitCode::from(2)
        }
    }
}

fn snapshot(args: &ViewArgs) -> ExitCode {
    let loaded = match load(&args.path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dagr view: {e}");
            return ExitCode::from(1);
        }
    };
    // Below ~20 columns no grammar survives; clamp instead of panicking on
    // underflowed layout math (a resize can momentarily report tiny sizes).
    let w = args.width.unwrap_or(120).max(20);
    let selected = args.select.as_deref();
    let scene =
        model::build(&loaded.doc, selected, None, loaded.chip.as_deref(), &model::ViewOpts::default());
    let frame = render::compose(
        &render::FrameInput {
            doc: &loaded.doc,
            scene: &scene,
            selected,
            banner: loaded.banner,
            flash: None,
            stale_min: None, // snapshots are for capture; staleness is a live concern
            watching: false,
            herdr: None, // hints are a live concern too
            prompt: None,
            messages: &loaded.message_summaries,
        },
        w,
    );
    let mut out = std::io::stdout().lock();
    for line in frame.lines {
        let _ = writeln!(out, "{line}");
    }
    ExitCode::SUCCESS
}

// ── interactive ─────────────────────────────────────────────────────

/// The only user-facing action is an editable contextual message.
enum Mode {
    Normal,
    /// `f` — the run-file picker (its own frame, like help).
    Picker(picker::State),
    /// `/` — incremental row search; the query lives on the bottom line.
    Search { buf: String },
    /// One editable contextual message. Starters only prefill text;
    /// authority remains a separate field and Herdr owns delivery.
    Message { draft: message::Draft },
}

struct App {
    path: String,
    loaded: Loaded,
    selected: Option<String>,
    queue_pos: usize,
    flash: Option<(String, u8)>, // message, ticks-to-live
    help: bool,
    scroll: usize,
    herdr: Option<herdr::Link>,
    mode: Mode,
    /// In-flight focus result (focus runs off-thread; even a bounded CLI
    /// wait has no business freezing the render loop).
    bg_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// Clickable regions of the last drawn screen. The renderer emits hits
    /// in logical-frame coordinates; the vertical layout remaps them after
    /// it docks detail and footer regions.
    hits: Vec<render::Hit>,
    /// Rows available to pointer interaction (the terminal's reserved
    /// search row is excluded).
    view_rows: usize,
    /// Height of the scrollable graph region. Page navigation must use the
    /// master pane, not count the fixed detail dock and footer as rows.
    page_rows: usize,
    /// Explicit focus-plus-context drill-down. Ordinary selection never
    /// changes geometry; only `d`/Esc crosses this boundary.
    details_open: bool,
    /// Independent scroll state for the full detail body. The selected task
    /// is frozen while this is active, so graph position remains restorable.
    detail_scroll: usize,
    detail_scroll_max: usize,
    detail_page_rows: usize,
    /// The painted lines of the last draw, kept so a copy returns exactly
    /// what was on screen.
    frame: Vec<String>,
    /// Live mouse text selection, in frame-line coordinates.
    sel: Option<select::Sel>,
    /// Bytes of the last painted screen. An identical frame is skipped
    /// rather than repainted.
    painted: Vec<u8>,
    /// Zoom stack: the last entry is the row key the trace is re-rooted at.
    zoom: Vec<String>,
    /// Row keys folded shut (← on a branch, `z` for settled ones).
    folded: std::collections::HashSet<String>,
    /// Last committed `/` query (n/N cycle it) and its match count.
    search: Option<String>,
    search_matches: usize,
    /// Previous plain click, for double-click zoom: when, and which
    /// frame line it landed on.
    last_click: Option<(std::time::Instant, usize)>,
}

impl App {
    fn hints(&self) -> Option<herdr::Hints> {
        self.herdr.as_ref().map(|l| l.hints())
    }

    fn view_opts(&self) -> model::ViewOpts<'_> {
        model::ViewOpts {
            zoom: self.zoom.last().map(String::as_str),
            folded: Some(&self.folded),
        }
    }

    fn scene(&self) -> model::Scene {
        let h = self.hints();
        model::build(
            &self.loaded.doc,
            self.selected.as_deref(),
            h.as_ref(),
            self.loaded.chip.as_deref(),
            &self.view_opts(),
        )
    }

    /// Tell the link which locator panes matter right now (live attempts).
    fn update_watch(&self) {
        let Some(link) = &self.herdr else { return };
        let mut panes = Vec::new();
        for t in self.loaded.doc.tasks.as_deref().unwrap_or(&[]) {
            for a in &t.attempts {
                // "blocked" is a TASK state, not an attempt state; "lost"
                // is exactly the attempt whose pane deserves a liveness
                // check ("is it still there?").
                if matches!(a.state.as_deref(), Some("working") | Some("queued") | Some("lost")) {
                    if let Some(p) = a.locator.as_ref().and_then(|l| l.pane.clone()) {
                        panes.push(p);
                    }
                }
            }
        }
        link.set_watch(panes);
    }

    fn selectable_keys(&self) -> Vec<String> {
        self.scene()
            .rows
            .iter()
            .filter(|r| r.selectable)
            .map(|r| r.key.clone())
            .collect()
    }

    fn move_sel(&mut self, delta: i64) {
        let keys = self.selectable_keys();
        if keys.is_empty() {
            return;
        }
        let cur = self
            .selected
            .as_deref()
            .and_then(|s| keys.iter().position(|k| k == s));
        let next = match cur {
            None => 0,
            Some(i) => (i as i64 + delta).clamp(0, keys.len() as i64 - 1) as usize,
        };
        self.selected = Some(keys[next].clone());
    }

    fn toggle_details(&mut self) {
        if self.details_open {
            self.details_open = false;
            self.detail_scroll = 0;
            self.detail_scroll_max = 0;
            return;
        }
        if self.selected.is_none() {
            self.flash = Some(("select a row first".into(), 8));
            return;
        }
        self.details_open = true;
        self.detail_scroll = 0;
        self.detail_scroll_max = 0;
    }

    fn move_detail(&mut self, delta: i64) {
        self.detail_scroll = (self.detail_scroll as i64 + delta)
            .clamp(0, self.detail_scroll_max as i64) as usize;
    }

    fn cycle_queue(&mut self) {
        let scene = self.scene();
        if scene.queue.is_empty() {
            self.flash = Some(("attention queue is empty".into(), 8));
            return;
        }
        self.queue_pos %= scene.queue.len();
        let tid = scene.queue[self.queue_pos].task_id.clone();
        self.jump_to_task(&tid);
        self.queue_pos += 1;
    }

    /// Select a task's latest real row, unfolding (and if needed unzooming)
    /// whatever hides it — a jump means "take me there", never a silent miss.
    fn jump_to_task(&mut self, tid: &str) {
        let key = self
            .loaded
            .doc
            .tasks
            .as_deref()
            .and_then(|ts| ts.iter().find(|t| t.id.as_deref() == Some(tid)))
            .and_then(|t| {
                t.attempts
                    .iter()
                    .max_by_key(|a| a.n.unwrap_or(0))
                    .and_then(|a| a.id.clone())
                    .or_else(|| t.id.clone())
            })
            .unwrap_or_else(|| tid.to_string());
        self.reveal(&key);
        self.selected = Some(key);
    }

    /// Open the way to a row: unfold every folded ancestor, and drop the
    /// zoom entirely if the row lives outside it.
    fn reveal(&mut self, key: &str) {
        let anc = model::ancestors(&self.loaded.doc, key);
        if let Some(root) = self.zoom.last() {
            if root != key && !anc.iter().any(|a| a == root) {
                self.zoom.clear();
            }
        }
        for a in &anc {
            self.folded.remove(a);
        }
    }

    fn row_state(&self, key: &str) -> Option<(bool, bool, Option<String>)> {
        self.scene()
            .rows
            .iter()
            .find(|r| r.key == key)
            .map(|r| (r.fold.is_some(), r.has_kids, r.parent.clone()))
    }

    /// → on a row: a folded row opens; an open branch zooms; a leaf stays.
    fn open_or_zoom(&mut self, key: String) {
        let Some((folded, has_kids, _)) = self.row_state(&key) else { return };
        if folded {
            self.folded.remove(&key);
        } else if has_kids {
            if self.zoom.last() != Some(&key) {
                self.zoom.push(key);
                self.flash = Some(("zoomed — esc/← backs out".into(), 10));
            }
        } else {
            self.flash = Some(("leaf row — nothing to zoom into".into(), 8));
        }
    }

    /// ← on a row: the zoom root un-zooms; an open branch folds; a folded
    /// or leaf row jumps to its parent.
    fn fold_or_up(&mut self) {
        let Some(key) = self.selected.clone() else { return };
        if self.zoom.last() == Some(&key) {
            self.zoom.pop();
            self.flash = Some(("zoomed out".into(), 6));
            return;
        }
        let Some((folded, has_kids, parent)) = self.row_state(&key) else { return };
        if !folded && has_kids {
            self.folded.insert(key);
            return;
        }
        if let Some(p) = parent {
            self.selected = Some(p);
        }
    }

    /// `z`: fold every fully-settled branch (or unfold them all again).
    fn toggle_settled(&mut self) {
        let roots = model::settled_roots(&self.loaded.doc);
        if roots.is_empty() {
            self.flash = Some(("no settled branches to fold".into(), 8));
            return;
        }
        if roots.iter().all(|k| self.folded.contains(k)) {
            for k in &roots {
                self.folded.remove(k);
            }
            self.flash = Some(("settled branches unfolded".into(), 8));
        } else {
            let n = roots.len();
            self.folded.extend(roots);
            self.snap_selection();
            self.flash = Some((
                format!("folded {n} settled branch{}", if n == 1 { "" } else { "es" }),
                10,
            ));
        }
    }

    /// A fold or reload can hide the selected row: land on its nearest
    /// visible ancestor instead of pointing at nothing.
    fn snap_selection(&mut self) {
        let Some(sel) = self.selected.clone() else { return };
        let keys = self.selectable_keys();
        if keys.contains(&sel) {
            return;
        }
        for anc in model::ancestors(&self.loaded.doc, &sel) {
            if keys.contains(&anc) {
                self.selected = Some(anc);
                return;
            }
        }
        self.selected = keys.first().cloned();
    }

    /// Recompute matches for the live query; land on the first one unless
    /// the cursor already sits on a match (incremental `/` typing).
    fn search_apply(&mut self) {
        let Some(q) = self.search.clone() else {
            self.search_matches = 0;
            return;
        };
        let scene = self.scene();
        let matches: Vec<String> = scene
            .rows
            .iter()
            .filter(|r| r.selectable && model::row_matches(r, &q))
            .map(|r| r.key.clone())
            .collect();
        self.search_matches = matches.len();
        if matches.is_empty()
            || self.selected.as_deref().is_some_and(|s| matches.iter().any(|m| m == s))
        {
            return;
        }
        self.selected = Some(matches[0].clone());
    }

    /// n/N — cycle matches of the last `/` query, wrapping.
    fn search_jump(&mut self, dir: i64) {
        let Some(q) = self.search.clone() else {
            self.flash = Some(("no search — press / first".into(), 8));
            return;
        };
        let scene = self.scene();
        let idxs: Vec<usize> = scene
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.selectable && model::row_matches(r, &q))
            .map(|(i, _)| i)
            .collect();
        self.search_matches = idxs.len();
        if idxs.is_empty() {
            self.flash = Some((format!("no match for /{q}"), 8));
            return;
        }
        let cur = self
            .selected
            .as_deref()
            .and_then(|s| scene.rows.iter().position(|r| r.key == s));
        let next = match (dir > 0, cur) {
            (true, Some(c)) => idxs.iter().copied().find(|&i| i > c).unwrap_or(idxs[0]),
            (true, None) => idxs[0],
            (false, Some(c)) => {
                idxs.iter().copied().rev().find(|&i| i < c).unwrap_or(*idxs.last().unwrap())
            }
            (false, None) => *idxs.last().unwrap(),
        };
        let nth = idxs.iter().position(|&i| i == next).unwrap_or(0) + 1;
        self.selected = Some(scene.rows[next].key.clone());
        self.flash = Some((format!("match {nth}/{}", idxs.len()), 8));
    }

    /// `f` — the run-file picker, rooted at the current file's directory.
    fn enter_picker(&mut self) {
        let base = std::path::Path::new(&self.path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let base = base.canonicalize().unwrap_or(base);
        self.mode = Mode::Picker(picker::State::open(base));
    }

    fn poll_picker(&mut self) {
        if let Mode::Picker(st) = &mut self.mode {
            st.poll();
        }
    }

    /// Point the pane at another run file (the `f` picker chose it).
    fn open_file(&mut self, path: std::path::PathBuf) {
        picker::mru_add(&path);
        self.path = path.to_string_lossy().to_string();
        self.zoom.clear();
        self.folded.clear();
        self.search = None;
        self.search_matches = 0;
        self.selected = None;
        self.details_open = false;
        self.detail_scroll = 0;
        self.detail_scroll_max = 0;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.clone());
        self.flash = Some((format!("watching {name}"), 10));
        self.reload(); // a load error overwrites the flash with the reason
        self.auto_select();
    }

    /// Start on the most urgent thing, like a human would.
    fn auto_select(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let scene = self.scene();
        self.selected = scene
            .queue
            .first()
            .map(|q| q.task_id.clone())
            .and_then(|tid| {
                scene
                    .rows
                    .iter()
                    .rev()
                    .find(|r| r.task_id == tid && !r.dotted)
                    .map(|r| r.key.clone())
            })
            .or_else(|| scene.rows.iter().find(|r| r.selectable).map(|r| r.key.clone()));
    }

    /// One key in the `f` picker: typing filters, arrows move.
    fn picker_key(&mut self, mut st: picker::State, k: event::KeyEvent) {
        use event::KeyCode::*;
        let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);
        match k.code {
            Esc => {}
            Char('c') if ctrl => {}
            Enter => match st.current().map(|it| it.path.clone()) {
                Some(p) => self.open_file(p),
                None => self.mode = Mode::Picker(st),
            },
            Up => {
                st.step(-1);
                self.mode = Mode::Picker(st);
            }
            Down => {
                st.step(1);
                self.mode = Mode::Picker(st);
            }
            Char('p' | 'k') if ctrl => {
                st.step(-1);
                self.mode = Mode::Picker(st);
            }
            Char('n' | 'j') if ctrl => {
                st.step(1);
                self.mode = Mode::Picker(st);
            }
            Backspace => {
                st.query.pop();
                st.pos = 0;
                self.mode = Mode::Picker(st);
            }
            Char(c) if !ctrl && !k.modifiers.contains(event::KeyModifiers::ALT) => {
                st.query.push(c);
                st.pos = 0;
                self.mode = Mode::Picker(st);
            }
            _ => self.mode = Mode::Picker(st),
        }
    }

    /// One key in `/` search: typing edits the query and jumps live.
    fn search_key(&mut self, mut buf: String, k: event::KeyEvent) {
        use event::KeyCode::*;
        let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);
        match k.code {
            Esc => {
                self.search = None;
                self.search_matches = 0;
            }
            Char('c') if ctrl => {
                self.search = None;
                self.search_matches = 0;
            }
            Enter => {
                if buf.is_empty() {
                    self.search = None;
                    self.search_matches = 0;
                } else {
                    let n = self.search_matches;
                    self.flash = Some((
                        format!("{n} match{} · n/N cycles", if n == 1 { "" } else { "es" }),
                        10,
                    ));
                }
            }
            Backspace => {
                buf.pop();
                self.search = (!buf.is_empty()).then(|| buf.clone());
                self.search_apply();
                self.mode = Mode::Search { buf };
            }
            Char(c) if !ctrl && !k.modifiers.contains(event::KeyModifiers::ALT) => {
                buf.push(c);
                self.search = Some(buf.clone());
                self.search_apply();
                self.mode = Mode::Search { buf };
            }
            _ => self.mode = Mode::Search { buf },
        }
    }

    /// Left-click: replay (column, terminal row) against the hit regions
    /// of the frame that is actually on screen. Anything that isn't a
    /// recorded hit is dead space — a click never invents an action.
    fn click(&mut self, col: usize, row: usize) {
        if row >= self.view_rows {
            return;
        }
        let line = row;
        let target = self
            .hits
            .iter()
            .find(|h| h.line == line && col >= h.x0 && col < h.x1)
            .map(|h| h.target.clone());
        match target {
            Some(render::HitTarget::Row(key)) => self.selected = Some(key),
            // same move as tab: the queue thinks in tasks, the cursor in
            // rows — land on the task's latest real row, unfolding to it
            Some(render::HitTarget::Task(tid)) => self.jump_to_task(&tid),
            // the ▸ chip only exists on a folded row: a click opens it
            Some(render::HitTarget::Fold(key)) => {
                self.folded.remove(&key);
            }
            Some(render::HitTarget::Message) => self.start_message(),
            Some(render::HitTarget::Details) => {
                if !self.details_open {
                    self.toggle_details();
                }
            }
            None => {}
        }
    }

    /// Plain text of the live selection, line by line, as it looked on
    /// screen. Trailing blanks go: nobody wants the pad columns a block
    /// drag ran through.
    fn selection_text(&self, width: usize) -> String {
        let Some(sel) = self.sel else { return String::new() };
        let ((l0, _), (l1, _)) = sel.span();
        let mut out: Vec<String> = Vec::new();
        for line in l0..=l1 {
            let Some(painted) = self.frame.get(line) else { continue };
            let Some((c0, c1)) = sel.cols_on(line, width) else { continue };
            out.push(select::slice(painted, c0, c1).trim_end().to_string());
        }
        out.join("\n")
    }

    fn focus(&mut self) {
        let locator = self.selected.as_deref().and_then(|k| {
            let (_, attempt) = self.task_target()?;
            if attempt != k {
                return None;
            }
            let tasks = self.loaded.doc.tasks.as_deref()?;
            let mut matches = tasks
                .iter()
                .flat_map(|t| t.attempts.iter())
                .filter(|a| a.id.as_deref() == Some(k));
            let attempt = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            attempt.locator.as_ref().map(|l| (l.pane.clone(), l.agent.clone()))
        });
        let msg = match locator {
            None => "no live locator on this row".into(),
            Some((None, None)) => "locator names neither pane nor agent".into(),
            Some(_) if self.herdr.is_none() => "not inside herdr — cannot focus".into(),
            Some(_) if self.bg_rx.is_some() => "focus already in flight".into(),
            Some((pane, agent)) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let target = pane.clone().unwrap_or_else(|| agent.clone().unwrap_or_default());
                std::thread::spawn(move || {
                    let r = match (pane, agent) {
                        (Some(p), _) => herdr::focus_pane(&p).map(|()| format!("focused {p}")),
                        (None, Some(a)) => {
                            herdr::focus_agent(&a).map(|()| format!("focused agent {a}"))
                        }
                        (None, None) => unreachable!(),
                    };
                    let _ = tx.send(r.unwrap_or_else(|e| format!("focus failed: {e}")));
                });
                self.bg_rx = Some(rx);
                format!("focusing {target}…")
            }
        };
        self.flash = Some((msg, 12));
    }

    /// Poll the in-flight focus (called every tick).
    fn poll_bg(&mut self) {
        let Some(rx) = &self.bg_rx else { return };
        match rx.try_recv() {
            Ok(msg) => {
                self.flash = Some((msg, 12));
                self.bg_rx = None;
                let run_id = self.loaded.doc.run.as_ref().and_then(|run| run.id.as_deref());
                if let Ok(messages) =
                    message::read_summaries(std::path::Path::new(&self.path), run_id)
                {
                    self.loaded.message_summaries = messages;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.bg_rx = None;
            }
        }
    }

    /// Resolve the selected task and, when present, its current attempt.
    fn task_target(&self) -> Option<(String, String)> {
        let sel = self.selected.as_deref()?;
        if !crate::contract::valid_identity(sel)
            || self.loaded.doc.projects.iter().any(|project| {
                project.id.as_deref().is_some_and(|id| format!("project:{id}") == sel)
            })
        {
            return None;
        }
        let tasks = self.loaded.doc.tasks.as_deref()?;
        let mut found = Vec::new();
        for t in tasks {
            for attempt in &t.attempts {
                if attempt.id.as_deref() == Some(sel) {
                    found.push((t.id.clone().unwrap_or_default(), sel.to_string()));
                }
            }
            if t.id.as_deref() == Some(sel) {
                let latest = if crate::model::needs_current_stub(t) {
                    String::new()
                } else {
                    t.attempts
                        .iter()
                        .max_by_key(|a| a.n.unwrap_or(0))
                        .and_then(|a| a.id.clone())
                        .unwrap_or_default()
                };
                found.push((sel.to_string(), latest));
            }
        }
        if found.len() != 1 || !crate::contract::valid_identity(&found[0].0) {
            return None;
        }
        let attempt = found[0].1.as_str();
        if !attempt.is_empty()
            && (!crate::contract::valid_identity(attempt)
                || tasks
                    .iter()
                    .flat_map(|task| task.attempts.iter())
                    .filter(|candidate| candidate.id.as_deref() == Some(attempt))
                    .count()
                    != 1)
        {
            return None;
        }
        Some(found.remove(0))
    }

    fn start_message(&mut self) {
        let Some((target, _)) = self.task_target() else {
            self.flash = Some(("select a task or gate first".into(), 8));
            return;
        };
        if self.loaded.message_starters.is_empty() {
            self.flash = Some(("no message starters available".into(), 8));
            return;
        }
        self.mode = Mode::Message {
            draft: message::Draft::from_starter(target, &self.loaded.message_starters, 0),
        };
    }

    fn submit_message(&mut self, draft: message::Draft) {
        if self.bg_rx.is_some() {
            self.flash = Some(("another command is still in flight".into(), 10));
            self.mode = Mode::Message { draft };
            return;
        }
        let prepared = message::prepare(
            std::path::Path::new(&self.path),
            &self.loaded.doc,
            &draft.target,
            &draft.starter_id,
            &draft.text,
            draft.authority,
        );
        let submission = match prepared {
            Ok(submission) => submission,
            Err(e) => {
                self.flash = Some((e, 16));
                self.mode = Mode::Message { draft };
                return;
            }
        };
        let id = submission.id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(message::deliver(submission));
        });
        self.bg_rx = Some(rx);
        self.flash = Some((format!("recorded {id}; queueing…"), 20));
    }

    /// One key in the contextual message composer. Starters replace the
    /// editable draft on Tab; ordinary text (including digits) remains
    /// completely free-form.
    fn message_key(&mut self, mut draft: message::Draft, k: event::KeyEvent) {
        use event::KeyCode::*;
        let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);
        let plain = !k
            .modifiers
            .intersects(event::KeyModifiers::CONTROL | event::KeyModifiers::ALT);
        match k.code {
            Esc => {
                self.flash = Some(("message cancelled".into(), 6));
            }
            Char('c') if ctrl => {
                self.flash = Some(("message cancelled".into(), 6));
            }
            Enter => self.submit_message(draft),
            Tab | BackTab => {
                let n = self.loaded.message_starters.len();
                if n > 0 {
                    let next = if matches!(k.code, BackTab) {
                        (draft.starter + n - 1) % n
                    } else {
                        (draft.starter + 1) % n
                    };
                    draft = draft.switch_to(&self.loaded.message_starters, next);
                }
                self.mode = Mode::Message { draft };
            }
            Char('t') if ctrl => {
                draft.authority = draft.authority.toggled();
                self.mode = Mode::Message { draft };
            }
            Char('u') if ctrl => {
                draft.text.clear();
                self.mode = Mode::Message { draft };
            }
            Char('w') if ctrl => {
                while draft.text.chars().last().is_some_and(|c| c.is_whitespace()) {
                    draft.text.pop();
                }
                while draft.text.chars().last().is_some_and(|c| !c.is_whitespace()) {
                    draft.text.pop();
                }
                self.mode = Mode::Message { draft };
            }
            Backspace => {
                draft.text.pop();
                self.mode = Mode::Message { draft };
            }
            Char(c) if plain => {
                if draft.text.len() + c.len_utf8() <= message::MESSAGE_LIMIT {
                    draft.text.push(c);
                } else {
                    self.flash = Some(("message is limited to 32 KiB".into(), 8));
                }
                self.mode = Mode::Message { draft };
            }
            _ => self.mode = Mode::Message { draft },
        }
    }

    /// The modal prompt line, if any (drawn above the footer).
    fn prompt_line(&self) -> Option<String> {
        match &self.mode {
            Mode::Normal => None,
            Mode::Message { draft } => {
                let starter = self
                    .loaded
                    .message_starters
                    .get(draft.starter)
                    .map(|s| s.label.as_str())
                    .unwrap_or("Custom");
                Some(format!(
                    "message → {} · {} ({}/{}) · authority: {}\n{}▏\nenter queue · tab starter · ctrl-t authority · esc cancel · config {}",
                    draft.target,
                    starter,
                    draft.starter + 1,
                    self.loaded.message_starters.len(),
                    draft.authority.label(),
                    draft.text,
                    self.loaded.message_config_path.display(),
                ))
            }
            // the picker draws its own frame; search lives on the bottom line
            Mode::Picker(_) | Mode::Search { .. } => None,
        }
    }

    fn reload(&mut self) {
        // the frame is about to be rebuilt from a new revision; a
        // highlight anchored to the old lines would frame the wrong text
        self.sel = None;
        match load(&self.path) {
            Ok(l) => {
                self.loaded = l;
                let stale_message = match &self.mode {
                    Mode::Message { draft } => {
                        !message::unique_task_target(&self.loaded.doc, &draft.target)
                    }
                    _ => false,
                };
                if stale_message {
                    self.mode = Mode::Normal;
                    self.flash = Some((
                        "message cancelled: target is no longer unique in this run".into(),
                        12,
                    ));
                }
                // view state that names rows survives only as long as the
                // rows do
                self.zoom.retain(|k| model::key_exists(&self.loaded.doc, k));
                self.folded.retain(|k| model::key_exists(&self.loaded.doc, k));
                let detail_target = self.selected.clone();
                self.snap_selection();
                if self.details_open && self.selected != detail_target {
                    self.details_open = false;
                    self.detail_scroll = 0;
                    self.detail_scroll_max = 0;
                    self.flash = Some(("details closed: selected row changed on reload".into(), 10));
                }
                self.update_watch();
            }
            Err(e) => self.flash = Some((e, 20)),
        }
    }
}

fn interactive(args: &ViewArgs) -> Result<(), String> {
    // A missing or malformed file is not fatal in the pane: start on an
    // empty scene with a banner and let the mtime watch pick the file up
    // when the producer writes it (graceful degradation).
    let loaded = load(&args.path);
    let initial_ok = loaded.is_ok();
    let loaded = loaded.unwrap_or_else(|e| Loaded {
        doc: empty_doc(),
        banner: Some(format!("waiting for run file — {e}")),
        generated_min: None,
        chip: None,
        message_starters: message::defaults(),
        message_config_path: message::config_path(std::path::Path::new(&args.path)),
        message_summaries: Vec::new(),
    });
    if initial_ok {
        // seed the `f` picker's recent list — sibling panes see it too
        picker::mru_add(std::path::Path::new(&args.path));
    }
    let mut app = App {
        path: args.path.clone(),
        loaded,
        selected: args.select.clone(),
        queue_pos: 0,
        flash: None,
        help: false,
        scroll: 0,
        herdr: herdr::Link::start(),
        mode: Mode::Normal,
        bg_rx: None,
        hits: Vec::new(),
        view_rows: 0,
        page_rows: 0,
        details_open: false,
        detail_scroll: 0,
        detail_scroll_max: 0,
        detail_page_rows: 0,
        frame: Vec::new(),
        sel: None,
        painted: Vec::new(),
        zoom: Vec::new(),
        folded: std::collections::HashSet::new(),
        search: None,
        search_matches: 0,
        last_click: None,
    };
    app.update_watch();
    app.auto_select();

    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    // bracketed paste makes a multi-line paste ONE event the text prompt
    // can absorb whole, instead of a key stream whose embedded newline
    // finalizes the intent and whose tail answers the confirm gate (F3)
    execute!(
        stdout,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        event::EnableBracketedPaste,
        event::EnableMouseCapture
    )
    .map_err(|e| e.to_string())?;
    let result = event_loop(&mut app, &mut stdout);
    let _ = execute!(
        stdout,
        event::DisableMouseCapture,
        event::DisableBracketedPaste,
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
    result
}

fn event_loop(app: &mut App, stdout: &mut std::io::Stdout) -> Result<(), String> {
    let mut last_mtime: Option<SystemTime> = std::fs::metadata(&app.path)
        .and_then(|m| m.modified())
        .ok();
    let mut watched_path = app.path.clone();
    let mut config_path = message::config_path(std::path::Path::new(&app.path));
    let mut journal_path = message::journal_path(std::path::Path::new(&app.path));
    let sidecar_mtime = |path: &std::path::Path| {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    };
    let mut last_config_mtime = sidecar_mtime(&config_path);
    let mut last_journal_mtime = sidecar_mtime(&journal_path);
    loop {
        app.poll_bg();
        app.poll_picker();
        draw(app, stdout)?;
        if event::poll(Duration::from_millis(300)).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                event::Event::Key(k) if k.kind != event::KeyEventKind::Release => {
                    use event::KeyCode::*;
                    // a keystroke moves on: the highlight belongs to the
                    // mouse gesture that made it
                    app.sel = None;
                    // modal keys first: while typing, 'q' is a letter
                    match std::mem::replace(&mut app.mode, Mode::Normal) {
                        Mode::Picker(st) => {
                            app.picker_key(st, k);
                            continue;
                        }
                        Mode::Search { buf } => {
                            app.search_key(buf, k);
                            continue;
                        }
                        Mode::Message { draft } => {
                            app.message_key(draft, k);
                            continue;
                        }
                        Mode::Normal => {}
                    }
                    let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);
                    if k.code == Char('c') && ctrl {
                        return Ok(());
                    }
                    if app.details_open && !app.help {
                        match k.code {
                            Char('q') => return Ok(()),
                            Char('d') if ctrl => {
                                app.move_detail(app.detail_page_rows.max(1) as i64)
                            }
                            Char('u') if ctrl => {
                                app.move_detail(-((app.detail_page_rows.max(1)) as i64))
                            }
                            PageDown => app.move_detail(app.detail_page_rows.max(1) as i64),
                            PageUp => app.move_detail(-((app.detail_page_rows.max(1)) as i64)),
                            Esc | Char('d') => app.toggle_details(),
                            Char('j') | Down => app.move_detail(1),
                            Char('k') | Up => app.move_detail(-1),
                            Char('g') | Home => app.detail_scroll = 0,
                            Char('G') | End => app.detail_scroll = app.detail_scroll_max,
                            Enter => app.focus(),
                            Char('m') => app.start_message(),
                            Char('y') => {
                                if let Some(key) = app.selected.clone() {
                                    write!(stdout, "{}", select::osc52(&key))
                                        .map_err(|e| e.to_string())?;
                                    stdout.flush().map_err(|e| e.to_string())?;
                                    app.flash = Some((format!("copied {key}"), 8));
                                }
                            }
                            Char('?') => app.help = true,
                            _ => {}
                        }
                        continue;
                    }
                    match k.code {
                        Char('q') => return Ok(()),
                        Esc => {
                            if app.help {
                                app.help = false;
                            } else if app.zoom.pop().is_some() {
                                app.flash = Some(("zoomed out".into(), 6));
                            } else {
                                return Ok(());
                            }
                        }
                        // In normal mode ctrl-u pages; the composer handles
                        // the same chord earlier as clear-line.
                        Char('d') if ctrl => app.move_sel((app.page_rows / 2).max(1) as i64),
                        Char('u') if ctrl => app.move_sel(-((app.page_rows / 2).max(1) as i64)),
                        Char('j') | Down => app.move_sel(1),
                        Char('k') | Up => app.move_sel(-1),
                        Char('g') => {
                            let keys = app.selectable_keys();
                            if let Some(f) = keys.first() {
                                app.selected = Some(f.clone());
                            }
                        }
                        Char('G') => {
                            let keys = app.selectable_keys();
                            if let Some(l) = keys.last() {
                                app.selected = Some(l.clone());
                            }
                        }
                        Right | Char('l') => {
                            if let Some(key) = app.selected.clone() {
                                app.open_or_zoom(key);
                            }
                        }
                        Left | Char('h') => app.fold_or_up(),
                        Char('z') => app.toggle_settled(),
                        Char('d') => app.toggle_details(),
                        Char('f') => app.enter_picker(),
                        Char('/') => {
                            app.search_matches = 0;
                            app.mode = Mode::Search { buf: String::new() };
                        }
                        Char('n') => app.search_jump(1),
                        Char('N') => app.search_jump(-1),
                        Char('y') => {
                            if let Some(key) = app.selected.clone() {
                                write!(stdout, "{}", select::osc52(&key))
                                    .map_err(|e| e.to_string())?;
                                stdout.flush().map_err(|e| e.to_string())?;
                                app.flash = Some((format!("copied {key}"), 8));
                            }
                        }
                        Tab => app.cycle_queue(),
                        Enter => app.focus(),
                        Char('r') => {
                            app.reload();
                            app.flash = Some(("reloaded".into(), 6));
                        }
                        Char('?') => app.help = !app.help,
                        Char('m') => app.start_message(),
                        _ => {}
                    }
                }
                event::Event::Paste(s) => {
                    // a paste is text for the text prompt and NOTHING
                    // else: never a stream of keys a confirm gate could
                    // mistake for a decision. Newlines
                    // become spaces — one paste, one line, no hidden
                    // Enter riding along.
                    let clean = || {
                        s.chars()
                            .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
                            .filter(|c| !c.is_control())
                            .collect::<String>()
                    };
                    let clean = clean();
                    if let Mode::Message { draft } = &mut app.mode {
                        let remaining = message::MESSAGE_LIMIT.saturating_sub(draft.text.len());
                        let end = clean
                            .char_indices()
                            .map(|(i, _)| i)
                            .take_while(|&i| i <= remaining)
                            .last()
                            .unwrap_or(0);
                        let end = if clean.len() <= remaining { clean.len() } else { end };
                        draft.text.push_str(&clean[..end]);
                        if end < clean.len() {
                            app.flash = Some(("message paste truncated at 32 KiB".into(), 8));
                        }
                    }
                }
                event::Event::Mouse(m) => {
                    use event::{MouseButton, MouseEventKind};
                    let (col, row) = (m.column as usize, m.row as usize);
                    let normal = matches!(app.mode, Mode::Normal);
                    match m.kind {
                        // press → drag → release IS the selection in every
                        // mode. The mouse cannot submit the composer.
                        MouseEventKind::Down(MouseButton::Left) => {
                            app.sel = (row < app.view_rows)
                                .then(|| select::Sel::new(row, col));
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(sel) = app.sel.as_mut() {
                                sel.to(row.min(app.view_rows.saturating_sub(1)), col);
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            let dragged = app.sel.is_some_and(|s| s.dragged);
                            if dragged {
                                // herdr's contract with the user: let go of
                                // a selection and it is on the clipboard
                                let (w, _) = terminal::size().map_err(|e| e.to_string())?;
                                let text = app.selection_text(w as usize);
                                if !text.is_empty() {
                                    write!(stdout, "{}", select::osc52(&text))
                                        .map_err(|e| e.to_string())?;
                                    let n = text.lines().count();
                                    let unit = if n == 1 { "line" } else { "lines" };
                                    app.flash = Some((format!("copied {n} {unit}"), 8));
                                }
                            } else if normal {
                                // a plain click: interactive rows select,
                                // everything else just drops the highlight
                                if app.help {
                                    app.help = false;
                                } else if row < app.view_rows {
                                    // two clicks on the same line inside
                                    // 450ms zoom the row (a folded one
                                    // opens instead) — same as →
                                    let line = row;
                                    let now = std::time::Instant::now();
                                    let double = app.last_click.take().is_some_and(|(t, l)| {
                                        l == line
                                            && now.duration_since(t)
                                                < Duration::from_millis(450)
                                    });
                                    let hit_row = app
                                        .hits
                                        .iter()
                                        .find(|h| h.line == line && col >= h.x0 && col < h.x1)
                                        .map(|h| h.target.clone());
                                    app.click(col, row);
                                    match (double, hit_row) {
                                        (true, Some(render::HitTarget::Row(key))) => {
                                            app.open_or_zoom(key)
                                        }
                                        _ => app.last_click = Some((now, line)),
                                    }
                                }
                                app.sel = None;
                            } else {
                                app.sel = None;
                            }
                        }
                        MouseEventKind::ScrollDown if normal && !app.help => {
                            app.sel = None;
                            if app.details_open {
                                app.move_detail(1);
                            } else {
                                app.move_sel(1);
                            }
                        }
                        MouseEventKind::ScrollUp if normal && !app.help => {
                            app.sel = None;
                            if app.details_open {
                                app.move_detail(-1);
                            } else {
                                app.move_sel(-1);
                            }
                        }
                        _ => {}
                    }
                }
                event::Event::Resize(_, _) => {
                    // a resize is the one moment the screen can hold
                    // garbage we did not put there: repaint unconditionally
                    app.painted.clear();
                }
                _ => {}
            }
        }
        // The file picker can switch runs without restarting the loop; move
        // all three watches (run, prompt config, message journal) together.
        if app.path != watched_path {
            watched_path = app.path.clone();
            config_path = message::config_path(std::path::Path::new(&app.path));
            journal_path = message::journal_path(std::path::Path::new(&app.path));
            last_mtime = std::fs::metadata(&app.path).and_then(|m| m.modified()).ok();
            last_config_mtime = sidecar_mtime(&config_path);
            last_journal_mtime = sidecar_mtime(&journal_path);
        }
        // watch: reload when the producer rewrites the file
        if let Ok(m) = std::fs::metadata(&app.path).and_then(|m| m.modified()) {
            if last_mtime.map(|t| m > t).unwrap_or(true) {
                last_mtime = Some(m);
                app.reload();
            }
        }
        let config_mtime = sidecar_mtime(&config_path);
        let journal_mtime = sidecar_mtime(&journal_path);
        if config_mtime != last_config_mtime || journal_mtime != last_journal_mtime {
            last_config_mtime = config_mtime;
            last_journal_mtime = journal_mtime;
            app.reload();
        }
        if let Some((_, ttl)) = &mut app.flash {
            if *ttl == 0 {
                app.flash = None;
            } else {
                *ttl -= 1;
            }
        }
    }
}

/// Viewport start inside one independently scrollable region. The caller
/// passes the graph's length, not the length of the entire composed frame:
/// detail and footer rows are docks, not scrollback.
fn viewport_start(
    frame_len: usize,
    visible: usize,
    sel_line: Option<usize>,
    scroll: usize,
) -> usize {
    let mut s = scroll;
    if let (Some(sel), true) = (sel_line, visible > 0) {
        if sel < s {
            s = sel;
        } else if sel >= s + visible {
            s = sel + 1 - visible;
        }
    }
    if frame_len > visible { s.min(frame_len - visible) } else { 0 }
}

const MIN_GRAPH_ROWS: usize = 3;

enum ScreenLine {
    Frame(usize),
    Generated(String),
}

struct ScreenFrame {
    lines: Vec<String>,
    hits: Vec<render::Hit>,
    graph_start: usize,
    graph_rows: usize,
    detail_scroll: usize,
    detail_scroll_max: usize,
    detail_page_rows: usize,
}

/// When a producer-controlled detail value is taller than the terminal can
/// physically display, preserve the card heading and tail and make the
/// omission explicit. Ordinary cards are never shortened: this path is only
/// reached after the dock has claimed every row except a small graph context
/// and the fixed footer.
fn detail_omission_line(width: usize, hidden: usize) -> String {
    let cw = width.saturating_sub(1);
    if cw < 8 {
        return style::paint("…", style::Style::dim(style::MUTED));
    }
    let mut line = style::Line::new(cw);
    line.put(0, "│ ", style::Style::fg(style::RULE));
    let unit = if hidden == 1 { "row" } else { "rows" };
    line.put(
        2,
        &format!("… {hidden} more detail {unit} · enlarge pane to reveal"),
        style::Style::dim(style::MUTED),
    );
    line.put(cw.saturating_sub(2), " │", style::Style::fg(style::RULE));
    line.render(None, true)
}

fn fit_detail(
    start: usize,
    end: usize,
    budget: usize,
    width: usize,
) -> Vec<ScreenLine> {
    let all: Vec<usize> = (start..end).collect();
    if all.len() <= budget {
        return all.into_iter().map(ScreenLine::Frame).collect();
    }
    if budget == 0 {
        return Vec::new();
    }

    // compose() prefixes a real selection card with one breathing row. If
    // the dock is constrained, spend the row on information instead.
    let card = if all.len() > 1 { &all[1..] } else { &all[..] };
    match budget {
        1 => vec![ScreenLine::Frame(card[0])],
        2 => vec![ScreenLine::Frame(card[0]), ScreenLine::Frame(*card.last().unwrap())],
        _ => {
            let tail_n = if budget >= 4 { 2 } else { 1 };
            let head_n = budget.saturating_sub(tail_n + 1).max(1);
            let head_n = head_n.min(card.len().saturating_sub(tail_n));
            let tail_start = card.len().saturating_sub(tail_n).max(head_n);
            let mut planned = card[..head_n]
                .iter()
                .copied()
                .map(ScreenLine::Frame)
                .collect::<Vec<_>>();
            let hidden = tail_start.saturating_sub(head_n);
            planned.push(ScreenLine::Generated(detail_omission_line(width, hidden)));
            planned.extend(card[tail_start..].iter().copied().map(ScreenLine::Frame));
            planned.truncate(budget);
            planned
        }
    }
}

fn detail_position_line(width: usize, first: usize, last: usize, total: usize) -> String {
    let cw = width.saturating_sub(1);
    let mut line = style::Line::new(cw);
    let above = first > 1;
    let below = last < total;
    let arrows = match (above, below) {
        (true, true) => "↑↓",
        (true, false) => "↑",
        (false, true) => "↓",
        (false, false) => "",
    };
    line.put(
        2.min(cw),
        &format!("details {first}–{last}/{total} {arrows}"),
        style::Style::dim(style::MUTED),
    );
    line.render(None, true)
}

/// Scroll only the explanatory body. The identity heading and action/closing
/// rows stay fixed so the user never loses either orientation or escape
/// affordances while reading a long receipt.
fn scroll_detail(
    frame: &render::Frame,
    start: usize,
    end: usize,
    budget: usize,
    width: usize,
    requested: usize,
) -> (Vec<ScreenLine>, usize, usize, usize) {
    if budget == 0 {
        return (Vec::new(), 0, 0, 0);
    }
    let len = end.saturating_sub(start);
    if len <= budget {
        let mut lines = (start..end).map(ScreenLine::Frame).collect::<Vec<_>>();
        while lines.len() < budget {
            lines.push(ScreenLine::Generated(String::new()));
        }
        return (lines, 0, 0, len.saturating_sub(3));
    }
    if budget < 5 || len < 3 {
        let mut lines = fit_detail(start, end, budget, width);
        while lines.len() < budget {
            lines.push(ScreenLine::Generated(String::new()));
        }
        return (lines, 0, 0, 0);
    }

    let action = (start + 1..end)
        .rev()
        .find(|line| {
            frame
                .lines
                .get(*line)
                .is_some_and(|text| text.contains("[m] message"))
        });
    let tail_start = action.unwrap_or_else(|| end.saturating_sub(1));
    let tail_len = end.saturating_sub(tail_start);
    let fixed = 2 + tail_len; // heading + position + action/border tail
    if fixed >= budget || tail_start <= start {
        let mut lines = fit_detail(start, end, budget, width);
        while lines.len() < budget {
            lines.push(ScreenLine::Generated(String::new()));
        }
        return (lines, 0, 0, 0);
    }

    let body_start = start + 1;
    let body_len = tail_start.saturating_sub(body_start);
    let body_rows = budget - fixed;
    let max_scroll = body_len.saturating_sub(body_rows);
    let scroll = requested.min(max_scroll);
    let visible_body = body_len.saturating_sub(scroll).min(body_rows);
    let first = if body_len == 0 { 0 } else { scroll + 1 };
    let last = scroll + visible_body;

    let mut lines = Vec::with_capacity(budget);
    lines.push(ScreenLine::Frame(start));
    lines.push(ScreenLine::Generated(detail_position_line(width, first, last, body_len)));
    lines.extend(
        (body_start + scroll..body_start + scroll + visible_body).map(ScreenLine::Frame),
    );
    while lines.len() + tail_len < budget {
        lines.push(ScreenLine::Generated(String::new()));
    }
    lines.extend((tail_start..end).map(ScreenLine::Frame));
    lines.truncate(budget);
    (lines, scroll, max_scroll, body_rows)
}

/// Materialize the terminal screen from the renderer's semantic regions.
/// The graph scrolls; selected-item detail and the command footer remain
/// fixed at the bottom. This is deliberately a second phase after horizontal
/// composition: terminal width determines wrapping, then terminal height
/// determines region allocation.
#[cfg(test)]
fn screen_frame(
    frame: &render::Frame,
    visible: usize,
    width: usize,
    scroll: usize,
) -> ScreenFrame {
    screen_frame_mode(frame, visible, width, scroll, false, 0)
}

fn screen_frame_mode(
    frame: &render::Frame,
    visible: usize,
    width: usize,
    scroll: usize,
    details_open: bool,
    detail_scroll: usize,
) -> ScreenFrame {
    if visible == 0 {
        return ScreenFrame {
            lines: Vec::new(),
            hits: Vec::new(),
            graph_start: 0,
            graph_rows: 0,
            detail_scroll: 0,
            detail_scroll_max: 0,
            detail_page_rows: 0,
        };
    }

    let graph_end = frame.graph_end.min(frame.lines.len());
    let detail_end = frame.detail_end.clamp(graph_end, frame.lines.len());
    let footer_len = frame.lines.len().saturating_sub(detail_end);
    let footer_take = footer_len.min(visible);
    let footer_start = frame.lines.len().saturating_sub(footer_take);
    let remaining = visible.saturating_sub(footer_take);

    let detail_len = detail_end.saturating_sub(graph_end);
    let (graph_rows, detail_take) = if details_open {
        // Focus mode makes one deliberate, terminal-sized transition. Its
        // boundary is independent of content length and selection.
        let min_detail = 5.min(remaining);
        let graph_rows = graph_end.min(remaining.saturating_sub(min_detail));
        (graph_rows, remaining.saturating_sub(graph_rows))
    } else {
        let graph_reserve = graph_end.min(MIN_GRAPH_ROWS).min(remaining);
        let detail_budget = remaining.saturating_sub(graph_reserve);
        let detail_take = detail_len.min(detail_budget);
        (remaining.saturating_sub(detail_take), detail_take)
    };
    let graph_start = if details_open {
        // The causal lens is the final six graph rows. On very short panes,
        // sacrifice the run heading/spacer before sacrificing either side of
        // the selected node; the user opened this mode to see inputs *and*
        // outputs together.
        graph_end.saturating_sub(graph_rows)
    } else {
        viewport_start(graph_end, graph_rows, frame.sel_line, scroll)
    };

    let mut planned = Vec::with_capacity(visible);
    planned.extend(
        (graph_start..graph_end)
            .take(graph_rows)
            .map(ScreenLine::Frame),
    );
    while planned.len() < graph_rows {
        planned.push(ScreenLine::Generated(String::new()));
    }
    let (detail_lines, detail_scroll, detail_scroll_max, detail_page_rows) = if details_open {
        scroll_detail(frame, graph_end, detail_end, detail_take, width, detail_scroll)
    } else {
        (fit_detail(graph_end, detail_end, detail_take, width), 0, 0, 0)
    };
    planned.extend(detail_lines);
    planned.extend((footer_start..frame.lines.len()).map(ScreenLine::Frame));

    let mut source_to_screen = vec![None; frame.lines.len()];
    for (screen_line, source) in planned.iter().enumerate() {
        let ScreenLine::Frame(source_line) = source else { continue };
        if let Some(slot) = source_to_screen.get_mut(*source_line) {
            *slot = Some(screen_line);
        }
    }
    let hits = frame
        .hits
        .iter()
        .filter_map(|hit| {
            let screen_line = source_to_screen.get(hit.line).copied().flatten()?;
            let mut hit = hit.clone();
            hit.line = screen_line;
            Some(hit)
        })
        .collect();
    let lines = planned
        .into_iter()
        .map(|line| match line {
            ScreenLine::Frame(i) => frame.lines.get(i).cloned().unwrap_or_default(),
            ScreenLine::Generated(line) => line,
        })
        .collect();
    ScreenFrame {
        lines,
        hits,
        graph_start,
        graph_rows,
        detail_scroll,
        detail_scroll_max,
        detail_page_rows,
    }
}

fn draw(app: &mut App, stdout: &mut std::io::Stdout) -> Result<(), String> {
    let (w, h) = terminal::size().map_err(|e| e.to_string())?;
    let (w, h) = ((w as usize).min(crate::scale::MAX_FRAME_WIDTH), h as usize);
    let main_view = !app.help && !matches!(app.mode, Mode::Picker(_));
    let frame = if app.help {
        let lines = render::help_lines();
        render::Frame {
            graph_end: lines.len(),
            detail_end: lines.len(),
            lines,
            sel_line: None,
            hits: Vec::new(),
        }
    } else if let Mode::Picker(st) = &app.mode {
        let (lines, sel) = picker::lines(st, w);
        render::Frame {
            graph_end: lines.len(),
            detail_end: lines.len(),
            lines,
            sel_line: sel,
            hits: Vec::new(),
        }
    } else {
        let hints = app.hints();
        let scene = model::build(
            &app.loaded.doc,
            app.selected.as_deref(),
            hints.as_ref(),
            app.loaded.chip.as_deref(),
            &app.view_opts(),
        );
        render::compose_with_inspector(
            &render::FrameInput {
                doc: &app.loaded.doc,
                scene: &scene,
                selected: app.selected.as_deref(),
                banner: app.loaded.banner.clone(),
                flash: app.flash.as_ref().map(|(m, _)| m.clone()),
                stale_min: stale_min(app.loaded.generated_min),
                watching: true,
                herdr: hints.as_ref(),
                prompt: app.prompt_line(),
                messages: &app.loaded.message_summaries,
            },
            w,
            if app.details_open {
                render::InspectorMode::Focus
            } else {
                render::InspectorMode::Compact
            },
        )
    };
    let visible = h.saturating_sub(1);
    let layout_scroll = if app.help { 0 } else { app.scroll };
    let layout_details = main_view && app.details_open;
    let screen = screen_frame_mode(
        &frame,
        visible,
        w,
        layout_scroll,
        layout_details,
        app.detail_scroll,
    );
    app.hits = screen.hits;
    app.view_rows = visible;
    app.page_rows = screen.graph_rows;
    app.detail_scroll = screen.detail_scroll;
    app.detail_scroll_max = screen.detail_scroll_max;
    app.detail_page_rows = screen.detail_page_rows;
    if main_view && !app.details_open {
        app.scroll = screen.graph_start;
    }
    app.frame = screen.lines;
    // Paint into ONE buffer and write it in ONE syscall. The old path
    // flushed a full-screen Clear on its own and then flushed again per
    // line, so the terminal had a genuinely blank screen to draw between
    // them — that is the flicker. Each row now ends with a clear-to-EOL
    // instead, which erases the old content of that row in the same write
    // that draws the new one.
    let mut buf: Vec<u8> = Vec::with_capacity(w * visible.max(1) + 64);
    let mut drawn = 0usize;
    for (i, idx) in (0..app.frame.len()).take(visible.max(1)).enumerate() {
        let line = &app.frame[idx];
        queue!(buf, cursor::MoveTo(0, i as u16)).map_err(|e| e.to_string())?;
        match app.sel.and_then(|s| s.cols_on(idx, w)) {
            Some((c0, c1)) => write!(buf, "{}", select::highlight(line, c0, c1)),
            None => write!(buf, "{line}"),
        }
        .map_err(|e| e.to_string())?;
        queue!(buf, terminal::Clear(ClearType::UntilNewLine)).map_err(|e| e.to_string())?;
        drawn = i + 1;
    }
    // rows the frame no longer reaches (it shrank, or the terminal grew)
    for i in drawn..visible {
        queue!(buf, cursor::MoveTo(0, i as u16), terminal::Clear(ClearType::UntilNewLine))
            .map_err(|e| e.to_string())?;
    }
    // the bottom terminal row is never part of the frame — the live `/`
    // query draws there, always on screen no matter where the trace is
    if h > 0 {
        queue!(buf, cursor::MoveTo(0, (h - 1) as u16), terminal::Clear(ClearType::UntilNewLine))
            .map_err(|e| e.to_string())?;
        let es = |n: usize| if n == 1 { "" } else { "es" };
        let text = match (&app.mode, &app.search) {
            (Mode::Search { buf: q }, _) => Some(format!(
                "/{q}▏  {} match{} — enter keeps it · esc clears",
                app.search_matches,
                es(app.search_matches)
            )),
            (Mode::Picker(_), _) => None,
            (_, Some(q)) => Some(format!(
                "/{q}  n/N cycles {} match{}",
                app.search_matches,
                es(app.search_matches)
            )),
            _ => None,
        };
        if let Some(t) = text {
            write!(
                buf,
                " {}",
                style::paint(&style::trunc(&t, w.saturating_sub(3)), style::Style::bold(style::ACCENT))
            )
            .map_err(|e| e.to_string())?;
        }
    }
    // An identical frame is not worth a repaint: the event loop wakes on a
    // 300ms poll timeout whether or not anything happened, and repainting
    // an unchanged screen three times a second is the other half of the
    // flicker.
    if buf == app.painted {
        return Ok(());
    }
    stdout.write_all(&buf).map_err(|e| e.to_string())?;
    app.painted = buf;
    stdout.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use event::{KeyCode, KeyModifiers};

    fn message_test_app() -> App {
        let starters = message::defaults();
        App {
            path: "run.json".into(),
            loaded: Loaded {
                doc: serde_json::from_str(
                    r#"{"dagr":3,"run":{"id":"r","orchestrator":{"pane":"wX:p1"}},"tasks":[]}"#,
                )
                .unwrap(),
                banner: None,
                generated_min: None,
                chip: None,
                message_starters: starters,
                message_config_path: "actions.json".into(),
                message_summaries: Vec::new(),
            },
            selected: None,
            queue_pos: 0,
            flash: None,
            help: false,
            scroll: 0,
            herdr: None,
            mode: Mode::Normal,
            bg_rx: None,
            hits: Vec::new(),
            view_rows: 0,
            page_rows: 0,
            details_open: false,
            detail_scroll: 0,
            detail_scroll_max: 0,
            detail_page_rows: 0,
            frame: Vec::new(),
            sel: None,
            painted: Vec::new(),
            zoom: Vec::new(),
            folded: std::collections::HashSet::new(),
            search: None,
            search_matches: 0,
            last_click: None,
        }
    }

    #[test]
    fn keyboard_navigation_treats_projects_as_real_inert_nodes() {
        let mut app = message_test_app();
        app.loaded.doc = serde_json::from_value(serde_json::json!({
            "dagr": 3,
            "run": {"id": "projects"},
            "projects": [
                {"id": "ROOT", "title": "Recovery"},
                {"id": "CORE", "title": "Core", "parent": "ROOT"}
            ],
            "tasks": [
                {"id": "PLAN", "title": "plan", "kind": "plan", "project": "CORE",
                 "state": "done", "deps": [], "attempts": []},
                {"id": "DEV", "title": "dev", "kind": "impl", "project": "CORE",
                 "state": "queued", "deps": ["PLAN"], "attempts": []}
            ]
        }))
        .unwrap();

        assert_eq!(
            app.selectable_keys(),
            ["project:ROOT", "project:CORE", "PLAN", "DEV"]
        );
        app.open_or_zoom("project:ROOT".into());
        assert_eq!(app.zoom, ["project:ROOT"]);
        app.selected = Some("project:ROOT".into());
        app.fold_or_up();
        assert!(app.zoom.is_empty(), "left at the zoom root zooms out first");
        app.fold_or_up();
        assert!(app.folded.contains("project:ROOT"));
        app.open_or_zoom("project:ROOT".into());
        assert!(!app.folded.contains("project:ROOT"));

        app.selected = Some("DEV".into());
        app.fold_or_up();
        assert_eq!(app.selected.as_deref(), Some("PLAN"));
        app.selected = Some("project:CORE".into());
        assert!(app.task_target().is_none(), "project rows cannot receive task messages");

        app.loaded.doc = serde_json::from_value(serde_json::json!({
            "tasks": [
                {"id": "DUP", "attempts": []},
                {"id": "DUP", "attempts": []}
            ]
        }))
        .unwrap();
        app.selected = Some("DUP".into());
        assert!(app.task_target().is_none(), "ambiguous ids fail closed");

        app.loaded.doc = serde_json::from_value(serde_json::json!({
            "tasks": [
                {"id": "A", "attempts": [{"id": "ATT", "n": 1}]},
                {"id": "B", "attempts": [{"id": "ATT", "n": 1}]}
            ]
        }))
        .unwrap();
        app.selected = Some("ATT".into());
        assert!(app.task_target().is_none(), "ambiguous attempt ids fail closed");
        app.selected = Some("A".into());
        assert!(app.task_target().is_none(), "messages cannot inherit an ambiguous attempt");
    }

    #[test]
    fn message_composer_cycles_editable_starters_and_authority_independently() {
        let mut app = message_test_app();
        let draft = message::Draft::from_starter(
            "G1".into(),
            &app.loaded.message_starters,
            0,
        );
        app.message_key(
            draft,
            event::KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        let draft = match std::mem::replace(&mut app.mode, Mode::Normal) {
            Mode::Message { draft } => draft,
            _ => panic!("Tab should keep the composer open"),
        };
        assert_eq!(draft.starter, 1);
        assert_eq!(draft.authority, message::Authority::Recommend);
        assert!(draft.text.contains("independent guidance"));

        app.message_key(
            draft,
            event::KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );
        let draft = match std::mem::replace(&mut app.mode, Mode::Normal) {
            Mode::Message { draft } => draft,
            _ => panic!("authority toggle should keep the composer open"),
        };
        assert_eq!(draft.authority, message::Authority::Decide);

        app.message_key(
            draft,
            event::KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE),
        );
        let draft = match std::mem::replace(&mut app.mode, Mode::Normal) {
            Mode::Message { draft } => draft,
            _ => panic!("ordinary text should keep the composer open"),
        };
        assert!(draft.text.ends_with('7'), "digits remain free-form text");
        app.message_key(draft, event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Normal));
    }

    #[test]
    fn reload_cancels_a_message_when_its_unique_target_becomes_duplicate() {
        let dir = std::env::temp_dir().join(format!(
            "dagr-message-reload-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.json");
        let unique = r#"{
            "dagr":3,
            "run":{"id":"r","orchestrator":{"pane":"wX:p1"}},
            "generated_at":"2026-08-20T01:02:03Z",
            "tasks":[{"id":"A","state":"queued","deps":[],"attempts":[]}]
        }"#;
        let duplicate = r#"{
            "dagr":3,
            "run":{"id":"r","orchestrator":{"pane":"wX:p1"}},
            "generated_at":"2026-08-20T01:02:04Z",
            "tasks":[
                {"id":"A","state":"queued","deps":[],"attempts":[]},
                {"id":"A","state":"queued","deps":[],"attempts":[]}
            ]
        }"#;
        std::fs::write(&path, unique).unwrap();

        let mut app = message_test_app();
        app.path = path.to_string_lossy().into_owned();
        app.loaded = load(&app.path).unwrap();
        app.selected = Some("A".into());
        app.start_message();
        assert!(matches!(app.mode, Mode::Message { .. }));

        std::fs::write(&path, duplicate).unwrap();
        app.reload();
        assert!(matches!(app.mode, Mode::Normal));
        assert!(
            app.flash.as_ref().is_some_and(|(text, _)| text.contains("target is no longer unique")),
            "reload should explain why the draft was cancelled: {:?}",
            app.flash
        );
        assert!(!message::journal_path(&path).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tall_doc(criteria: Option<String>) -> Doc {
        let tasks = (0..40)
            .map(|i| {
                serde_json::json!({
                    "id": format!("T{i:02}"),
                    "title": format!("task {i:02}"),
                    "kind": "impl",
                    "owner": "developer",
                    "state": "queued",
                    "deps": [],
                    "criteria": (i == 20).then(|| criteria.clone()).flatten(),
                    "attempts": []
                })
            })
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({
            "dagr": 3,
            "run": {"id": "tall"},
            "tasks": tasks
        }))
        .unwrap()
    }

    fn composed(doc: &Doc, selected: &str, width: usize, prompt: Option<String>) -> render::Frame {
        let scene = model::build(
            doc,
            Some(selected),
            None,
            None,
            &model::ViewOpts::default(),
        );
        render::compose(
            &render::FrameInput {
                doc,
                scene: &scene,
                selected: Some(selected),
                banner: None,
                flash: None,
                stale_min: None,
                watching: false,
                herdr: None,
                prompt,
                messages: &[],
            },
            width,
        )
    }

    fn composed_with(
        doc: &Doc,
        selected: &str,
        width: usize,
        inspector: render::InspectorMode,
    ) -> render::Frame {
        let scene = model::build(
            doc,
            Some(selected),
            None,
            None,
            &model::ViewOpts::default(),
        );
        render::compose_with_inspector(
            &render::FrameInput {
                doc,
                scene: &scene,
                selected: Some(selected),
                banner: None,
                flash: None,
                stale_min: None,
                watching: false,
                herdr: None,
                prompt: None,
                messages: &[],
            },
            width,
            inspector,
        )
    }

    #[test]
    fn viewport_follows_selection_inside_the_graph_region() {
        assert_eq!(viewport_start(32, 23, Some(3), 0), 0);
        assert_eq!(viewport_start(10, 23, Some(3), 0), 0);
        assert_eq!(viewport_start(32, 23, Some(30), 0), 8);
    }

    #[test]
    fn compact_inspector_keeps_selection_geometry_constant() {
        let short = tall_doc(None);
        let long = tall_doc(Some("criterion ".repeat(120)));
        let short = composed_with(&short, "T20", 72, render::InspectorMode::Compact);
        let long = composed_with(&long, "T20", 72, render::InspectorMode::Compact);
        assert_eq!(short.detail_end - short.graph_end, 3);
        assert_eq!(long.detail_end - long.graph_end, 3);

        let short_screen = screen_frame_mode(&short, 20, 72, 0, false, 0);
        let long_screen = screen_frame_mode(&long, 20, 72, 0, false, 0);
        assert_eq!(short_screen.graph_rows, long_screen.graph_rows);
        assert_eq!(short_screen.graph_rows, 15, "20 rows - 3 inspector - 2 footer");
        assert_eq!(short_screen.lines.len(), 20);
        assert_eq!(long_screen.lines.len(), 20);
        assert!(
            long_screen
                .lines
                .last()
                .is_some_and(|line| select::plain(line).contains("d details"))
        );
    }

    #[test]
    fn focus_details_keep_a_fixed_lens_and_scroll_only_the_body() {
        let mut long = tall_doc(Some("criterion ".repeat(120)));
        let tasks = long.tasks.as_mut().expect("fixture tasks");
        tasks[20].deps = vec!["T19".into()];
        tasks[21].deps = vec!["T20".into()];
        let logical = composed_with(&long, "T20", 72, render::InspectorMode::Focus);
        let top = screen_frame_mode(&logical, 20, 72, 0, true, 0);
        let bottom = screen_frame_mode(&logical, 20, 72, 0, true, usize::MAX);
        assert_eq!(top.graph_rows, bottom.graph_rows);
        assert_eq!(top.graph_rows, logical.graph_end, "the six-row lens and run header fit");
        assert!(top.detail_scroll_max > 0, "long detail must be independently scrollable");
        assert_eq!(bottom.detail_scroll, bottom.detail_scroll_max);
        assert!(bottom.detail_scroll > top.detail_scroll);

        let top_plain = top.lines.iter().map(|line| select::plain(line)).collect::<Vec<_>>();
        let bottom_plain = bottom.lines.iter().map(|line| select::plain(line)).collect::<Vec<_>>();
        assert_eq!(
            &top_plain[..top.graph_rows],
            &bottom_plain[..bottom.graph_rows],
            "scrolling details must not move or repaint the graph lens"
        );
        for screen in [&top_plain, &bottom_plain] {
            assert!(screen.iter().any(|line| line.contains("focus") && line.contains("T20")));
            assert!(screen.iter().any(|line| line.contains("details ")));
            assert!(screen.iter().any(|line| line.contains("[m] message orchestrator")));
            assert!(screen.iter().any(|line| line.ends_with('┘')));
        }

        let short = tall_doc(None);
        let short = composed_with(&short, "T20", 72, render::InspectorMode::Focus);
        let short = screen_frame_mode(&short, 20, 72, 0, true, 0);
        assert_eq!(short.graph_rows, top.graph_rows, "detail length cannot move the boundary");

        let tiny = screen_frame_mode(&logical, 13, 40, 0, true, 0);
        let tiny_plain = tiny.lines.iter().map(|line| select::plain(line)).collect::<Vec<_>>();
        assert!(tiny_plain.iter().any(|line| line.contains("inputs") && line.contains("T19")));
        assert!(tiny_plain.iter().any(|line| line.contains("focus") && line.contains("T20")));
        assert!(tiny_plain.iter().any(|line| line.contains("unlocks") && line.contains("T21")));

        let mut app = message_test_app();
        app.loaded.doc = long;
        app.selected = Some("T20".into());
        app.scroll = 17;
        app.toggle_details();
        assert!(app.details_open);
        app.toggle_details();
        assert_eq!(app.selected.as_deref(), Some("T20"));
        assert_eq!(app.scroll, 17, "closing details restores the exact graph context");
    }

    #[test]
    fn long_graph_keeps_selection_detail_and_footer_on_screen() {
        let doc = tall_doc(None);
        for selected in ["T00", "T20", "T39"] {
            let logical = composed(&doc, selected, 120, None);
            assert!(logical.graph_end > 40, "fixture must exceed the viewport");
            let screen = screen_frame(&logical, 20, 120, 0);
            let plain = screen
                .lines
                .iter()
                .map(|line| select::plain(line))
                .collect::<Vec<_>>();
            assert_eq!(plain.len(), 20);
            assert!(
                plain.iter().any(|line| line.contains(&format!("○ {selected}"))),
                "selected graph row disappeared for {selected}:\n{}",
                plain.join("\n")
            );
            assert!(
                plain.iter().any(|line| line.contains(&format!("─ {selected} ·"))),
                "detail heading is not docked for {selected}:\n{}",
                plain.join("\n")
            );
            assert!(plain.iter().any(|line| line.contains("[m] message orchestrator")));
            assert!(plain.last().is_some_and(|line| line.contains("j/k")), "footer is fixed");

            let message_hit = screen
                .hits
                .iter()
                .find(|hit| matches!(hit.target, render::HitTarget::Message))
                .expect("message action remains clickable after regional layout");
            assert!(plain[message_hit.line].contains("[m] message"));
            let row_hit = screen
                .hits
                .iter()
                .find(|hit| matches!(&hit.target, render::HitTarget::Row(key) if key == selected))
                .expect("selected graph row remains clickable after scrolling");
            assert!(plain[row_hit.line].contains(selected));
        }
    }

    #[test]
    fn constrained_height_preserves_context_actions_and_names_omitted_detail() {
        let doc = tall_doc(Some("criterion ".repeat(120)));
        let logical = composed(&doc, "T20", 40, None);
        let screen = screen_frame(&logical, 12, 40, 0);
        let plain = screen
            .lines
            .iter()
            .map(|line| select::plain(line))
            .collect::<Vec<_>>();
        assert_eq!(plain.len(), 12);
        assert!(plain.iter().any(|line| line.contains("○ T20")), "graph context survives");
        assert!(
            plain.iter().any(|line| line.contains("─ T20 ·")),
            "card heading survives:\n{}",
            plain.join("\n")
        );
        assert!(plain.iter().any(|line| line.contains("more detail rows")), "clipping is explicit");
        assert!(plain.iter().any(|line| line.contains("[m] message orchestrator")), "action survives");
        assert!(plain.iter().any(|line| line.ends_with('┘')), "card remains framed");
        assert!(plain.last().is_some_and(|line| line.contains("? help")));
    }

    #[test]
    fn composer_footer_is_fixed_without_displacing_the_selected_row() {
        let doc = tall_doc(None);
        let prompt = "line one\nline two\nline three".to_string();
        let logical = composed(&doc, "T20", 72, Some(prompt));
        let screen = screen_frame(&logical, 18, 72, 0);
        let plain = screen
            .lines
            .iter()
            .map(|line| select::plain(line))
            .collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("○ T20")));
        for line in ["line one", "line two", "line three"] {
            assert!(plain.iter().any(|row| row.contains(line)), "missing prompt {line:?}");
        }
        assert!(plain.last().is_some_and(|line| line.contains("j/k")));
    }
}
