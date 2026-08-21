use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

fn repo_file(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

#[test]
fn package_plugin_contract_and_binary_versions_are_0_3() {
    assert!(repo_file("Cargo.toml").contains("version = \"0.3.0\""));
    assert!(repo_file("herdr-plugin.toml").contains("version = \"0.3.0\""));
    let cargo_lock = repo_file("Cargo.lock").replace("\r\n", "\n");
    assert!(cargo_lock.contains("name = \"dagr\"\nversion = \"0.3.0\""));
    assert!(repo_file("CONTRACT.md").starts_with("# The dagr run-state contract — v3"));

    let output = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .arg("--version")
        .output()
        .expect("dagr runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "dagr 0.3.0 (contract v3; reads v1/v2)"
    );
}

#[test]
fn release_workflow_has_exactly_five_checksum_assets() {
    let workflow = repo_file(".github/workflows/release.yml");
    let targets: BTreeSet<String> = workflow
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("- { target: ")
                .and_then(|rest| rest.split(',').next())
                .map(str::to_string)
        })
        .collect();
    let expected: BTreeSet<String> = [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-musl",
        "x86_64-unknown-linux-musl",
        "x86_64-pc-windows-msvc",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(targets, expected);
    let artifacts: BTreeSet<String> = workflow
        .lines()
        .filter_map(|line| {
            line.split("artifact: ")
                .nth(1)
                .and_then(|rest| rest.split([' ', '}']).next())
                .map(str::to_string)
        })
        .collect();
    let expected_artifacts: BTreeSet<String> = [
        "dagr-aarch64-apple-darwin.tar.gz",
        "dagr-x86_64-apple-darwin.tar.gz",
        "dagr-aarch64-unknown-linux-musl.tar.gz",
        "dagr-x86_64-unknown-linux-musl.tar.gz",
        "dagr-x86_64-pc-windows-msvc.zip",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(artifacts, expected_artifacts);
    assert_eq!(workflow.matches("checksum: sha256").count(), 1);
    assert!(workflow.contains("git rev-parse HEAD > COMMIT"));
    assert!(workflow.contains("needs: binaries"), "publication waits for every target");
}

#[test]
fn installers_are_exact_release_checksum_verified_atomic_and_fallback_capable() {
    let unix = repo_file("scripts/install.sh");
    let windows = repo_file("scripts/install.ps1");
    for target in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-musl",
        "x86_64-unknown-linux-musl",
    ] {
        assert!(unix.contains(target), "Unix installer omitted {target}");
    }
    assert!(windows.contains("x86_64-pc-windows-msvc"));
    for installer in [&unix, &windows] {
        assert!(installer.contains("COMMIT"));
        assert!(installer.contains(".sha256"));
        assert!(installer.contains("checksum mismatch"));
        assert!(installer.contains("does not match the"));
        assert!(installer.contains("building this source with Cargo"));
    }
    assert!(unix.contains("mv -f \"$stage\" \"$bin_dir/$name\""));
    assert!(windows.contains("[System.IO.File]::Replace"));
    assert!(!unix.contains("verify_binary_target"));
    assert!(!windows.contains("verify-binary-target"));
}

#[test]
fn public_stage_copies_the_release_surface_deliberately() {
    if !Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/build-public-tree.sh")
        .is_file()
    {
        return;
    }
    let stage = repo_file("scripts/build-public-tree.sh");
    for path in [
        "scripts/install.sh",
        "scripts/install.ps1",
        ".github/workflows/release.yml",
        ".github/workflows/ci.yml",
        "Cargo.toml",
        "Cargo.lock",
        "herdr-plugin.toml",
        "CONTRACT.md",
    ] {
        assert!(stage.contains(path), "public stage omitted {path}");
    }
    assert!(stage.contains("--stage-only"));
    assert!(!stage.contains("demos/actions"));
}
