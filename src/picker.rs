//! `f` — open another run file without leaving the pane. A fuzzy picker
//! over (a) the files this pane has opened before (persisted, so sibling
//! panes share the list) and (b) a bounded background scan for contract
//! documents near the current one. The scan runs off-thread: the pane
//! never blocks on a directory walk.

use crate::style::{self, paint, trunc, Style};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::SystemTime;

pub struct Item {
    pub path: PathBuf,
    pub label: String,
    pub mtime: SystemTime,
    pub recent: bool,
}

pub struct State {
    pub query: String,
    pub pos: usize,
    items: Vec<Item>,
    scanning: bool,
    rx: Receiver<Vec<Item>>,
}

impl State {
    pub fn open(base: PathBuf) -> State {
        State {
            query: String::new(),
            pos: 0,
            items: mru_items(&base),
            scanning: true,
            rx: scan(base),
        }
    }

    /// Fold a finished scan in (called once per event-loop tick).
    pub fn poll(&mut self) {
        if !self.scanning {
            return;
        }
        match self.rx.try_recv() {
            Ok(found) => {
                self.scanning = false;
                let have: std::collections::HashSet<PathBuf> =
                    self.items.iter().map(|i| i.path.clone()).collect();
                self.items.extend(found.into_iter().filter(|i| !have.contains(&i.path)));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.scanning = false,
        }
    }

    pub fn filtered(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|&i| fuzzy(&self.items[i].label, &self.query)).collect()
    }

    pub fn current(&self) -> Option<&Item> {
        let f = self.filtered();
        f.get(self.pos.min(f.len().saturating_sub(1))).map(|&i| &self.items[i])
    }

    pub fn step(&mut self, delta: i64) {
        let n = self.filtered().len();
        if n == 0 {
            self.pos = 0;
            return;
        }
        self.pos = (self.pos.min(n - 1) as i64 + delta).clamp(0, n as i64 - 1) as usize;
    }
}

/// Case-insensitive subsequence match — every query char must appear, in
/// order. The empty query matches everything.
pub fn fuzzy(hay: &str, needle: &str) -> bool {
    let mut h = hay.chars().flat_map(char::to_lowercase);
    'q: for n in needle.chars().flat_map(char::to_lowercase) {
        for c in h.by_ref() {
            if c == n {
                continue 'q;
            }
        }
        return false;
    }
    true
}

/// Contract documents open with a `"dagr"` version key — 2KB of head is
/// enough to tell a run file from any other JSON without parsing it.
fn sniff(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut buf = [0u8; 2048];
    let n = f.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    head.contains("\"dagr\"") && (head.contains("\"tasks\"") || head.contains("\"run\""))
}

const SKIP_DIRS: [&str; 7] =
    ["node_modules", "target", "dist", "build", "venv", ".venv", "__pycache__"];

/// Walk `base` for run files on a thread. Bounded on purpose — depth,
/// entries, and result count — this is a picker, not find(1). Newest
/// first: the file someone wants is almost always the one just written.
fn scan(base: PathBuf) -> Receiver<Vec<Item>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut out: Vec<Item> = Vec::new();
        let mut stack = vec![(base.clone(), 0usize)];
        let mut visited = 0usize;
        'walk: while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                visited += 1;
                if visited > 20_000 || out.len() >= 400 {
                    break 'walk;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    // symlinked dirs stay unfollowed: no cycles, no surprises
                    if depth < 5 && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push((e.path(), depth + 1));
                    }
                } else if ft.is_file() && name.ends_with(".json") && sniff(&e.path()) {
                    let mtime =
                        e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    out.push(Item {
                        label: label_for(&e.path(), &base),
                        path: e.path(),
                        mtime,
                        recent: false,
                    });
                }
            }
        }
        out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        let _ = tx.send(out);
    });
    rx
}

fn label_for(p: &Path, base: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).display().to_string()
}

fn mru_file() -> Option<PathBuf> {
    #[cfg(windows)]
    let windows_base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let windows_base: Option<PathBuf> = None;
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or(windows_base)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("dagr/recent"))
}

/// Remember an opened run file (front of the list, deduped, capped).
pub fn mru_add(path: &Path) {
    let Some(f) = mru_file() else { return };
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let abs = abs.to_string_lossy().to_string();
    let mut list: Vec<String> = std::fs::read_to_string(&f)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    list.retain(|l| *l != abs && !l.is_empty());
    list.insert(0, abs);
    list.truncate(20);
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&f, list.join("\n") + "\n");
}

fn mru_items(base: &Path) -> Vec<Item> {
    let Some(f) = mru_file() else { return Vec::new() };
    let Ok(s) = std::fs::read_to_string(&f) else { return Vec::new() };
    s.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let p = PathBuf::from(l);
            let meta = std::fs::metadata(&p).ok()?; // gone files drop out
            Some(Item {
                label: label_for(&p, base),
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                path: p,
                recent: true,
            })
        })
        .collect()
}

/// The picker frame. Returns (lines, selected line) so the viewport can
/// keep the cursor on screen through a long list.
pub fn lines(st: &State, w: usize) -> (Vec<String>, Option<usize>) {
    let mut out = vec![
        paint(" open a run file", Style::bold(style::ACCENT))
            + &if st.scanning {
                paint("  scanning…", Style::dim(style::MUTED))
            } else {
                String::new()
            },
        format!(" › {}▏", st.query),
        String::new(),
    ];
    let filt = st.filtered();
    let pos = st.pos.min(filt.len().saturating_sub(1));
    let mut sel_line = None;
    for (i, &ix) in filt.iter().enumerate() {
        let it = &st.items[ix];
        let here = i == pos;
        if here {
            sel_line = Some(out.len());
        }
        let mark = if here { paint("▸", Style::bold(style::ACCENT)) } else { " ".into() };
        let label = trunc(&it.label, w.saturating_sub(14));
        let ink = if here { Style::fg(style::TEXT) } else { Style::dim(style::TEXT) };
        let mut line = format!(" {mark} {}", paint(&label, ink));
        if it.recent {
            line += &paint("  · recent", Style::dim(style::MUTED));
        }
        out.push(line);
    }
    if filt.is_empty() {
        out.push(paint("   (no run files found — type less, or esc)", Style::dim(style::MUTED)));
    }
    out.push(String::new());
    out.push(paint(" type to filter · ↑/↓ move · enter open · esc cancel", Style::dim(style::MUTED)));
    (out, sel_line)
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_is_a_case_blind_ordered_subsequence() {
        assert!(fuzzy("demos/selfrun/run.json", "selfrun"));
        assert!(fuzzy("demos/selfrun/run.json", "dsr.j"));
        assert!(fuzzy("Samples/Run.JSON", "run.json"));
        assert!(!fuzzy("run.json", "nosj")); // order matters
        assert!(fuzzy("anything", ""));
    }
}
