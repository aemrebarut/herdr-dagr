//! Serde types for the dagr run-state contract, v1 (see CONTRACT.md).
//!
//! Parsing is deliberately permissive: every field is optional at the type
//! level and unknown fields are ignored, so structurally sparse documents
//! parse and `check` reports their problems as findings. (A field of the
//! wrong JSON *type* still fails the whole serde parse and surfaces as a
//! single E001 — field-level findings apply to documents that parse.) The
//! validator, not the type system, is the contract's enforcement surface.

// Fields parsed but not yet consumed are still contract surface (future
// renderer/analytics milestones); keep the schema complete without noise.
#![allow(dead_code)]

use serde::Deserialize;

/// Version 2 adds optional recursive projects and an orchestrator locator.
/// Version 1 remains readable so existing run files gain the corrected gate
/// projection without a migration ceremony.
pub const CONTRACT_VERSION: u64 = 2;
pub const CONTRACT_VERSIONS: &[u64] = &[1, 2];

pub const TASK_STATES: &[&str] = &[
    "queued", "working", "review", "blocked", "done", "failed", "rejected", "canceled",
    "settled_unverified",
];
pub const ATTEMPT_STATES: &[&str] = &[
    "queued", "working", "done", "failed", "rejected", "settled_unverified", "lost",
];
/// Attempt states that require an `outcome`.
pub const TERMINAL_STATES: &[&str] = &["done", "failed", "rejected", "settled_unverified"];
pub const EVIDENCE_TIERS: &[&str] = &["verified", "reported", "heuristic", "asserted"];
pub const CAUSE_TYPES: &[&str] = &["initial", "sent_back", "gate_failed", "followup", "superseded"];
pub const FUTURE_ON: &[&str] = &["pass", "fail"];
pub const ATTRIBUTIONS: &[&str] = &["planned", "predicted"];
pub const EVENT_TYPES: &[&str] = &[
    "attempt_started", "attempt_settled", "promoted", "directive", "message_resolved", "note",
];
pub const DIRECTIVE_VERBS: &[&str] = &["reject", "unblock", "answer", "rule"];
/// Placeholders an action argv template may use (CONTRACT §9).
pub const ACTION_PLACEHOLDERS: &[&str] = &["{task}", "{attempt}", "{operator}", "{text}", "{key}"];
/// Verbs the pane binds to keys: u / a / o / x.
pub const BOUND_ACTIONS: &[&str] = &["unblock", "answer", "accept", "reject"];

#[derive(Deserialize)]
pub struct Doc {
    pub dagr: Option<u64>,
    pub run: Option<Run>,
    pub generated_at: Option<String>,
    /// v2: recursive visual/organizational scopes. The run is the implicit
    /// root project; these are optional named descendants.
    #[serde(default)]
    pub projects: Vec<Project>,
    pub tasks: Option<Vec<Task>>,
    #[serde(default)]
    pub events: Vec<Event>,
    /// §9 optional extension: verb → producer CLI template.
    pub actions: Option<std::collections::BTreeMap<String, ActionTpl>>,
}

#[derive(Deserialize)]
pub struct ActionTpl {
    /// Kept as raw values so a non-string element is a per-field E190
    /// with a JSON path, not a whole-document E001.
    pub argv: Option<Vec<serde_json::Value>>,
}

impl ActionTpl {
    /// The template as strings — `None` unless EVERY element is a
    /// string. A malformed template must fail loudly; dropping the bad
    /// element would silently repair it into a different argv than the
    /// one the producer declared.
    pub fn argv_strings(&self) -> Option<Vec<String>> {
        self.argv
            .as_ref()?
            .iter()
            .map(|x| x.as_str().map(String::from))
            .collect()
    }
}

#[derive(Deserialize)]
pub struct Run {
    pub id: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    /// Where operator messages from the pane should be queued. This is a
    /// transport locator, not a second workflow engine.
    pub orchestrator: Option<Locator>,
}

#[derive(Deserialize)]
pub struct Project {
    pub id: Option<String>,
    pub title: Option<String>,
    /// Omit for a direct child of the run root.
    pub parent: Option<String>,
    pub owner: Option<String>,
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct Task {
    pub id: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub owner: Option<String>,
    /// v2 visual home. Dependencies may freely cross project boundaries;
    /// those edges remain graph edges rather than forcing duplicate homes.
    pub project: Option<String>,
    pub state: Option<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    pub inputs: Option<Vec<String>>,
    pub unblock: Option<String>,
    pub note: Option<String>,
    /// Human-readable acceptance/gate criterion. It is displayed as
    /// context, never executed by dagr.
    pub criteria: Option<String>,
    pub policy: Option<Policy>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
}

#[derive(Deserialize)]
pub struct Policy {
    pub rounds_max: Option<u64>,
    pub gate_cmd: Option<Vec<String>>,
    #[serde(default)]
    pub futures: Vec<Future>,
}

#[derive(Deserialize)]
pub struct Future {
    pub on: Option<String>,
    pub streak: Option<u64>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub node: Option<FutureNode>,
    pub after: Option<String>,
    pub loop_back: Option<bool>,
    pub source: Option<String>,
}

#[derive(Deserialize)]
pub struct FutureNode {
    pub id: Option<String>,
    pub title: Option<String>,
    pub actor: Option<String>,
    pub model: Option<String>,
    pub attribution: Option<String>,
}

#[derive(Deserialize)]
pub struct Attempt {
    pub id: Option<String>,
    pub n: Option<u64>,
    pub cause: Option<Cause>,
    pub actor: Option<String>,
    pub model: Option<String>,
    pub locator: Option<Locator>,
    pub state: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub outcome: Option<Outcome>,
    pub progress: Option<Progress>,
    pub liveness: Option<Liveness>,
    pub chain_key: Option<String>,
}

#[derive(Deserialize)]
pub struct Cause {
    #[serde(rename = "type")]
    pub cause_type: Option<String>,
    pub by: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct Locator {
    pub pane: Option<String>,
    pub agent: Option<String>,
}

#[derive(Deserialize)]
pub struct Outcome {
    pub result: Option<String>,
    pub evidence: Option<String>,
    pub receipt: Option<String>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct Progress {
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct Liveness {
    pub prompt_acknowledged: Option<bool>,
    pub last_output_at: Option<String>,
    pub queued_input: Option<u64>,
}

#[derive(Deserialize)]
pub struct Event {
    pub at: Option<String>,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub task: Option<String>,
    pub attempt: Option<String>,
    pub actor: Option<String>,
    pub verb: Option<String>,
    pub by: Option<String>,
    pub detail: Option<String>,
    /// Durable correlation back to the immutable operator message journal.
    pub message_id: Option<String>,
    #[serde(default)]
    pub source_messages: Vec<String>,
}
