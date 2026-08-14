//! §9 actions: the pane's only mutating surface, and it mutates nothing
//! itself. A producer-declared argv template is filled, shown to the
//! human as an unambiguous argv array, confirm-gated, and executed; the
//! only rendered result is whatever the producer then writes to the run
//! file. No local state change, ever (CONTRACT non-goals).

use crate::contract::Doc;

/// Pane key → verb (CONTRACT §9).
pub fn verb_for_key(c: char) -> Option<&'static str> {
    match c {
        'u' => Some("unblock"),
        'a' => Some("answer"),
        'o' => Some("accept"),
        'x' => Some("reject"),
        _ => None,
    }
}

/// Deterministic idempotency key over the *complete intent*: document
/// revision + target + verb + operator + the typed text. Same intent →
/// same key (a nervous double-press retries one command, the producer
/// dedupes); a corrected reason or answer is a NEW intent even within
/// the same document generation. Fields are length-prefixed before
/// hashing so no delimiter inside a value can make two different tuples
/// encode identically. FNV-1a 64; no deps.
pub fn idem_key(fields: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for f in fields {
        eat(f.len().to_string().as_bytes());
        eat(b":");
        eat(f.as_bytes());
    }
    format!("dagr-{h:016x}")
}

/// Fill one template element in a single pass: only placeholders present
/// in the ORIGINAL template are substituted, and substituted text is
/// never rescanned — an id or answer containing `{key}` stays literal.
fn fill(template: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        out.push_str(&rest[..open]);
        let ph = &rest[open..open + close + 1];
        match lookup(ph) {
            Some(v) => out.push_str(&v),
            None => out.push_str(ph),
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// A built-but-not-yet-confirmed action. The argv (and the idempotency
/// key inside it) is finalized only once every intent-bearing field —
/// including `{text}` — is known; the confirm gate always shows the
/// exact vector that will run.
#[derive(Debug)]
pub struct Pending {
    pub verb: String,
    task: String,
    attempt: String,
    run_id: String,
    generated: String,
    operator: String,
    tpl: Vec<String>,
    text: Option<String>,
    /// Set by `finalize()`; what `run()` executes and `preview()` shows.
    pub argv: Vec<String>,
}

impl Pending {
    pub fn needs_text(&self) -> bool {
        self.text.is_none() && self.tpl.iter().any(|a| a.contains("{text}"))
    }

    pub fn fill_text(&mut self, text: &str) {
        self.text = Some(text.to_string());
    }

    /// Compute the key over the complete intent and materialize the final
    /// argv. Idempotent; called before the confirm gate is shown.
    pub fn finalize(&mut self) {
        let text = self.text.clone().unwrap_or_default();
        let key = idem_key(&[
            &self.run_id,
            &self.verb,
            &self.task,
            &self.attempt,
            &self.generated,
            &self.operator,
            &text,
        ]);
        self.argv = self
            .tpl
            .iter()
            .map(|a| {
                fill(a, &|ph| match ph {
                    "{task}" => Some(self.task.clone()),
                    "{attempt}" => Some(self.attempt.clone()),
                    "{operator}" => Some(self.operator.clone()),
                    "{text}" => Some(text.clone()),
                    "{key}" => Some(key.clone()),
                    _ => None,
                })
            })
            .collect();
    }

    /// The exact command shown at the confirm gate: the argv as a JSON
    /// array. Injective and fully escaped — two different vectors can
    /// never display identically, and control characters are visible.
    /// serde escapes C0 controls, but bidi overrides and zero-width
    /// scalars pass through as raw UTF-8 — exactly the characters that
    /// make the DISPLAYED argv differ from what runs, planted in data
    /// the human has no reason to distrust. They are
    /// escaped here as `\u{XXXX}`; a literal backslash in the argv is
    /// itself JSON-escaped to `\\`, so the escape stays injective.
    pub fn preview(&self) -> String {
        let json = serde_json::to_string(&self.argv).unwrap_or_default();
        let mut out = String::with_capacity(json.len());
        for c in json.chars() {
            if is_invisible(c) {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32));
            } else {
                out.push(c);
            }
        }
        out
    }
}

/// Scalars that render as nothing (or reorder what renders): bidi
/// controls, zero-width joiners/spaces, soft hyphen, BOM — plus anything
/// non-ASCII that terminals give zero columns (combining marks).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    ) || (c as u32 >= 0x80 && unicode_width::UnicodeWidthChar::width(c) == Some(0))
}

pub fn operator() -> String {
    std::env::var("DAGR_OPERATOR")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "operator".into())
}

/// Build the pending action for a verb against the current selection.
pub fn build(doc: &Doc, verb: &str, task_id: &str, attempt_id: &str) -> Result<Pending, String> {
    let actions = doc
        .actions
        .as_ref()
        .ok_or_else(|| "producer declares no actions".to_string())?;
    let tpl = actions
        .get(verb)
        .ok_or_else(|| format!("producer declares no {verb:?} action"))?;
    let argv_tpl = tpl
        .argv_strings()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{verb:?} action has no usable argv (E190)"))?;
    // the key hashes the document revision; without one every repetition
    // of this intent keys identically forever and a producer's dedupe
    // silently swallows it
    if doc.generated_at.is_none() {
        return Err("document has no generated_at — actions need a revision to key intents".into());
    }
    // a template that names {attempt} needs one; asking the human to
    // authorise a command that is guaranteed to be refused wastes the
    // confirmation
    if attempt_id.is_empty() && argv_tpl.iter().any(|a| a.contains("{attempt}")) {
        return Err(format!("no attempt to {verb} — this task has none yet"));
    }
    let mut p = Pending {
        verb: verb.to_string(),
        task: task_id.to_string(),
        attempt: attempt_id.to_string(),
        run_id: doc
            .run
            .as_ref()
            .and_then(|r| r.id.clone())
            .unwrap_or_default(),
        generated: doc.generated_at.clone().unwrap_or_default(),
        operator: operator(),
        tpl: argv_tpl,
        text: None,
        argv: Vec::new(),
    };
    if !p.needs_text() {
        p.finalize();
    }
    Ok(p)
}

/// Total wall-clock budget for a producer CLI call, capture included.
/// Short under test so the kill path is exercisable.
const RUN_BUDGET: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_secs(2)
} else {
    std::time::Duration::from_secs(8)
};
/// Most output we keep per stream.
const CAPTURE_CAP: usize = 64 * 1024;

/// Drain one pipe on its own thread (a pipe left undrained can block the
/// child; a descendant holding the pipe can block US, so the reader is
/// detached and reports through a channel instead of being joined).
fn drain(
    stream: Option<impl std::io::Read + Send + 'static>,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut s) = stream {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = Vec::new();
            let _ = (&mut s).take(CAPTURE_CAP as u64).read_to_end(&mut buf);
            // keep consuming (discarding) so the writer never blocks
            let mut sink = [0u8; 8192];
            while matches!(s.read(&mut sink), Ok(n) if n > 0) {}
            let _ = tx.send(buf);
        });
    }
    rx
}

/// Run the confirmed argv with a hard total budget. The child gets its
/// own process group so a timeout kill reaps helpers too; stdout/stderr
/// are drained concurrently (a chatty producer must not deadlock on pipe
/// capacity, and a backgrounded descendant must not wedge the capture).
/// The success message claims only what is known: the producer exited 0.
/// Whether the action APPLIED is readable solely from the producer's
/// next write, which the pane renders when it lands.
pub fn run(pending: &Pending) -> String {
    use std::process::{Command, Stdio};
    if pending.argv.is_empty() {
        return format!("{}: not finalized", pending.verb);
    }
    let mut cmd = Command::new(&pending.argv[0]);
    cmd.args(&pending.argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return format!("{}: spawn failed: {e}", pending.verb),
    };
    let pid = child.id();
    let out_rx = drain(child.stdout.take());
    let err_rx = drain(child.stderr.take());
    let deadline = std::time::Instant::now() + RUN_BUDGET;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    #[cfg(unix)]
                    unsafe {
                        // signal the whole group directly — helpers
                        // included. Resolving a `kill` binary through
                        // PATH can fail in a thin plugin environment,
                        // which is exactly when the group must still
                        // die.
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return format!(
                        "{}: producer CLI hung >{}s — killed",
                        pending.verb,
                        RUN_BUDGET.as_secs()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return format!("{}: wait failed: {e}", pending.verb),
        }
    };

    // Capture within the remaining budget (recomputed per stream — the
    // waits share one deadline, they don't each get their own); a
    // descendant that inherited a pipe cannot hold the pane hostage.
    let take = |rx: std::sync::mpsc::Receiver<Vec<u8>>| {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        rx.recv_timeout(left.max(std::time::Duration::from_millis(100)))
            .unwrap_or_default()
    };
    let (out, err) = (take(out_rx), take(err_rx));
    // success: first line of stdout (the producer's own summary line).
    // failure: LAST non-empty line of stderr — an interpreter puts the
    // message at the end of a traceback, and the first line of one is
    // always the useless "Traceback (most recent call last):" (M4 F11).
    let pick = |bytes: &[u8], last: bool| -> String {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if last { lines.next_back() } else { lines.next() }.unwrap_or("").to_string()
    };
    let tail = if status.success() { pick(&out, false) } else { pick(&err, true) };
    if status.success() {
        if tail.is_empty() {
            format!("{}: producer exited 0 — awaiting its write", pending.verb)
        } else {
            format!("{}: producer exited 0 — {tail} — awaiting its write", pending.verb)
        }
    } else {
        format!("{} failed ({status}): {tail}", pending.verb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(actions: &str) -> Doc {
        serde_json::from_str(&format!(
            r#"{{"run": {{"id": "r1"}}, "generated_at": "2026-01-01T10:00:00Z", "actions": {actions}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn idem_key_covers_the_whole_intent() {
        let base = ["r1", "reject", "T1", "T1·a2", "g1", "op", "reason A"];
        assert_eq!(idem_key(&base), idem_key(&base), "deterministic");
        for i in 0..base.len() {
            let mut m = base;
            m[i] = "different";
            assert_ne!(idem_key(&base), idem_key(&m), "field {i} must matter");
        }
        // the F1 scenario: same generation, corrected reason → new intent
        let mut fixed = base;
        fixed[6] = "reason B (corrected)";
        assert_ne!(idem_key(&base), idem_key(&fixed));
    }

    #[test]
    fn idem_key_encoding_has_no_delimiter_collisions() {
        assert_ne!(
            idem_key(&["T\u{1f}A", "B"]),
            idem_key(&["T", "A\u{1f}B"]),
            "length-prefixing must separate structurally"
        );
        assert_ne!(idem_key(&["ab", ""]), idem_key(&["a", "b"]));
    }

    #[test]
    fn fill_is_single_pass_and_original_tokens_only() {
        let lookup = |ph: &str| match ph {
            "{task}" => Some("T{attempt}".to_string()),
            "{attempt}" => Some("A1".to_string()),
            _ => None,
        };
        // substituted text containing template syntax stays literal
        assert_eq!(fill("{task}", &lookup), "T{attempt}");
        assert_eq!(fill("x-{attempt}-y", &lookup), "x-A1-y");
        // unknown tokens pass through untouched
        assert_eq!(fill("{nope}", &lookup), "{nope}");
    }

    #[test]
    fn build_finalizes_textless_actions_and_defers_texted_ones() {
        let d = doc(
            r#"{"accept": {"argv": ["prod", "accept", "{task}", "--key", "{key}"]},
                "reject": {"argv": ["prod", "reject", "{task}", "--reason", "{text}", "--key", "{key}"]}}"#,
        );
        let a = build(&d, "accept", "T1", "T1·a1").unwrap();
        assert!(!a.needs_text());
        assert_eq!(a.argv[2], "T1");
        assert!(a.argv[4].starts_with("dagr-"));

        let mut r = build(&d, "reject", "T1", "T1·a1").unwrap();
        assert!(r.needs_text());
        assert!(r.argv.is_empty(), "no argv before the intent is complete");
        r.fill_text("error paths untested");
        r.finalize();
        assert_eq!(r.argv[4], "error paths untested");
        let key_a = r.argv[6].clone();
        r.fill_text("different reason");
        r.finalize();
        assert_ne!(r.argv[6], key_a, "text is part of the intent → new key");
    }

    #[test]
    fn preview_is_injective_and_escaped() {
        let mut p1 = Pending {
            verb: "x".into(), task: String::new(), attempt: String::new(),
            run_id: String::new(), generated: String::new(), operator: String::new(),
            tpl: vec![], text: None,
            argv: vec!["tool".into(), "safe looking".into()],
        };
        let p2 = Pending {
            argv: vec!["tool".into(), "\"safe".into(), "looking\"".into()],
            ..Pending {
                verb: "x".into(), task: String::new(), attempt: String::new(),
                run_id: String::new(), generated: String::new(), operator: String::new(),
                tpl: vec![], text: None, argv: vec![],
            }
        };
        assert_ne!(p1.preview(), p2.preview());
        p1.argv = vec!["t".into(), "a\tb\u{7}".into()];
        assert!(p1.preview().contains("\\t"), "controls are escaped visibly");
    }

    #[test]
    fn build_refuses_missing_or_empty_templates() {
        let d = doc(r#"{"accept": {"argv": []}}"#);
        assert!(build(&d, "unblock", "T", "A").is_err());
        assert!(build(&d, "accept", "T", "A").is_err());
    }

    #[test]
    fn build_refuses_a_template_with_any_non_string_element() {
        // a malformed template must fail, never be silently repaired
        // into a shorter argv where flags eat the wrong values (M4 F7)
        let d = doc(r#"{"reject": {"argv": ["prod", "reject", "{task}", "--rounds", 3, "--key", "{key}"]}}"#);
        assert!(build(&d, "reject", "T1", "T1·a1").is_err());
    }

    #[test]
    fn build_refuses_a_document_without_generated_at() {
        // without a revision in the tuple, every repetition of an intent
        // keys identically forever and dedupe swallows it (M4 F10)
        let d: Doc = serde_json::from_str(
            r#"{"run": {"id": "r1"}, "actions": {"accept": {"argv": ["prod", "accept", "{task}", "--key", "{key}"]}}}"#,
        )
        .unwrap();
        let e = build(&d, "accept", "T1", "T1·a1").unwrap_err();
        assert!(e.contains("generated_at"), "{e}");
    }

    #[test]
    fn build_refuses_an_attempt_template_without_an_attempt() {
        // asking the human to authorise a command that is guaranteed to
        // be refused wastes the confirmation (M4 F14)
        let d = doc(r#"{"accept": {"argv": ["prod", "accept", "{task}", "--attempt", "{attempt}", "--key", "{key}"]}}"#);
        let e = build(&d, "accept", "B1", "").unwrap_err();
        assert!(e.contains("no attempt"), "{e}");
    }

    #[test]
    fn preview_escapes_bidi_and_zero_width_scalars() {
        // the argv display IS the trust boundary: a task id carrying a
        // right-to-left override must not display differently from what
        // runs (M4 F9)
        let mut p = Pending {
            verb: "x".into(), task: String::new(), attempt: String::new(),
            run_id: String::new(), generated: String::new(), operator: String::new(),
            tpl: vec![], text: None,
            argv: vec!["prod".into(), "T1\u{202e}dlrow--\u{200b}".into()],
        };
        let pv = p.preview();
        assert!(!pv.contains('\u{202e}'), "raw RLO must not reach the terminal: {pv}");
        assert!(!pv.contains('\u{200b}'), "raw ZWSP must not reach the terminal: {pv}");
        assert!(pv.contains("\\u{202e}") && pv.contains("\\u{200b}"), "escaped visibly: {pv}");
        // injective against a literal backslash-u string (serde escapes
        // the backslash, so the two spellings cannot collide)
        p.argv = vec!["prod".into(), "T1\\u{202e}dlrow--\\u{200b}".into()];
        assert_ne!(pv, p.preview());
    }

    #[test]
    fn run_failure_flash_is_the_last_stderr_line_not_the_traceback_header() {
        let p = Pending {
            verb: "test".into(), task: String::new(), attempt: String::new(),
            run_id: String::new(), generated: String::new(), operator: String::new(),
            tpl: vec![], text: None,
            argv: vec![
                "/bin/sh".into(), "-c".into(),
                "echo 'Traceback (most recent call last):' >&2; echo '  File x, line 1' >&2; echo 'IndexError: list index out of range' >&2; exit 1".into(),
            ],
        };
        let msg = run(&p);
        assert!(msg.contains("IndexError"), "the message line, not the header: {msg}");
        assert!(!msg.contains("Traceback"), "{msg}");
    }

    #[test]
    fn run_kills_a_hung_producer_group_at_the_deadline() {
        // the whole 8s safety mechanism, kill included, must actually
        // fire (M4 F16); RUN_BUDGET is 2s under cfg(test)
        let p = Pending {
            verb: "test".into(), task: String::new(), attempt: String::new(),
            run_id: String::new(), generated: String::new(), operator: String::new(),
            tpl: vec![], text: None,
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
        };
        let t0 = std::time::Instant::now();
        let msg = run(&p);
        assert!(msg.contains("killed"), "{msg}");
        assert!(
            t0.elapsed() < RUN_BUDGET + std::time::Duration::from_secs(2),
            "took {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn run_survives_a_chatty_producer_and_a_pipe_holding_descendant() {
        // chatty: >64KiB on stdout must neither block nor fail
        let mut p = Pending {
            verb: "test".into(), task: String::new(), attempt: String::new(),
            run_id: String::new(), generated: String::new(), operator: String::new(),
            tpl: vec![], text: None,
            argv: vec![
                "/bin/sh".into(), "-c".into(),
                "i=0; while [ $i -lt 3000 ]; do echo 'a long line of producer output padding padding'; i=$((i+1)); done".into(),
            ],
        };
        let msg = run(&p);
        assert!(msg.contains("exited 0"), "chatty producer: {msg}");

        // descendant keeps stdout open after the parent exits: must return
        // promptly (bounded by budget), not hang on EOF
        p.argv = vec![
            "/bin/sh".into(), "-c".into(),
            "sleep 30 & echo done".into(),
        ];
        let t0 = std::time::Instant::now();
        let msg = run(&p);
        assert!(t0.elapsed() < RUN_BUDGET + std::time::Duration::from_secs(2), "took {:?}", t0.elapsed());
        assert!(msg.contains("exited 0"), "descendant case: {msg}");
    }
}
