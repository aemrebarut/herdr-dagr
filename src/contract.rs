//! Serde types for the dagr run-state contract, v3 (see CONTRACT.md).
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

/// Version 3 makes the contextual composer the sole user-facing action.
/// Versions 1 and 2 remain readable without activating their legacy argv.
pub const CONTRACT_VERSION: u64 = 3;
pub const CONTRACT_VERSIONS: &[u64] = &[1, 2, 3];

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
/// Identities are interaction handles, not just labels. Empty/blank ids and
/// terminal-active controls cannot safely name a selectable row or action.
/// The validator reports them and the view keeps their rows visible but inert.
pub fn valid_identity(id: &str) -> bool {
    !id.trim().is_empty() && !id.chars().any(crate::style::terminal_active)
}

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
    /// v1/v2 producer argv declarations are accepted as opaque history.
    /// v3 never validates, binds, expands, or executes this data.
    pub actions: Option<serde_json::Value>,
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
