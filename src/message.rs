//! Operator → orchestrator messages. dagr records and transports intent;
//! it does not interpret it or run a second workflow engine. Prompt starters
//! are editable text, authority is a separate explicit field, and the
//! immutable journal is written before Herdr receives the message.

use crate::contract::Doc;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CONFIG_LIMIT: u64 = 64 * 1024;
pub const MESSAGE_LIMIT: usize = 32 * 1024;
const ACTION_LIMIT: usize = 9;
static MESSAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Authority {
    /// Gather/think/recommend, then return the decision to the user.
    Recommend,
    /// The orchestrator may decide and continue within existing scope.
    Decide,
}

impl Authority {
    pub fn label(self) -> &'static str {
        match self {
            Authority::Recommend => "return to me",
            Authority::Decide => "may decide + continue",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Authority::Recommend => Authority::Decide,
            Authority::Decide => Authority::Recommend,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Starter {
    pub id: String,
    pub label: String,
    pub prompt: String,
    pub authority: Authority,
}

#[derive(Deserialize)]
struct ConfigFile {
    version: Option<u64>,
    #[serde(default = "yes")]
    include_defaults: bool,
    #[serde(default)]
    actions: Vec<Starter>,
}

fn yes() -> bool {
    true
}

pub struct Config {
    pub starters: Vec<Starter>,
    pub path: PathBuf,
    pub warning: Option<String>,
}

pub fn defaults() -> Vec<Starter> {
    vec![
        Starter {
            id: "use-judgment".into(),
            label: "Use judgment".into(),
            prompt: "Use your best judgment on this and continue within the current scope.".into(),
            authority: Authority::Decide,
        },
        Starter {
            id: "get-guidance".into(),
            label: "Get guidance".into(),
            prompt: "Get independent guidance, synthesize it, and return with a recommendation."
                .into(),
            authority: Authority::Recommend,
        },
        Starter {
            id: "snooze".into(),
            label: "Snooze".into(),
            prompt: "Snooze this for now; keep monitoring it and surface it again when it becomes urgent or blocks downstream work."
                .into(),
            authority: Authority::Decide,
        },
    ]
}

pub fn config_path(run_path: &Path) -> PathBuf {
    run_path.parent().unwrap_or_else(|| Path::new(".")).join("actions.json")
}

pub fn journal_path(run_path: &Path) -> PathBuf {
    run_path.parent().unwrap_or_else(|| Path::new(".")).join("messages.jsonl")
}

/// Invalid optional configuration never takes down the run viewer: built-ins
/// remain available and the pane discloses the exact file problem.
pub fn load_config(run_path: &Path) -> Config {
    let path = config_path(run_path);
    let raw = match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Config { starters: defaults(), path, warning: None };
        }
        Err(e) => {
            return Config {
                starters: defaults(),
                path: path.clone(),
                warning: Some(format!("cannot read {}: {e}", path.display())),
            };
        }
        Ok(meta) if meta.len() > CONFIG_LIMIT => {
            return Config {
                starters: defaults(),
                path: path.clone(),
                warning: Some(format!("{} is larger than 64 KiB", path.display())),
            };
        }
        Ok(_) => match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) => {
                return Config {
                    starters: defaults(),
                    path: path.clone(),
                    warning: Some(format!("cannot read {}: {e}", path.display())),
                };
            }
        },
    };
    let parsed: ConfigFile = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            return Config {
                starters: defaults(),
                path: path.clone(),
                warning: Some(format!("invalid {}: {e}", path.display())),
            };
        }
    };
    if parsed.version.is_some_and(|version| version != 1) {
        return Config {
            starters: defaults(),
            path: path.clone(),
            warning: Some(format!(
                "invalid {}: unsupported actions config version (expected 1)",
                path.display()
            )),
        };
    }
    let mut starters = if parsed.include_defaults { defaults() } else { Vec::new() };
    for custom in parsed.actions {
        if custom.id.trim().is_empty()
            || custom.label.trim().is_empty()
            || custom.prompt.trim().is_empty()
            || custom.id.len() > 128
            || custom.label.len() > 80
            || custom.prompt.len() > MESSAGE_LIMIT
            || custom.id.chars().any(char::is_control)
            || custom.label.chars().any(char::is_control)
            || custom
                .prompt
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Config {
                starters: defaults(),
                path: path.clone(),
                warning: Some(format!(
                    "invalid {}: every action needs safe, nonempty id, label, and prompt",
                    path.display()
                )),
            };
        }
        if let Some(slot) = starters.iter_mut().find(|s| s.id == custom.id) {
            *slot = custom;
        } else {
            starters.push(custom);
        }
    }
    if starters.is_empty() {
        return Config {
            starters: defaults(),
            path: path.clone(),
            warning: Some(format!("{} declares no actions; using defaults", path.display())),
        };
    }
    let warning = (starters.len() > ACTION_LIMIT).then(|| {
        format!(
            "{} declares more than {ACTION_LIMIT} actions; only the first {ACTION_LIMIT} are shown",
            path.display()
        )
    });
    starters.truncate(ACTION_LIMIT);
    Config { starters, path, warning }
}

#[derive(Clone, Debug)]
pub struct Draft {
    pub target: String,
    pub text: String,
    pub authority: Authority,
    pub starter: usize,
    base: String,
}

impl Draft {
    pub fn from_starter(target: String, starters: &[Starter], starter: usize) -> Self {
        let idx = starter.min(starters.len().saturating_sub(1));
        let picked = starters.get(idx).cloned().unwrap_or_else(|| defaults()[0].clone());
        let base = picked.prompt.trim().to_string();
        Draft {
            target,
            text: format!("{base}\n"),
            authority: picked.authority,
            starter: idx,
            base,
        }
    }

    /// Change the starter without discarding instructions appended after
    /// its prefill. Editing is tail-oriented in the TUI, so this preserves
    /// the common "pick guidance, add model details, compare another
    /// starter" flow while still allowing Ctrl-U to begin from scratch.
    pub fn switch_to(self, starters: &[Starter], starter: usize) -> Self {
        let extra = if self.text.is_empty() {
            String::new()
        } else if self.base.starts_with(&self.text) {
            // The user only backspaced into the prefill; there is no custom
            // suffix to preserve or duplicate under the next starter.
            String::new()
        } else {
            self.text
                .strip_prefix(&self.base)
                .unwrap_or(&self.text)
                .trim_start_matches('\n')
                .to_string()
        };
        let mut next = Self::from_starter(self.target, starters, starter);
        if !extra.is_empty() {
            next.text.push_str(&extra);
        }
        next
    }
}

#[derive(Clone, Debug)]
enum Destination {
    Pane(String),
    Agent(String),
}

impl Destination {
    fn target(&self) -> &str {
        match self {
            Destination::Pane(s) | Destination::Agent(s) => s,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Destination::Pane(_) => "pane",
            Destination::Agent(_) => "agent",
        }
    }
}

pub struct Submission {
    pub id: String,
    run: String,
    journal: PathBuf,
    destination: Destination,
    envelope: String,
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub id: String,
    pub target: String,
    pub text: String,
    pub authority: Authority,
    pub status: String,
    queued_at_ms: u128,
}

/// Reconstruct current delivery state from the append-only journal. Unknown
/// record kinds are ignored so future receipts remain backward compatible.
pub fn read_summaries(run_path: &Path, run_id: Option<&str>) -> Result<Vec<Summary>, String> {
    let path = journal_path(run_path);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut by_id: std::collections::HashMap<String, Summary> =
        std::collections::HashMap::new();
    for (line_no, line) in std::io::BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| {
            format!("cannot read {} line {}: {e}", path.display(), line_no + 1)
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
            format!("invalid {} line {}: {e}", path.display(), line_no + 1)
        })?;
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if kind == "message_queued" {
            if value.get("run").and_then(|v| v.as_str()) != run_id {
                continue;
            }
            let authority = match value.get("authority").and_then(|v| v.as_str()) {
                Some("decide") => Authority::Decide,
                _ => Authority::Recommend,
            };
            by_id.insert(
                id.to_string(),
                Summary {
                    id: id.to_string(),
                    target: value
                        .get("target")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    text: value
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    authority,
                    status: "recorded".into(),
                    queued_at_ms: value
                        .get("queued_at_ms")
                        .and_then(|v| v.as_u64())
                        .map(u128::from)
                        .unwrap_or(0),
                },
            );
        } else if value.get("run").and_then(|v| v.as_str()) == run_id {
            let Some(summary) = by_id.get_mut(id) else { continue };
            summary.status = match kind {
                "message_delivered" => "delivered",
                "message_delivery_failed" => "delivery failed",
                _ => continue,
            }
            .into();
        }
    }
    let mut out: Vec<Summary> = by_id.into_values().collect();
    out.sort_by(|a, b| (a.queued_at_ms, &a.id).cmp(&(b.queued_at_ms, &b.id)));
    Ok(out)
}

fn millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn fnv64(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for part in parts {
        for b in *part {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn append_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("cannot append {}: {e}", path.display()))?;
    let mut line = serde_json::to_vec(value)
        .map_err(|e| format!("cannot encode {}: {e}", path.display()))?;
    line.push(b'\n');
    file.write_all(&line)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_data())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn envelope_field(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| match c {
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect(),
            '\t' => "\\t".chars().collect(),
            c if c.is_control() => format!("\\u{{{:x}}}", c as u32).chars().collect(),
            c => vec![c],
        })
        .collect()
}

/// Persist the immutable raw intent before returning work that may be sent.
pub fn prepare(
    run_path: &Path,
    doc: &Doc,
    target: &str,
    text: &str,
    authority: Authority,
) -> Result<Submission, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("message is empty".into());
    }
    if text.len() > MESSAGE_LIMIT {
        return Err("message is larger than 32 KiB".into());
    }
    let run = doc.run.as_ref().ok_or_else(|| "run block is missing".to_string())?;
    let run_id = run
        .id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "run.id is missing".to_string())?;
    if !doc
        .tasks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|task| task.id.as_deref() == Some(target))
    {
        return Err(format!(
            "task {target:?} is no longer in this run — re-select a current row"
        ));
    }
    let locator = run.orchestrator.as_ref().ok_or_else(|| {
        "run.orchestrator is not declared — the producer must record its pane or agent locator"
            .to_string()
    })?;
    let destination = if let Some(pane) = locator.pane.as_deref().filter(|s| !s.is_empty()) {
        Destination::Pane(pane.to_string())
    } else if let Some(agent) = locator.agent.as_deref().filter(|s| !s.is_empty()) {
        Destination::Agent(agent.to_string())
    } else {
        return Err("run.orchestrator names neither pane nor agent".into());
    };
    let queued_at_ms = millis();
    let revision = doc.generated_at.as_deref().unwrap_or("unknown");
    let sequence = MESSAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let hash = fnv64(&[
        run_id.as_bytes(),
        revision.as_bytes(),
        target.as_bytes(),
        text.as_bytes(),
        queued_at_ms.to_string().as_bytes(),
        std::process::id().to_string().as_bytes(),
        sequence.to_string().as_bytes(),
    ]);
    let id = format!("msg-{hash:016x}");
    let journal = journal_path(run_path);
    let record = serde_json::json!({
        "type": "message_queued",
        "id": id,
        "queued_at_ms": queued_at_ms,
        "run": run_id,
        "revision": revision,
        "target": target,
        "authority": authority,
        "text": text,
        "destination": {"kind": destination.kind(), "target": destination.target()},
    });
    append_json(&journal, &record)?;
    let envelope = format!(
        "[DAGR OPERATOR MESSAGE]\nmessage_id: {id}\nrun: {}\nrevision: {}\ntarget: {}\nauthority: {}\n\n{text}\n\nAcknowledge this message. Preserve message_id in any resolution event you append to the run file.",
        envelope_field(run_id),
        envelope_field(revision),
        envelope_field(target),
        match authority {
            Authority::Recommend => "recommend_and_return",
            Authority::Decide => "may_decide_and_continue",
        }
    );
    Ok(Submission {
        id,
        run: run_id.to_string(),
        journal,
        destination,
        envelope,
    })
}

/// Queue through Herdr, then append a delivery receipt. A failed transport
/// never erases the already-recorded operator intent.
pub fn deliver(submission: Submission) -> String {
    let result = crate::herdr::prompt(submission.destination.target(), &submission.envelope);
    let (kind, detail) = match result {
        Ok(()) => ("message_delivered", None),
        Err(e) => ("message_delivery_failed", Some(e)),
    };
    let mut record = serde_json::json!({
        "type": kind,
        "id": submission.id,
        "run": submission.run,
        "at_ms": millis(),
    });
    if let Some(detail) = detail.as_deref() {
        record["detail"] = serde_json::Value::String(detail.to_string());
    }
    let journal_result = append_json(&submission.journal, &record);
    match (kind, detail, journal_result) {
        ("message_delivered", _, Ok(())) => format!("queued {} to orchestrator", submission.id),
        ("message_delivered", _, Err(e)) => {
            format!("queued {}, but delivery receipt failed: {e}", submission.id)
        }
        (_, Some(e), Ok(())) => format!("{} recorded; delivery failed: {e}", submission.id),
        (_, Some(e), Err(j)) => format!("delivery failed: {e}; receipt failed: {j}"),
        _ => format!("{} recorded", submission.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_run(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dagr-message-{name}-{}-{}",
            std::process::id(),
            millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("run.json")
    }

    #[test]
    fn defaults_stay_small_and_authority_is_explicit() {
        let d = defaults();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].authority, Authority::Decide);
        assert_eq!(d[1].authority, Authority::Recommend);
    }

    #[test]
    fn config_can_override_a_default_and_add_one() {
        let run = temp_run("config");
        let cfg = config_path(&run);
        std::fs::write(
            &cfg,
            r#"{"version":1,"actions":[
              {"id":"get-guidance","label":"Council","prompt":"Ask two reviewers.","authority":"recommend"},
              {"id":"custom","label":"Custom","prompt":"Do the custom thing.","authority":"decide"}
            ]}"#,
        )
        .unwrap();
        let loaded = load_config(&run);
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.starters.len(), 4);
        assert_eq!(loaded.starters[1].label, "Council");
        let _ = std::fs::remove_dir_all(run.parent().unwrap());
    }

    #[test]
    fn unsupported_config_version_is_visible_and_falls_back_safely() {
        let run = temp_run("config-version");
        std::fs::write(
            config_path(&run),
            r#"{"version":99,"include_defaults":false,"actions":[
              {"id":"surprise","label":"Surprise","prompt":"Do it.","authority":"decide"}
            ]}"#,
        )
        .unwrap();
        let loaded = load_config(&run);
        assert_eq!(loaded.starters, defaults());
        assert!(loaded.warning.as_deref().is_some_and(|w| w.contains("expected 1")));
        let _ = std::fs::remove_dir_all(run.parent().unwrap());
    }

    #[test]
    fn switching_starters_keeps_appended_operator_detail() {
        let starters = defaults();
        let mut draft = Draft::from_starter("G1".into(), &starters, 0);
        draft.text.push_str("Use sol5.6 max and one independent reviewer.");
        let draft = draft.switch_to(&starters, 1);
        assert!(draft.text.starts_with(&starters[1].prompt));
        assert!(draft.text.ends_with("Use sol5.6 max and one independent reviewer."));
        assert_eq!(draft.authority, Authority::Recommend);
    }

    #[test]
    fn prepare_journals_raw_text_before_delivery() {
        let run_path = temp_run("journal");
        let doc: Doc = serde_json::from_str(
            r#"{"dagr":2,"run":{"id":"r","orchestrator":{"pane":"wX:p1"}},"generated_at":"2026-08-17T01:02:03Z","tasks":[{"id":"G1"}]}"#,
        )
        .unwrap();
        let sub = prepare(
            &run_path,
            &doc,
            "G1",
            "Ask sol and fable independently.",
            Authority::Recommend,
        )
        .unwrap();
        let raw = std::fs::read_to_string(journal_path(&run_path)).unwrap();
        assert!(raw.contains(&sub.id));
        assert!(raw.contains("Ask sol and fable independently."));
        assert!(raw.contains("\"authority\":\"recommend\""));
        append_json(
            &journal_path(&run_path),
            &serde_json::json!({"type":"message_delivered","id":sub.id,"run":"r","at_ms":42}),
        )
        .unwrap();
        append_json(
            &journal_path(&run_path),
            &serde_json::json!({
                "type":"message_queued","id":"other","queued_at_ms":1,
                "run":"another-run","target":"G1","authority":"decide","text":"not ours"
            }),
        )
        .unwrap();
        let summaries = read_summaries(&run_path, Some("r")).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].status, "delivered");
        assert_eq!(summaries[0].target, "G1");
        let _ = std::fs::remove_dir_all(run_path.parent().unwrap());
    }

    #[test]
    fn prepare_refuses_a_target_removed_by_a_live_reload() {
        let run_path = temp_run("stale-target");
        let doc: Doc = serde_json::from_str(
            r#"{"dagr":2,"run":{"id":"r","orchestrator":{"pane":"wX:p1"}},"tasks":[]}"#,
        )
        .unwrap();
        let error = prepare(&run_path, &doc, "gone", "continue", Authority::Decide)
            .err()
            .expect("stale target must be refused");
        assert!(error.contains("no longer in this run"));
        assert!(!journal_path(&run_path).exists());
        let _ = std::fs::remove_dir_all(run_path.parent().unwrap());
    }
}
