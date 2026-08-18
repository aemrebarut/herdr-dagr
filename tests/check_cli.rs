//! Integration tests for `dagr check`, driven through the real binary
//! (CARGO_BIN_EXE_dagr) so they exercise the CLI contract — exit codes,
//! --json shape, finding codes — exactly as a producer's write→check→fix
//! loop sees them.

use std::process::Command;

fn run_check(doc: &str) -> (i32, Vec<(String, String)>) {
    let dir = std::env::temp_dir().join(format!(
        "dagr-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.json");
    std::fs::write(&path, doc).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .args(["check", path.to_str().unwrap(), "--json"])
        .output()
        .expect("dagr runs");
    let code = out.status.code().unwrap_or(-1);
    let findings: Vec<(String, String)> = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .ok()
        .and_then(|v| {
            v.as_array().map(|a| {
                a.iter()
                    .map(|f| {
                        (
                            f["level"].as_str().unwrap_or("").to_string(),
                            f["code"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    (code, findings)
}

fn has(findings: &[(String, String)], code: &str) -> bool {
    findings.iter().any(|(_, c)| c == code)
}

/// A minimal valid document: one done task, one verified attempt.
fn base() -> serde_json::Value {
    serde_json::json!({
        "dagr": 1,
        "run": {"id": "r1", "title": "t", "started_at": "2026-01-01T10:00:00Z"},
        "generated_at": "2026-01-01T11:00:00Z",
        "tasks": [{
            "id": "A", "title": "task a", "kind": "impl", "state": "done", "deps": [],
            "attempts": [{
                "id": "A·a1", "n": 1, "cause": {"type": "initial"},
                "actor": "dev", "state": "done",
                "started_at": "2026-01-01T10:05:00Z", "ended_at": "2026-01-01T10:30:00Z",
                "outcome": {"result": "done", "evidence": "verified", "receipt": "test ✓"}
            }]
        }],
        "events": []
    })
}

#[test]
fn clean_doc_exits_zero() {
    let (code, findings) = run_check(&base().to_string());
    assert_eq!(code, 0, "clean doc should exit 0, findings: {findings:?}");
    assert!(findings.is_empty(), "unexpected findings: {findings:?}");
}

#[test]
fn not_json_is_e001() {
    let (code, findings) = run_check("{nope");
    assert_eq!(code, 1);
    assert!(has(&findings, "E001"), "{findings:?}");
}

#[test]
fn wrong_version_is_e100() {
    let mut d = base();
    d["dagr"] = serde_json::json!(99);
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E100"), "{findings:?}");
}

#[test]
fn duplicate_task_id_is_e110() {
    let mut d = base();
    let t = d["tasks"][0].clone();
    d["tasks"].as_array_mut().unwrap().push(t);
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E110"), "{findings:?}");
}

#[test]
fn bad_task_state_is_e112() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("zombie");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E112"), "{findings:?}");
}

#[test]
fn dangling_dep_is_e120() {
    let mut d = base();
    d["tasks"][0]["deps"] = serde_json::json!(["NOPE"]);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E120"), "{findings:?}");
}

#[test]
fn dangling_gate_input_is_e121() {
    let mut d = base();
    d["tasks"][0]["kind"] = serde_json::json!("gate");
    d["tasks"][0]["inputs"] = serde_json::json!(["NOPE"]);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E121"), "{findings:?}");
}

#[test]
fn duplicate_attempt_id_is_e130() {
    let mut d = base();
    let a = d["tasks"][0]["attempts"][0].clone();
    d["tasks"][0]["attempts"].as_array_mut().unwrap().push(a);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E130"), "{findings:?}");
}

#[test]
fn bad_cause_type_is_e133() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["cause"] = serde_json::json!({"type": "vibes"});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E133"), "{findings:?}");
}

#[test]
fn bad_evidence_tier_is_e142() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["outcome"]["evidence"] = serde_json::json!("trust-me");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E142"), "{findings:?}");
}

#[test]
fn task_state_contradicting_attempts_is_e150() {
    let mut d = base();
    // task says done, its only attempt failed
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("failed");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::json!(
        {"result": "failed", "evidence": "verified", "reason": "boom"});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E150"), "{findings:?}");
}

#[test]
fn bad_timestamp_is_e180() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["started_at"] = serde_json::json!("yesterday-ish");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E180"), "{findings:?}");
}

#[test]
fn dependency_cycle_is_e122() {
    let mut d = base();
    d["tasks"][0]["deps"] = serde_json::json!(["B"]);
    let mut b = d["tasks"][0].clone();
    b["id"] = serde_json::json!("B");
    b["deps"] = serde_json::json!(["A"]);
    b["attempts"] = serde_json::json!([]);
    b["state"] = serde_json::json!("queued");
    d["tasks"].as_array_mut().unwrap().push(b);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E122"), "{findings:?}");
}

#[test]
fn cycle_doc_still_renders_rows() {
    // the same cyclic doc must not produce an empty trace
    let mut d = base();
    d["tasks"][0]["deps"] = serde_json::json!(["B"]);
    let mut b = d["tasks"][0].clone();
    b["id"] = serde_json::json!("B");
    b["deps"] = serde_json::json!(["A"]);
    b["attempts"] = serde_json::json!([]);
    b["state"] = serde_json::json!("queued");
    d["tasks"].as_array_mut().unwrap().push(b);
    let (code, out) = run_snapshot(&d.to_string(), &[]);
    assert_eq!(code, 0);
    assert!(out.contains("task a"), "cyclic tasks must stay visible:\n{out}");
}

#[test]
fn mutual_cause_cycle_is_e135() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"] = serde_json::json!([
        {"id": "A·a1", "n": 1, "cause": {"type": "followup", "ref": "A·a2"},
         "actor": "dev", "state": "done",
         "started_at": "2026-01-01T10:05:00Z", "ended_at": "2026-01-01T10:06:00Z",
         "outcome": {"result": "done", "evidence": "verified"}},
        {"id": "A·a2", "n": 2, "cause": {"type": "followup", "ref": "A·a1"},
         "actor": "dev", "state": "working",
         "started_at": "2026-01-01T10:07:00Z"}
    ]);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E135"), "{findings:?}");
}

#[test]
fn cause_from_the_future_is_e136() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"] = serde_json::json!([
        {"id": "A·a1", "n": 1, "cause": {"type": "followup", "ref": "A·a2"},
         "actor": "dev", "state": "done",
         "started_at": "2026-01-01T10:05:00Z", "ended_at": "2026-01-01T10:06:00Z",
         "outcome": {"result": "done", "evidence": "verified"}},
        {"id": "A·a2", "n": 2, "cause": {"type": "initial"},
         "actor": "dev", "state": "working",
         "started_at": "2026-01-01T10:30:00Z"}
    ]);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E136"), "{findings:?}");
}

#[test]
fn after_outside_own_policy_is_e162() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::Value::Null;
    d["tasks"][0]["policy"] = serde_json::json!({
        "futures": [{"on": "fail", "after": "GHOST",
                     "node": {"id": "F1", "title": "fix"}}]
    });
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E162"), "{findings:?}");
}

#[test]
fn duplicate_future_node_id_is_e164() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::Value::Null;
    d["tasks"][0]["policy"] = serde_json::json!({
        "futures": [
            {"on": "fail", "node": {"id": "F1", "title": "fix"}},
            {"on": "fail", "node": {"id": "F1", "title": "fix again"}}
        ]
    });
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E164"), "{findings:?}");
}

#[test]
fn impossible_calendar_dates_are_e180() {
    for bad in [
        "2026-99-99T99:99:00Z",
        "2026-02-30T10:00:00Z",
        "2026-00-10T10:00:00Z",
        "2026-01-01T25:00:00Z",
        "2026-01-01T10:61:00Z",
    ] {
        let mut d = base();
        d["tasks"][0]["attempts"][0]["started_at"] = serde_json::json!(bad);
        let (_, findings) = run_check(&d.to_string());
        assert!(has(&findings, "E180"), "{bad} should be E180: {findings:?}");
    }
}

#[test]
fn offset_timestamps_are_valid_and_normalized() {
    let mut d = base();
    // 05:05-05:00 == 10:05Z: started_at before ended_at only if the
    // offset is actually applied
    d["tasks"][0]["attempts"][0]["started_at"] = serde_json::json!("2026-01-01T05:05:00-05:00");
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 0, "offset timestamp should be valid: {findings:?}");
}

#[test]
fn attempt_id_colliding_with_task_id_is_e113() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["id"] = serde_json::json!("A");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E113"), "{findings:?}");
}

#[test]
fn ended_before_started_is_e181() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::json!("2026-01-01T09:00:00Z");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E181"), "{findings:?}");
}

#[test]
fn queued_task_with_working_attempt_is_e150() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("queued");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::Value::Null;
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E150"), "{findings:?}");
}

#[test]
fn queued_task_with_settled_latest_is_e150() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("queued");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E150"), "backward move: {findings:?}");
}

#[test]
fn failed_task_with_lost_latest_attempt_is_legal() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("failed");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("lost");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    let (_, findings) = run_check(&d.to_string());
    assert!(
        !findings.iter().any(|(_, c)| c == "E150"),
        "lost latest attempt fails the task legally: {findings:?}"
    );
}

#[test]
fn gate_with_explicit_empty_inputs_warns_w202() {
    let mut d = base();
    d["tasks"][0]["kind"] = serde_json::json!("gate");
    d["tasks"][0]["inputs"] = serde_json::json!([]);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "W202"), "{findings:?}");
}

#[test]
fn empty_liveness_object_still_warns_w208() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["locator"] = serde_json::json!({"pane": "x:1"});
    d["tasks"][0]["attempts"][0]["liveness"] = serde_json::json!({});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "W208"), "{findings:?}");
}

#[test]
fn underspecified_events_are_e172() {
    let mut d = base();
    d["events"] = serde_json::json!([
        {"at": "2026-01-01T10:31:00Z", "type": "attempt_settled"},
        {"at": "2026-01-01T10:32:00Z", "type": "promoted"},
        {"at": "2026-01-01T10:33:00Z", "type": "directive", "verb": "bogus"}
    ]);
    let (_, findings) = run_check(&d.to_string());
    let e172 = findings.iter().filter(|(_, c)| c == "E172").count();
    assert!(e172 >= 4, "settled/promoted subjects, bad verb, missing by: {findings:?}");
}

#[test]
fn completion_without_evidence_warns_w201() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::json!({"result": "done"});
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 0, "warnings alone keep exit 0");
    assert!(has(&findings, "W201"), "{findings:?}");
}

#[test]
fn strict_turns_warnings_into_failure() {
    let mut d = base();
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::json!({"result": "done"});
    let dir = std::env::temp_dir().join(format!("dagr-strict-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.json");
    std::fs::write(&path, d.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .args(["check", path.to_str().unwrap(), "--strict"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
}

// ── §9 actions (optional extension) ────────────────────────────────────

#[test]
fn action_without_argv_is_e190() {
    let mut d = base();
    d["actions"] = serde_json::json!({"accept": {"argv": []}});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E190"), "{findings:?}");
}

#[test]
fn unknown_placeholder_is_e191() {
    let mut d = base();
    d["actions"] = serde_json::json!({"accept": {"argv": ["prod", "accept", "{tsak}", "--key", "{key}"]}});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E191"), "{findings:?}");
}

#[test]
fn unbound_verb_warns_w211_but_known_verbs_pass_clean() {
    let mut d = base();
    d["actions"] = serde_json::json!({
        "accept": {"argv": ["prod", "accept", "{task}", "--attempt", "{attempt}", "--key", "{key}"]},
        "escalate": {"argv": ["prod", "escalate", "{task}", "--key", "{key}"]}
    });
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 0, "{findings:?}");
    assert!(has(&findings, "W211"), "{findings:?}");
    assert_eq!(findings.len(), 1, "accept must be finding-free: {findings:?}");
}

#[test]
fn a_cycle_through_gate_inputs_is_e122() {
    // L1 waits on G1; G1 fans in L1 via inputs only. Without inputs in
    // the E122 edge set this deadlock is validator-clean.
    let d = serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T10:00:00Z",
        "tasks": [
            {"id": "L1", "title": "lane", "kind": "impl", "state": "queued", "deps": ["G1"], "attempts": []},
            {"id": "G1", "title": "gate", "kind": "gate", "state": "queued", "deps": [], "inputs": ["L1"], "attempts": []}
        ]
    });
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E122"), "{findings:?}");
}

#[test]
fn docs_without_actions_get_no_action_findings() {
    let (_, findings) = run_check(&base().to_string());
    assert!(!findings.iter().any(|(_, c)| c.starts_with("E19") || c == "W211"));
}

// ── dagr stats ─────────────────────────────────────────────────────────

#[test]
fn stats_reports_flow_over_the_state_matrix() {
    let dir = std::env::temp_dir().join(format!("dagr-stats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.json");
    std::fs::write(&path, STATES).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .args(["stats", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["wip"], 2, "T2 and T10 are the working tasks: {v}");
    assert!(v["critical_path"].as_array().is_some_and(|p| !p.is_empty()));
    assert!(v["tasks"].as_array().is_some_and(|t| t.len() == 12));
}

fn run_stats(doc: &serde_json::Value) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!(
        "dagr-stats-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .args(["stats", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn critical_path_stops_at_settled_dependencies() {
    // L1 → R1(done) → B1: R1 is settled, so L1 and B1 are separate
    // 1-chains — the path must NOT thread through R1. The real longest
    // chain is C2 → C1, both unfinished with a genuine deps edge.
    let doc = serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T12:00:00Z",
        "tasks": [
            {"id": "B1", "state": "working", "deps": [], "attempts": []},
            {"id": "R1", "state": "done", "deps": ["B1"], "attempts": []},
            {"id": "L1", "state": "queued", "deps": ["R1"], "attempts": []},
            {"id": "C1", "state": "working", "deps": [], "attempts": []},
            {"id": "C2", "state": "queued", "deps": ["C1"], "attempts": []}
        ]
    });
    let v = run_stats(&doc);
    assert_eq!(
        v["critical_path"],
        serde_json::json!(["C1", "C2"]),
        "settled R1 must break the L1→R1→B1 chain: {v}"
    );
}

#[test]
fn critical_path_follows_gate_inputs_edges() {
    // the same gate, declared the skill's way: fan-in in `inputs`, empty
    // `deps`. The path must NOT truncate at the gate — the deps-only
    // blind spot E122 had, in a different consumer.
    let doc = serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T12:00:00Z",
        "tasks": [
            {"id": "L1", "state": "working", "deps": [], "attempts": []},
            {"id": "L2", "state": "queued", "deps": ["L1"], "attempts": []},
            {"id": "G1", "kind": "gate", "state": "queued", "deps": [], "inputs": ["L1", "L2"], "attempts": []},
            {"id": "S1", "state": "queued", "deps": ["G1"], "attempts": []}
        ]
    });
    let v = run_stats(&doc);
    assert_eq!(
        v["critical_path"],
        serde_json::json!(["L1", "L2", "G1", "S1"]),
        "an inputs-declared gate must not halve the path: {v}"
    );
}

#[test]
fn lost_attempts_contribute_no_duration() {
    // `lost` is not a settlement (CONTRACT: the runtime vanished); a
    // 90-minute lost span must not inflate the mean or the ETA built on
    // it.
    let doc = serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T12:00:00Z",
        "tasks": [{
            "id": "A", "state": "working", "deps": [],
            "attempts": [
                {"id": "A·a1", "n": 1, "state": "lost",
                 "started_at": "2026-01-01T09:00:00Z", "ended_at": "2026-01-01T10:30:00Z"},
                {"id": "A·a2", "n": 2, "state": "done",
                 "started_at": "2026-01-01T10:30:00Z", "ended_at": "2026-01-01T10:50:00Z",
                 "outcome": {"result": "done", "evidence": "reported"}},
                {"id": "A·a3", "n": 3, "state": "working",
                 "started_at": "2026-01-01T11:00:00Z"}
            ]
        }]
    });
    let v = run_stats(&doc);
    assert_eq!(v["avg_attempt_min"], 20, "only the settled 20m attempt counts: {v}");
    assert_eq!(v["tasks"][0]["worked_min"], 20, "{v}");
}

#[test]
fn clock_falls_back_to_latest_timestamp_and_says_so() {
    // no generated_at; the latest timestamp anywhere is an event's `at`
    let doc = serde_json::json!({
        "dagr": 1,
        "run": {"id": "r", "started_at": "2026-01-01T10:00:00Z"},
        "tasks": [{
            "id": "A", "state": "working", "deps": [],
            "attempts": [{
                "id": "A·a1", "n": 1, "state": "working",
                "started_at": "2026-01-01T10:10:00Z",
                "liveness": {"last_output_at": "2026-01-01T11:30:00Z"}
            }]
        }],
        "events": [{"at": "2026-01-01T11:45:00Z", "type": "note", "task": "A"}]
    });
    let v = run_stats(&doc);
    assert_eq!(v["clock"], "latest_timestamp", "{v}");
    // 11:45 is the doc's clock → A's latest start was 95 minutes ago
    let task = &v["tasks"][0];
    assert_eq!(task["since_latest_start_min"], 95, "{v}");

    let with_clock = run_stats(&serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T12:00:00Z",
        "tasks": [{"id": "A", "state": "working", "deps": [], "attempts": []}]
    }));
    assert_eq!(with_clock["clock"], "generated_at", "{with_clock}");
}

#[test]
fn durations_are_terminal_only_and_rework_is_cause_based() {
    let doc = serde_json::json!({
        "dagr": 1, "generated_at": "2026-01-01T12:00:00Z",
        "tasks": [
            // live attempt: its started→ended-less span must not count
            {"id": "A", "state": "working", "deps": [], "attempts": [
                {"id": "A·a1", "n": 1, "state": "done",
                 "started_at": "2026-01-01T10:00:00Z", "ended_at": "2026-01-01T10:20:00Z"},
                {"id": "A·a2", "n": 2, "state": "working",
                 "cause": {"type": "sent_back", "ref": "R"},
                 "started_at": "2026-01-01T11:00:00Z", "ended_at": "2026-01-01T11:59:00Z"}
            ]},
            // second attempt by deliberate iteration: NOT rework
            {"id": "B", "state": "done", "deps": [], "attempts": [
                {"id": "B·a1", "n": 1, "state": "done",
                 "started_at": "2026-01-01T10:00:00Z", "ended_at": "2026-01-01T10:20:00Z"},
                {"id": "B·a2", "n": 2, "state": "done", "cause": {"type": "followup"},
                 "started_at": "2026-01-01T10:30:00Z", "ended_at": "2026-01-01T10:50:00Z"}
            ]}
        ]
    });
    let v = run_stats(&doc);
    // three terminal attempts of 20m each; A·a2 (live, 59m) excluded
    assert_eq!(v["avg_attempt_min"], 20, "{v}");
    // A bounced (sent_back); B iterated (followup) — 1 of 2
    assert_eq!(v["rework"]["reworked"], 1, "{v}");
    assert_eq!(v["rework"]["attempted"], 2, "{v}");
    // settled B reports no since_latest_start; live A does
    assert_eq!(v["tasks"][1]["since_latest_start_min"], serde_json::Value::Null, "{v}");
    assert_eq!(v["tasks"][0]["since_latest_start_min"], 60, "{v}");
}

// ── view --snapshot robustness: hostile-but-parsing inputs must not panic ──

fn run_snapshot(doc: &str, extra: &[&str]) -> (i32, String) {
    run_snapshot_with_working_glyph(doc, extra, None)
}

fn run_snapshot_with_working_glyph(
    doc: &str,
    extra: &[&str],
    working_glyph: Option<&str>,
) -> (i32, String) {
    let dir = std::env::temp_dir().join(format!(
        "dagr-snap-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.json");
    std::fs::write(&path, doc).unwrap();
    let mut args = vec!["view", path.to_str().unwrap(), "--snapshot"];
    args.extend_from_slice(extra);
    let mut command = Command::new(env!("CARGO_BIN_EXE_dagr"));
    command.args(&args).env_remove("DAGR_WORKING_GLYPH");
    if let Some(glyph) = working_glyph {
        command.env("DAGR_WORKING_GLYPH", glyph);
    }
    let out = command.output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn snapshot_renders_empty_doc() {
    let (code, out) = run_snapshot("{}", &[]);
    assert_eq!(code, 0);
    assert!(!out.is_empty());
}

#[test]
fn snapshot_survives_tiny_and_huge_widths() {
    let doc = base().to_string();
    for w in ["1", "20", "40", "300"] {
        let (code, _) = run_snapshot(&doc, &["--width", w]);
        assert_eq!(code, 0, "width {w} must not crash");
    }
}

// ── the state-matrix fixture: the whole state machine as data ──────────
// samples/states.json carries every task state, every attempt state, and
// every evidence tier; these tests hold the renderer to it.

const STATES: &str = include_str!("../samples/states.json");

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn state_matrix_fixture_is_clean() {
    let (code, findings) = run_check(STATES);
    assert_eq!(code, 0, "states.json must stay warning-free: {findings:?}");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn state_matrix_renders_inside_every_width() {
    for w in [40usize, 70, 100, 120, 150, 200] {
        let (code, out) = run_snapshot(STATES, &["--width", &w.to_string()]);
        assert_eq!(code, 0, "width {w}");
        for line in strip_ansi(&out).lines() {
            // every glyph in the grammar is single-column, so chars ≈ columns
            assert!(
                line.chars().count() <= w,
                "width {w} overflowed: {:?} ({} cols)",
                line,
                line.chars().count()
            );
        }
    }
}

#[test]
fn working_glyph_uses_bullseye_with_explicit_ascii_fallback() {
    let (_, normal) = run_snapshot(STATES, &["--width", "200"]);
    let (_, ascii) =
        run_snapshot_with_working_glyph(STATES, &["--width", "200"], Some("*"));
    assert!(strip_ansi(&normal).contains("◎ T2"), "default working glyph is ◎");
    assert!(strip_ansi(&ascii).contains("* T2"), "explicit fallback is ASCII *");
}

#[test]
fn blocked_and_review_outrank_working_attempts() {
    let (_, out) = run_snapshot(STATES, &["--width", "200"]);
    let plain = strip_ansi(&out);
    let t4 = plain.lines().find(|l| l.contains("T4  ")).expect("T4 row");
    assert!(t4.contains('■') && t4.contains("BLOCKED"), "task blocked must outrank attempt working: {t4:?}");
    let t3 = plain.lines().find(|l| l.contains("T3  ")).expect("T3 row");
    assert!(t3.contains('◈') && t3.contains("review"), "task review must outrank attempt working: {t3:?}");
}

#[test]
fn stubs_are_earned_not_ambient() {
    let (_, out) = run_snapshot(STATES, &["--width", "200"]);
    let plain = strip_ansi(&out);
    assert!(plain.contains("rescope after block"), "blocked task with working attempt earns its future");
    assert!(plain.contains("» T5"), "attempt-less blocked task earns its future");
    assert!(!plain.contains("phantom future"), "queued task must not speculate");
}

#[test]
fn gates_show_state_bearing_joins_with_and_without_attempts() {
    let (_, out) = run_snapshot(STATES, &["--width", "200"]);
    let plain = strip_ansi(&out);
    let g1 = plain.lines().find(|l| l.contains("⋈ G1")).expect("G1 row");
    let g2 = plain.lines().find(|l| l.contains("⋈ G2")).expect("G2 row");
    assert!(g1.contains("●◎✗→⋈ G1"), "G1 shows ordered done/working/failed inputs:\n{g1}");
    assert!(g2.contains("●→⋈ G2"), "attempted G2 keeps the join:\n{g2}");
    assert!(plain.contains("waits T2"), "unmet gate names its blocker");
}

fn seven_lane_join_doc() -> String {
    seven_lane_join_doc_at_depth(4, "G")
}

fn seven_lane_join_doc_at_depth(root_depth: usize, gate_id: &str) -> String {
    let mut tasks = Vec::new();
    for depth in 0..root_depth {
        let id = format!("R{depth}");
        let deps = if depth == 0 { vec![] } else { vec![format!("R{}", depth - 1)] };
        tasks.push(serde_json::json!({
            "id": id,
            "title": "root",
            "kind": "impl",
            "state": "queued",
            "deps": deps,
            "attempts": []
        }));
    }
    let lane_parent = format!("R{}", root_depth - 1);
    for lane in 1..=7 {
        let id = format!("STREAM{lane}");
        tasks.push(serde_json::json!({
            "id": id,
            "title": "lane",
            "kind": "impl",
            "state": "queued",
            "deps": [lane_parent],
            "attempts": []
        }));
    }
    tasks.push(serde_json::json!({
        "id": gate_id,
        "title": "join seven lanes",
        "kind": "gate",
        "state": "queued",
        "deps": ["STREAM1", "STREAM2", "STREAM3", "STREAM4", "STREAM5", "STREAM6", "STREAM7"],
        "attempts": []
    }));
    serde_json::json!({
        "dagr": 1,
        "run": {"id": "join", "title": "join"},
        "generated_at": "2026-08-17T12:00:00Z",
        "tasks": tasks,
        "events": []
    })
    .to_string()
}

#[test]
fn join_strip_degrades_from_inputs_to_counts_to_total() {
    let doc = seven_lane_join_doc();
    let (_, wide) = run_snapshot(&doc, &["--width", "78"]);
    let (_, compact) = run_snapshot(&doc, &["--width", "34"]);
    let (_, tiny) = run_snapshot(&doc, &["--width", "20"]);
    assert!(strip_ansi(&wide).contains("○○○○○○○→⋈ G"), "wide strip:\n{wide}");
    assert!(strip_ansi(&compact).contains("○7→⋈ G"), "counted strip:\n{compact}");
    assert!(strip_ansi(&tiny).contains("7→1 ⋈ G"), "tiny strip:\n{tiny}");
}

#[test]
fn deep_rows_elide_ancestry_before_join_or_identity() {
    let compact_doc = seven_lane_join_doc_at_depth(10, "GATE-LONG");
    let (_, tiny) = run_snapshot(&compact_doc, &["--width", "20"]);
    let plain = strip_ansi(&tiny);
    let gate = plain.lines().find(|line| line.contains("⋈")).expect("gate row");
    assert!(gate.contains("…"), "deep ancestry is explicitly elided:\n{gate}");
    assert!(gate.contains("7→1 ⋈ GATE"), "join and useful id prefix survive:\n{gate}");

    let full_doc = seven_lane_join_doc_at_depth(30, "GATE-LONG");
    let (_, full) = run_snapshot(&full_doc, &["--width", "96"]);
    let plain = strip_ansi(&full);
    let gate = plain.lines().find(|line| line.contains("⋈")).expect("gate row");
    assert!(gate.contains("…"), "deep ancestry is explicitly elided:\n{gate}");
    assert!(
        gate.contains("○7→⋈ GATE-LONG"),
        "join and useful id prefix survive the full layout:\n{gate}"
    );
}

#[test]
fn off_tree_annotations_render() {
    let (_, out) = run_snapshot(STATES, &["--width", "200"]);
    let plain = strip_ansi(&out);
    assert!(plain.contains("⇠ T3") && plain.contains("⇠ T7"), "extra deps beyond the rail get ⇠ chips");
    assert!(plain.contains("⇠ resync@G1"), "the task note is ink, not dead data");
}

#[test]
fn selected_gate_unrolls_its_fanin() {
    let (_, out) = run_snapshot(STATES, &["--width", "200", "--select", "G1"]);
    let plain = strip_ansi(&out);
    assert!(plain.contains("holds"), "the unmet input is named as holding the gate:\n{plain}");
}

#[test]
fn snapshot_survives_absurd_streak() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["state"] = serde_json::json!("working");
    d["tasks"][0]["attempts"][0]["outcome"] = serde_json::Value::Null;
    d["tasks"][0]["attempts"][0]["ended_at"] = serde_json::Value::Null;
    d["tasks"][0]["policy"] = serde_json::json!({
        "futures": [{"on": "fail", "streak": 18446744073709551615u64,
                     "node": {"id": "X", "title": "x"}}]
    });
    let (code, _) = run_snapshot(&d.to_string(), &["--select", "A·a1"]);
    assert_eq!(code, 0, "huge streak must not panic");
}

#[test]
fn action_template_without_key_is_e192() {
    let mut d = base();
    d["actions"] = serde_json::json!({
        "accept": {"argv": ["prod", "accept", "{task}"]}
    });
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E192"), "{findings:?}");
}

#[test]
fn non_string_argv_element_is_e190_at_its_path() {
    let mut d = base();
    d["actions"] = serde_json::json!({
        "accept": {"argv": ["prod", 42, "{key}"]}
    });
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E190"), "non-string element must be E190, not E001: {findings:?}");
}

#[test]
fn empty_argv0_is_e193() {
    let mut d = base();
    d["actions"] = serde_json::json!({
        "accept": {"argv": ["", "accept", "{key}"]}
    });
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E193"), "{findings:?}");
}

#[test]
fn placeholder_in_argv0_is_e193() {
    // the executable must be pinned by the template — an argv[0]
    // resolved from the environment or run data at confirm time is the
    // one slot the gate's reader is least likely to scrutinise (M4 F8)
    for argv0 in ["{operator}", "prod-{task}"] {
        let mut d = base();
        d["actions"] = serde_json::json!({
            "accept": {"argv": [argv0, "accept", "{task}", "--key", "{key}"]}
        });
        let (code, findings) = run_check(&d.to_string());
        assert_eq!(code, 1, "{argv0}");
        assert!(has(&findings, "E193"), "{argv0}: {findings:?}");
    }
}

#[test]
fn actions_without_generated_at_is_e194() {
    // intent keys hash the document revision; without one every
    // repetition of an intent keys identically forever (M4 F10)
    let mut d = base();
    d["actions"] = serde_json::json!({
        "accept": {"argv": ["prod", "accept", "{task}", "--key", "{key}"]}
    });
    d.as_object_mut().unwrap().remove("generated_at");
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E194"), "{findings:?}");
}
