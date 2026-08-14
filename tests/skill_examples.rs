//! The producer skill's recipes are executable, not illustrative: every
//! JSON example shipped with the skill must be contract-clean under
//! `--strict` (zero errors AND zero warnings). A recipe that plants
//! findings in a follower's file is a defect in the skill.
//! The same bar holds for every checked-in run document the repo points
//! at — the acceptance demos and samples ARE the claim that this system
//! produces clean documents, so editing one into a finding-producing
//! state must fail the suite.

use std::process::Command;

fn dagr() -> &'static str {
    env!("CARGO_BIN_EXE_dagr")
}

fn assert_dir_strict_clean(rel_dir: &str, floor: usize) {
    let dir = format!("{}/{rel_dir}", env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort();
    for path in entries {
        let out = Command::new(dagr())
            .args(["check", path.to_str().unwrap(), "--strict", "--json"])
            .output()
            .expect("dagr check failed to spawn");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.trim() == "[]",
            "{} is not strict-clean:\n{}",
            path.display(),
            stdout
        );
        checked += 1;
    }
    assert!(checked >= floor, "{rel_dir}: expected at least {floor} documents, found {checked}");
}

#[test]
fn every_skill_example_is_strict_clean() {
    assert_dir_strict_clean("skills/dagr-producer/examples", 8);
}

#[test]
fn every_sample_is_strict_clean() {
    assert_dir_strict_clean("samples", 2);
}

#[test]
fn the_selfrun_acceptance_demo_is_strict_clean() {
    assert_dir_strict_clean("demos/selfrun", 1);
}

#[test]
fn the_actions_demo_fixture_is_strict_clean() {
    assert_dir_strict_clean("demos/actions", 1);
}
