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

#[test]
fn version_reports_the_current_contract_and_compatibility() {
    let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .arg("--version")
        .output()
        .expect("dagr runs");
    assert!(out.status.success());
    let version = String::from_utf8_lossy(&out.stdout);
    assert!(version.contains("dagr 0.3.1 (contract v3; reads v1/v2)"), "{version}");
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
fn v2_projects_and_orchestrator_locator_are_clean() {
    let mut d = base();
    d["dagr"] = serde_json::json!(2);
    d["run"]["orchestrator"] = serde_json::json!({"pane": "wX:p1"});
    d["projects"] = serde_json::json!([
        {"id": "P", "title": "Product"},
        {"id": "A", "title": "Stream A", "parent": "P"}
    ]);
    d["tasks"][0]["project"] = serde_json::json!("A");
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 0, "v2 document should be clean: {findings:?}");
}

#[test]
fn project_hierarchy_and_task_homes_are_validated() {
    let mut d = base();
    d["dagr"] = serde_json::json!(2);
    d["projects"] = serde_json::json!([
        {"id": "A", "title": "A", "parent": "B"},
        {"id": "B", "title": "B", "parent": "A"}
    ]);
    d["tasks"][0]["project"] = serde_json::json!("MISSING");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E106"), "project cycle: {findings:?}");
    assert!(has(&findings, "E107"), "unknown task home: {findings:?}");
}

#[test]
fn a_gate_cannot_claim_a_project_that_excludes_an_input() {
    let mut d = base();
    d["dagr"] = serde_json::json!(2);
    d["projects"] = serde_json::json!([
        {"id": "P", "title": "Parent"},
        {"id": "A", "title": "A", "parent": "P"},
        {"id": "B", "title": "B", "parent": "P"}
    ]);
    d["tasks"][0]["project"] = serde_json::json!("A");
    let gate = serde_json::json!({
        "id":"G", "title":"join", "kind":"gate", "project":"B",
        "state":"queued", "deps":["A"], "attempts":[]
    });
    d["tasks"].as_array_mut().unwrap().push(gate);
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E108"), "{findings:?}");
}

#[test]
fn empty_orchestrator_locator_is_e103() {
    let mut d = base();
    d["dagr"] = serde_json::json!(2);
    d["run"]["orchestrator"] = serde_json::json!({});
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E103"), "{findings:?}");
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
fn terminal_active_and_blank_identities_are_rejected() {
    let mut d = base();
    d["tasks"][0]["id"] = serde_json::json!("  ");
    d["tasks"][0]["attempts"][0]["id"] = serde_json::json!("A\u{1b}");
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E111"), "blank task identity: {findings:?}");
    assert!(has(&findings, "E131"), "terminal-active attempt identity: {findings:?}");
}

#[test]
fn task_identity_cannot_alias_a_project_row_key() {
    let mut d = base();
    d["dagr"] = serde_json::json!(2);
    d["projects"] = serde_json::json!([{"id": "P", "title": "Project"}]);
    d["tasks"][0]["id"] = serde_json::json!("project:P");
    let (code, findings) = run_check(&d.to_string());
    assert_eq!(code, 1);
    assert!(has(&findings, "E113"), "{findings:?}");
}

#[test]
fn bad_task_state_is_e112() {
    let mut d = base();
    d["tasks"][0]["state"] = serde_json::json!("zombie");
    let (_, findings) = run_check(&d.to_string());
    assert!(has(&findings, "E112"), "{findings:?}");
}

#[test]
fn canceled_is_a_task_level_state_with_or_without_history() {
    let mut with_history = base();
    with_history["tasks"][0]["state"] = serde_json::json!("canceled");
    let (code, findings) = run_check(&with_history.to_string());
    assert_eq!(code, 0, "historical attempts remain valid: {findings:?}");
    assert!(findings.is_empty(), "{findings:?}");

    let mut before_start = with_history;
    before_start["tasks"][0]["attempts"] = serde_json::json!([]);
    let (code, findings) = run_check(&before_start.to_string());
    assert_eq!(code, 0, "cancel-before-start is valid: {findings:?}");
    assert!(findings.is_empty(), "{findings:?}");
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

#[test]
fn legacy_action_data_is_readable_but_inert_in_v1_v2_and_v3() {
    for version in [1, 2, 3] {
        let mut d = base();
        d["dagr"] = serde_json::json!(version);
        d["actions"] = serde_json::json!({
            "unblock": {"argv": ["/tmp/must-not-run", "{task}"]},
            "arbitrary-old-shape": [false, 42, {"shell": "touch sentinel"}]
        });
        let (code, findings) = run_check(&d.to_string());
        assert_eq!(code, 0, "v{version} legacy data must remain readable: {findings:?}");
        assert!(findings.is_empty(), "v{version}: {findings:?}");
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!root.join("src/action.rs").exists(), "the producer executor must not ship");
    let view = std::fs::read_to_string(root.join("src/view.rs")).unwrap();
    assert!(!view.contains("start_action"), "legacy argv must have no interaction binding");
    assert!(!view.contains("std::process::Command"), "the viewer must not spawn arbitrary commands");
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
    assert_eq!(v["settled"], 6, "canceled is terminal for flow stats: {v}");
    assert!(v["tasks"].as_array().is_some_and(|t| t.len() == 13));
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

#[test]
fn compact_snapshot_matches_the_interactive_browse_inspector() {
    let (full_code, full) = run_snapshot(STATES, &["--width", "72", "--select", "G1"]);
    let (compact_code, compact) =
        run_snapshot(STATES, &["--compact", "--width", "72", "--select", "G1"]);
    assert_eq!(full_code, 0);
    assert_eq!(compact_code, 0);

    let full = strip_ansi(&full);
    let compact = strip_ansi(&compact);
    assert!(
        full.contains("┌─ G1 · WAITING"),
        "legacy full snapshot changed:\n{full}"
    );
    assert!(
        compact.lines().any(|line| {
            line.starts_with('╭') && line.contains("G1") && line.contains("○ WAITING")
        }),
        "compact snapshot did not use the browse inspector:\n{compact}"
    );
    assert!(
        compact
            .lines()
            .any(|line| line.starts_with('╰') && line.ends_with('╯')),
        "compact metadata border missing:\n{compact}"
    );
    assert!(
        !compact.contains("┌─ G1 · WAITING"),
        "compact capture must not fall back to the full detail card:\n{compact}"
    );
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

    let (_, selected) = run_snapshot(STATES, &["--width", "150", "--select", "T4"]);
    assert!(
        strip_ansi(&selected).contains("T4·a1 · attempt 1 · BLOCKED"),
        "focus card must use the same effective state as the row:\n{selected}"
    );
}

#[test]
fn canceled_renders_as_terminal_task_truth() {
    let (_, out) = run_snapshot(STATES, &["--width", "200", "--select", "T11"]);
    let plain = strip_ansi(&out);
    let row = plain.lines().find(|l| l.contains("× T11")).expect("canceled row");
    assert!(row.contains("canceled"), "{row:?}");
    assert!(plain.contains("T11 · CANCELED"), "focus card agrees with row:\n{plain}");
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
    let g1 = plain.lines().find(|l| l.contains("◎ G1")).expect("G1 row");
    let g2 = plain.lines().find(|l| l.contains("◎ G2")).expect("G2 row");
    assert!(g1.contains("●◎?→◎ G1"), "G1 shows ordered done/working/lost inputs:\n{g1}");
    assert!(g2.contains("●→◎ G2"), "attempted G2 keeps the join:\n{g2}");
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

fn in_nested_project(doc: &str, depth: usize) -> String {
    let mut value: serde_json::Value = serde_json::from_str(doc).unwrap();
    value["dagr"] = serde_json::json!(2);
    value["projects"] = serde_json::Value::Array(
        (0..depth)
            .map(|i| {
                let mut project = serde_json::json!({"id": format!("P{i}"), "title": format!("Level {i}")});
                if i > 0 {
                    project["parent"] = serde_json::json!(format!("P{}", i - 1));
                }
                project
            })
            .collect(),
    );
    let home = format!("P{}", depth - 1);
    for task in value["tasks"].as_array_mut().unwrap() {
        task["project"] = serde_json::json!(home);
    }
    value.to_string()
}

#[test]
fn join_strip_degrades_from_inputs_to_counts_to_total() {
    let doc = in_nested_project(&seven_lane_join_doc(), 4);
    let (_, wide) = run_snapshot(&doc, &["--width", "78"]);
    let (_, compact) = run_snapshot(&doc, &["--width", "28"]);
    let (_, tiny) = run_snapshot(&doc, &["--width", "20"]);
    assert!(strip_ansi(&wide).contains("○○○○○○○→◎ G"), "wide strip:\n{wide}");
    assert!(strip_ansi(&compact).contains("○7→◎ G"), "counted strip:\n{compact}");
    assert!(strip_ansi(&tiny).contains("7→1 ◎ G"), "tiny strip:\n{tiny}");
}

#[test]
fn deep_scope_milestones_keep_the_join_and_useful_identity() {
    let compact_doc = in_nested_project(&seven_lane_join_doc_at_depth(1, "GATE-LONG"), 10);
    let (_, tiny) = run_snapshot(&compact_doc, &["--width", "20"]);
    let plain = strip_ansi(&tiny);
    let gate = plain.lines().find(|line| line.contains("◎ GATE")).expect("gate row");
    assert!(plain.lines().any(|line| line.contains("▾ ○ P9")), "deep project node and identity survive:\n{plain}");
    assert!(gate.contains("7→1 ◎ GATE"), "join and useful id prefix survive:\n{gate}");

    let full_doc = in_nested_project(&seven_lane_join_doc_at_depth(1, "GATE-LONG"), 30);
    let (_, full) = run_snapshot(&full_doc, &["--width", "96"]);
    let plain = strip_ansi(&full);
    let gate = plain.lines().find(|line| line.contains("◎ GATE")).expect("gate row");
    assert!(plain.lines().any(|line| line.contains("▾ ○ P29")), "deep project node and identity survive:\n{plain}");
    assert!(
        gate.contains("○7→◎ GATE-LONG"),
        "counted join and useful id survive the deep full layout:\n{gate}"
    );
}

#[test]
fn join_identity_survives_narrow_normal_and_deep_snapshots_in_both_glyph_modes() {
    let doc = in_nested_project(&seven_lane_join_doc_at_depth(1, "GATE-LONG"), 12);
    for (fallback, glyph) in [(None, '◎'), (Some("*"), '*')] {
        for width in [20usize, 96, 200] {
            let (code, output) = run_snapshot_with_working_glyph(
                &doc,
                &["--width", &width.to_string()],
                fallback,
            );
            assert_eq!(code, 0, "width={width} fallback={fallback:?}");
            let plain = strip_ansi(&output);
            let gate = plain
                .lines()
                .find(|line| line.contains(&format!("{glyph} GATE")))
                .unwrap_or_else(|| panic!("gate identity missing at width={width}:\n{plain}"));
            let expected = match width {
                20 => format!("7→1 {glyph} GATE"),
                96 | 200 => format!("○○○○○○○→{glyph} GATE-LONG"),
                _ => unreachable!(),
            };
            assert!(gate.contains(&expected), "width={width}: expected {expected:?}: {gate}");
            assert!(
                plain.lines().any(|line| line.contains("P11")),
                "deep project identity missing at width={width}:\n{plain}"
            );
        }
    }
}

#[test]
fn snapshot_escapes_terminal_controls_and_rejects_oversized_geometry() {
    let hostile = serde_json::json!({
        "dagr": 2,
        "run": {"id": "escape", "title": "head\n\u{1b}[31mPWN\u{202e}"},
        "tasks": [{
            "id": "T", "title": "body\t\u{1b}]8;;bad\u{7}link\u{202e}",
            "kind": "impl", "state": "queued", "deps": [], "attempts": []
        }]
    })
    .to_string();
    let (code, output) = run_snapshot(&hostile, &["--width", "96"]);
    assert_eq!(code, 0);
    let plain = strip_ansi(&output);
    assert!(plain.contains("head\\n\\x1b[31mPWN\\u{202e}"), "{plain}");
    assert!(plain.contains("body\\t\\x1b]8;;bad\\x07link\\u{202e}"), "{plain}");

    let (code, _) = run_snapshot(&base().to_string(), &["--width", "4097"]);
    assert_eq!(code, 2, "absurd frame widths fail closed before allocation");

    let tasks: Vec<_> = (0..=4096)
        .map(|i| serde_json::json!({"id": format!("T{i}"), "attempts": []}))
        .collect();
    let too_many = serde_json::json!({"dagr": 2, "run": {"id": "many"}, "tasks": tasks});
    let (code, _) = run_snapshot(&too_many.to_string(), &["--width", "80"]);
    assert_eq!(code, 1, "oversized documents fail before graph traversal");
}

#[test]
fn off_tree_annotations_render() {
    let (_, out) = run_snapshot(STATES, &["--width", "200"]);
    let plain = strip_ansi(&out);
    assert!(plain.contains("⇠ T3") && plain.contains("⇠ T7"), "extra deps beyond the rail get ⇠ chips");
    assert!(plain.contains("⇠ resync@G1"), "the task note is ink, not dead data");

    let (_, compact) = run_snapshot(STATES, &["--width", "94"]);
    assert!(
        strip_ansi(&compact).contains("⇠"),
        "compact rows preserve relational ink when space exists:\n{compact}"
    );
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
