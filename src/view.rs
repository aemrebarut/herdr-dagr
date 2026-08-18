//! `dagr view` — the pane. Interactive by default (crossterm raw mode,
//! alternate screen, watch-on-mtime); `--snapshot` prints one frame to
//! stdout for demos, CI capture, and eyeballing against the Python
//! reference. The renderer never crashes on bad input: contract errors
//! become a banner, and the last good scene stays up.

use crate::{action, check, contract::Doc, herdr, model, picker, render, select, stats, style};
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
    /// E19x findings against action templates, as (code, path). The
    /// banner alone tells the operator "something is wrong somewhere";
    /// `start_action` uses these to refuse the specific broken template
    /// instead of running a repaired-or-different command (M4 F7).
    action_findings: Vec<(String, String)>,
}

fn load(path: &str) -> Result<Loaded, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc: Doc =
        serde_json::from_str(&raw).map_err(|e| format!("not a contract document: {e}"))?;
    let report = check::check(&doc);
    let banner = match report.errors() {
        0 => None,
        n => Some(format!(
            "{n} contract error{} — run `dagr check {path}`; drawing what parses",
            if n == 1 { "" } else { "s" }
        )),
    };
    let action_findings = report
        .findings
        .iter()
        .filter(|f| f.code.starts_with("E19"))
        .map(|f| (f.code.to_string(), f.path.clone()))
        .collect();
    let generated_min = doc.generated_at.as_deref().and_then(model::parse_min);
    let chip = stats::header_chip(&doc);
    Ok(Loaded { doc, banner, generated_min, chip, action_findings })
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

/// Modal state for §9 actions: Normal → (Input →) Confirm → Normal.
/// Nothing runs until the human sees the exact argv and confirms.
enum Mode {
    Normal,
    Input { pending: action::Pending, buf: String },
    Confirm { pending: action::Pending, since: std::time::Instant },
    /// `f` — the run-file picker (its own frame, like help).
    Picker(picker::State),
    /// `/` — incremental row search; the query lives on the bottom line.
    Search { buf: String },
}

/// How long the confirm gate must have been on screen before `y`
/// counts. Keys already sitting in the terminal's input queue when the
/// gate appears (a paste tail, a reflex) arrive within milliseconds of
/// each other; a read-and-decide does not.
const CONFIRM_ARM: Duration = Duration::from_millis(400);

/// What a modal keypress asks the event loop to do. Pure data: the loop
/// performs it, tests assert it.
enum ModalStep {
    Stay(Mode),
    /// → Normal, with a flash.
    Cancel(&'static str),
    /// Intent complete and finalized: show the confirm gate.
    ToConfirm(action::Pending),
    /// The human confirmed a visible, armed gate: run it.
    Execute(action::Pending),
}

/// The modal transition for one key event. `armed` = the gate has been
/// up at least CONFIRM_ARM; `seen` = its lines were inside the viewport
/// on the last draw. `y` executes only when both hold (F1, F3).
fn modal_step(
    mode: Mode,
    code: event::KeyCode,
    mods: event::KeyModifiers,
    armed: bool,
    seen: bool,
) -> ModalStep {
    use event::KeyCode::*;
    let ctrl = mods.contains(event::KeyModifiers::CONTROL);
    let plain = !mods.intersects(event::KeyModifiers::CONTROL | event::KeyModifiers::ALT);
    match mode {
        Mode::Normal => ModalStep::Stay(Mode::Normal),
        Mode::Input { mut pending, mut buf } => match code {
            Esc => ModalStep::Cancel("cancelled"),
            Char('c') if ctrl => ModalStep::Cancel("cancelled"),
            Enter => {
                pending.fill_text(&buf);
                // the key hashes the typed text, so the argv can only be
                // finalized now — and the confirm gate shows exactly it
                pending.finalize();
                ModalStep::ToConfirm(pending)
            }
            Backspace => {
                buf.pop();
                ModalStep::Stay(Mode::Input { pending, buf })
            }
            // line editing: the reflexes must edit, not type literal
            // letters into the reason
            Char('u') if ctrl => {
                buf.clear();
                ModalStep::Stay(Mode::Input { pending, buf })
            }
            Char('w') if ctrl => {
                while buf.chars().last().is_some_and(|c| c == ' ') {
                    buf.pop();
                }
                while buf.chars().last().is_some_and(|c| c != ' ') {
                    buf.pop();
                }
                ModalStep::Stay(Mode::Input { pending, buf })
            }
            Char(c) if plain => {
                buf.push(c);
                ModalStep::Stay(Mode::Input { pending, buf })
            }
            _ => ModalStep::Stay(Mode::Input { pending, buf }),
        },
        Mode::Confirm { pending, since } => match code {
            // 'y' ONLY — exactly what the prompt advertises. Enter must
            // not run: a reflexive second Enter straight out of the text
            // prompt would be an undisclosed execution path. And even
            // 'y' is inert until the gate has actually been on screen
            // long enough to have been read (F1, F3).
            Char('y') if plain && armed && seen => ModalStep::Execute(pending),
            Esc | Char('n') => ModalStep::Cancel("cancelled"),
            Char('c') if ctrl => ModalStep::Cancel("cancelled"),
            _ => ModalStep::Stay(Mode::Confirm { pending, since }),
        },
        // picker and search keys are handled by the event loop directly
        m => ModalStep::Stay(m),
    }
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
    /// Whether the modal prompt's lines were inside the viewport on the
    /// last draw. `y` is inert until this was true.
    gate_seen: bool,
    /// In-flight focus result (focus runs off-thread; even a bounded CLI
    /// wait has no business freezing the render loop).
    bg_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// Clickable regions of the last drawn frame, plus the viewport the
    /// frame was drawn through — a click is (column, terminal row) and
    /// only means something relative to that exact draw.
    hits: Vec<render::Hit>,
    view_start: usize,
    view_rows: usize,
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
        let line = self.view_start + row;
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
            let tasks = self.loaded.doc.tasks.as_deref()?;
            tasks
                .iter()
                .flat_map(|t| t.attempts.iter())
                .find(|a| a.id.as_deref() == Some(k))
                .and_then(|a| a.locator.as_ref())
                .map(|l| (l.pane.clone(), l.agent.clone()))
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
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.bg_rx = None;
            }
        }
    }

    /// Selection → (task id, attempt id) for action placeholders.
    fn action_target(&self) -> Option<(String, String)> {
        let sel = self.selected.as_deref()?;
        let tasks = self.loaded.doc.tasks.as_deref()?;
        for t in tasks {
            if t.attempts.iter().any(|a| a.id.as_deref() == Some(sel)) {
                return Some((t.id.clone().unwrap_or_default(), sel.to_string()));
            }
        }
        tasks.iter().find(|t| t.id.as_deref() == Some(sel)).map(|t| {
            let latest = t
                .attempts
                .iter()
                .max_by_key(|a| a.n.unwrap_or(0))
                .and_then(|a| a.id.clone())
                .unwrap_or_default();
            (sel.to_string(), latest)
        })
    }

    fn start_action(&mut self, key: char) {
        let Some(verb) = action::verb_for_key(key) else { return };
        // a template the validator flagged at load must not build: the
        // banner said "N contract errors" but not WHERE; running a
        // broken template anyway would silently exceed it (M4 F7)
        let vp = format!("actions.{verb}");
        if let Some((code, _)) = self
            .loaded
            .action_findings
            .iter()
            .find(|(_, path)| *path == vp || path.starts_with(&format!("{vp}.")))
        {
            self.flash =
                Some((format!("{verb} template is invalid ({code}) — run dagr check"), 12));
            return;
        }
        let Some((task_id, attempt_id)) = self.action_target() else {
            self.flash = Some(("nothing selected".into(), 8));
            return;
        };
        match action::build(&self.loaded.doc, verb, &task_id, &attempt_id) {
            Err(e) => self.flash = Some((e, 12)),
            Ok(pending) => {
                if pending.needs_text() {
                    self.mode = Mode::Input { pending, buf: String::new() };
                } else {
                    self.enter_confirm(pending);
                }
            }
        }
    }

    /// Show the confirm gate. Everything already sitting in the
    /// terminal's input queue is drained first: keys queued before the
    /// gate existed cannot be a response to it.
    fn enter_confirm(&mut self, pending: action::Pending) {
        while event::poll(Duration::ZERO).unwrap_or(false) {
            let _ = event::read();
        }
        self.gate_seen = false; // proven by the next draw
        self.mode = Mode::Confirm { pending, since: std::time::Instant::now() };
    }

    /// The only place anything runs — after the human saw the exact
    /// argv on screen and confirmed it. Runs off the UI thread.
    fn execute(&mut self, pending: action::Pending) {
        if self.bg_rx.is_some() {
            // keep the confirmed intent — dropping it silently and
            // blaming "another action" (there may be none — the guard
            // is shared with [enter] focus) forces a full retype with
            // no explanation
            self.flash = Some((
                "waiting on an earlier command — press y again in a moment".into(),
                12,
            ));
            self.mode = Mode::Confirm { pending, since: std::time::Instant::now() };
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let verb = pending.verb.clone();
        std::thread::spawn(move || {
            let _ = tx.send(action::run(&pending));
        });
        self.bg_rx = Some(rx);
        self.flash = Some((format!("running {verb}…"), 30));
    }

    /// The modal prompt line, if any (drawn above the footer).
    fn prompt_line(&self) -> Option<String> {
        match &self.mode {
            Mode::Normal => None,
            Mode::Input { pending, buf } => Some(format!(
                "{} · text ({}▏) — enter to continue · esc to cancel",
                pending.verb, buf
            )),
            Mode::Confirm { pending, .. } => {
                Some(format!("run? {} — [y] run · [esc] cancel", pending.preview()))
            }
            // the picker draws its own frame; search lives on the bottom line
            Mode::Picker(_) | Mode::Search { .. } => None,
        }
    }

    fn reload(&mut self) {
        // the frame is about to be rebuilt from a new revision; a
        // highlight anchored to the old lines would frame the wrong text
        self.sel = None;
        // an open ACTION modal points at the document it was built from;
        // carrying the intent silently across a revision can apply it to
        // state the human never saw. The picker and search reference no
        // document state — a producer rewriting the file every few
        // seconds must not keep killing them.
        if matches!(self.mode, Mode::Input { .. } | Mode::Confirm { .. }) {
            self.mode = Mode::Normal;
            self.flash = Some((
                "run file changed — action cancelled; re-select and re-confirm".into(),
                20,
            ));
        }
        match load(&self.path) {
            Ok(l) => {
                self.loaded = l;
                // view state that names rows survives only as long as the
                // rows do
                self.zoom.retain(|k| model::key_exists(&self.loaded.doc, k));
                self.folded.retain(|k| model::key_exists(&self.loaded.doc, k));
                self.snap_selection();
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
        action_findings: Vec::new(),
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
        gate_seen: false,
        bg_rx: None,
        hits: Vec::new(),
        view_start: 0,
        view_rows: 0,
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
                        m @ (Mode::Input { .. } | Mode::Confirm { .. }) => {
                            let armed = match &m {
                                Mode::Confirm { since, .. } => since.elapsed() >= CONFIRM_ARM,
                                _ => false,
                            };
                            match modal_step(m, k.code, k.modifiers, armed, app.gate_seen) {
                                ModalStep::Stay(m) => app.mode = m,
                                ModalStep::Cancel(msg) => app.flash = Some((msg.into(), 6)),
                                ModalStep::ToConfirm(p) => app.enter_confirm(p),
                                ModalStep::Execute(p) => app.execute(p),
                            }
                            continue;
                        }
                        Mode::Picker(st) => {
                            app.picker_key(st, k);
                            continue;
                        }
                        Mode::Search { buf } => {
                            app.search_key(buf, k);
                            continue;
                        }
                        Mode::Normal => {}
                    }
                    let ctrl = k.modifiers.contains(event::KeyModifiers::CONTROL);
                    if k.code == Char('c') && ctrl {
                        return Ok(());
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
                        // half-page jumps outrank the action keys: ctrl-u
                        // must page, not open the unblock prompt
                        Char('d') if ctrl => app.move_sel((app.view_rows / 2).max(1) as i64),
                        Char('u') if ctrl => app.move_sel(-((app.view_rows / 2).max(1) as i64)),
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
                        Char(c @ ('u' | 'a' | 'o' | 'x')) => app.start_action(c),
                        _ => {}
                    }
                }
                event::Event::Paste(s) => {
                    // a paste is text for the text prompt and NOTHING
                    // else: never a stream of keys a confirm gate could
                    // mistake for a decision. Newlines
                    // become spaces — one paste, one line, no hidden
                    // Enter riding along.
                    if let Mode::Input { buf, .. } = &mut app.mode {
                        buf.extend(
                            s.chars()
                                .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
                                .filter(|c| !c.is_control()),
                        );
                    }
                }
                event::Event::Mouse(m) => {
                    use event::{MouseButton, MouseEventKind};
                    let (col, row) = (m.column as usize, m.row as usize);
                    let normal = matches!(app.mode, Mode::Normal);
                    match m.kind {
                        // press → drag → release IS the selection, and it
                        // runs in every mode: copying the argv out of a
                        // confirm gate is reading, not answering. What the
                        // mouse still cannot do in a modal is act.
                        MouseEventKind::Down(MouseButton::Left) => {
                            app.sel = (row < app.view_rows)
                                .then(|| select::Sel::new(app.view_start + row, col));
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(sel) = app.sel.as_mut() {
                                sel.to(app.view_start + row.min(app.view_rows.saturating_sub(1)), col);
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
                                    let line = app.view_start + row;
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
                            app.move_sel(1);
                        }
                        MouseEventKind::ScrollUp if normal && !app.help => {
                            app.sel = None;
                            app.move_sel(-1);
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
        // watch: reload when the producer rewrites the file
        if let Ok(m) = std::fs::metadata(&app.path).and_then(|m| m.modified()) {
            if last_mtime.map(|t| m > t).unwrap_or(true) {
                last_mtime = Some(m);
                app.reload();
            }
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

/// Viewport start for a frame. Normal mode scrolls the minimum needed
/// to keep the selected row on screen; while a modal gate is open the
/// frame TAIL is pinned instead — the prompt lives there, and a confirm
/// gate rendered below the fold reads as a hung pane while `y` still
/// executes.
fn viewport_start(
    frame_len: usize,
    visible: usize,
    sel_line: Option<usize>,
    scroll: usize,
    modal: bool,
) -> usize {
    if modal {
        return frame_len.saturating_sub(visible);
    }
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

fn draw(app: &mut App, stdout: &mut std::io::Stdout) -> Result<(), String> {
    let (w, h) = terminal::size().map_err(|e| e.to_string())?;
    let (w, h) = (w as usize, h as usize);
    // only the ACTION modals pin the frame tail (the confirm gate lives
    // there); the picker draws its own frame and search follows the cursor
    let modal = matches!(app.mode, Mode::Input { .. } | Mode::Confirm { .. });
    let (lines, sel_line, prompt_line) = if app.help {
        app.hits = Vec::new();
        (render::help_lines(), None, None)
    } else if let Mode::Picker(st) = &app.mode {
        app.hits = Vec::new();
        let (lines, sel) = picker::lines(st, w);
        (lines, sel, None)
    } else {
        let hints = app.hints();
        let scene = model::build(
            &app.loaded.doc,
            app.selected.as_deref(),
            hints.as_ref(),
            app.loaded.chip.as_deref(),
            &app.view_opts(),
        );
        let frame = render::compose(
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
            },
            w,
        );
        app.hits = frame.hits;
        (frame.lines, frame.sel_line, frame.prompt_line)
    };
    let visible = h.saturating_sub(1);
    let start = viewport_start(lines.len(), visible, sel_line, app.scroll, modal);
    app.view_start = start;
    app.view_rows = visible;
    if !modal {
        // the modal tail-pin is transient; the browsing scroll position
        // survives the modal and is restored on cancel
        app.scroll = start;
    }
    // the gate counts as SHOWN only if the whole prompt block (its first
    // line through the frame tail) is inside the viewport right now;
    // `y` stays inert until a draw proved that
    app.gate_seen =
        modal && prompt_line.is_some_and(|p| p >= start && lines.len() <= start + visible);
    app.frame = lines;
    // Paint into ONE buffer and write it in ONE syscall. The old path
    // flushed a full-screen Clear on its own and then flushed again per
    // line, so the terminal had a genuinely blank screen to draw between
    // them — that is the flicker. Each row now ends with a clear-to-EOL
    // instead, which erases the old content of that row in the same write
    // that draws the new one.
    let mut buf: Vec<u8> = Vec::with_capacity(w * visible.max(1) + 64);
    let mut drawn = 0usize;
    for (i, idx) in (start..app.frame.len()).take(visible.max(1)).enumerate() {
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

    fn pending_reject() -> action::Pending {
        let doc: Doc = serde_json::from_str(
            r#"{"run": {"id": "r"}, "generated_at": "2026-01-01T10:00:00Z",
                "actions": {"reject": {"argv": ["prod", "reject", "{task}", "--reason", "{text}", "--key", "{key}"]}}}"#,
        )
        .unwrap();
        action::build(&doc, "reject", "T1", "T1·a1").unwrap()
    }

    fn confirm(p: action::Pending) -> Mode {
        Mode::Confirm { pending: p, since: std::time::Instant::now() }
    }

    fn input(buf: &str) -> Mode {
        Mode::Input { pending: pending_reject(), buf: buf.into() }
    }

    fn step_stay(m: Mode, code: KeyCode, mods: KeyModifiers) -> Mode {
        match modal_step(m, code, mods, false, false) {
            ModalStep::Stay(m) => m,
            _ => panic!("expected Stay"),
        }
    }

    fn buf_of(m: &Mode) -> &str {
        match m {
            Mode::Input { buf, .. } => buf,
            _ => panic!("expected Input"),
        }
    }

    // ── F1: the viewport must never draw a gate below the fold ──────

    #[test]
    fn viewport_pins_the_frame_tail_while_a_gate_is_open() {
        // the review's repro numbers: 32-line frame, selection at 3
        assert_eq!(viewport_start(32, 23, Some(3), 0, false), 0, "normal: follow selection");
        assert_eq!(viewport_start(32, 23, Some(3), 0, true), 9, "modal: tail on screen");
        assert_eq!(viewport_start(10, 23, Some(3), 0, true), 0, "short frame: all visible");
        assert_eq!(viewport_start(32, 23, Some(30), 0, false), 8, "normal: follow a deep selection");
    }

    // ── F3: `y` answers the gate, not the keyboard buffer ────────────

    #[test]
    fn confirm_y_is_inert_until_the_gate_was_drawn_and_armed() {
        for (armed, seen) in [(false, false), (true, false), (false, true)] {
            let s = modal_step(
                confirm(pending_reject()),
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                armed,
                seen,
            );
            assert!(
                matches!(s, ModalStep::Stay(Mode::Confirm { .. })),
                "armed={armed} seen={seen}: y must not execute"
            );
        }
        let s =
            modal_step(confirm(pending_reject()), KeyCode::Char('y'), KeyModifiers::NONE, true, true);
        assert!(matches!(s, ModalStep::Execute(_)));
    }

    #[test]
    fn confirm_runs_on_plain_y_only() {
        for (code, mods) in [
            (KeyCode::Enter, KeyModifiers::NONE), // the undisclosed-Enter path
            (KeyCode::Char('Y'), KeyModifiers::SHIFT),
            (KeyCode::Char('j'), KeyModifiers::NONE),
            (KeyCode::Char('y'), KeyModifiers::CONTROL),
        ] {
            let s = modal_step(confirm(pending_reject()), code, mods, true, true);
            assert!(matches!(s, ModalStep::Stay(Mode::Confirm { .. })), "{code:?} must be inert");
        }
        for (code, mods) in [
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('n'), KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let s = modal_step(confirm(pending_reject()), code, mods, true, true);
            assert!(matches!(s, ModalStep::Cancel(_)), "{code:?} must cancel");
        }
    }

    // ── F15: line editing edits; modifiers never type letters ────────

    #[test]
    fn input_mode_edits_finalizes_and_cancels() {
        let m = step_stay(input("error path"), KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(buf_of(&m), "error paths");
        let m = step_stay(m, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(buf_of(&m), "error path");
        let m = step_stay(m, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(buf_of(&m), "error ", "ctrl+w deletes a word, not types a w");
        let m = step_stay(m, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(buf_of(&m), "", "ctrl+u clears, not types a u");

        match modal_step(input("why"), KeyCode::Enter, KeyModifiers::NONE, false, false) {
            ModalStep::ToConfirm(p) => {
                assert!(p.preview().contains("why"), "text is in the finalized argv");
                assert!(p.preview().contains("dagr-"), "key is finalized");
            }
            _ => panic!("Enter must finalize into the confirm gate"),
        }
        assert!(matches!(
            modal_step(input("half-typed"), KeyCode::Esc, KeyModifiers::NONE, false, false),
            ModalStep::Cancel(_)
        ));
        assert!(matches!(
            modal_step(input("half-typed"), KeyCode::Char('c'), KeyModifiers::CONTROL, false, false),
            ModalStep::Cancel(_)
        ));
    }
}
