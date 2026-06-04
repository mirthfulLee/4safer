use super::support::{run_4safer_on_fixture, run_4safer_on_manifest, run_4safer_raw};
use std::fs;

#[test]
fn cargo_4safer_help_lists_common_flags() {
    let output = run_4safer_raw(&["4safer", "-h"]);
    assert!(
        output.status.success(),
        "expected `cargo 4safer -h` to succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cargo 4safer"));
    assert!(stdout.contains("-C, --dir <DIR>"));
    assert!(stdout.contains("-o, --out <PATH>"));
    assert!(stdout.contains("-b, --backend <BACKEND>"));
    assert!(stdout.contains("tests"));
    assert!(stdout.contains("kani"));
    assert!(stdout.contains("fuzz"));
    assert!(stdout.contains("--disable-budget-limit"));
    assert!(stdout.contains("--disable-reuse-penalty"));
    assert!(stdout.contains("--disable-depth-penalty"));
    assert!(!stdout.contains("--strategy"));
    assert!(!stdout.contains("unsafe-wrapper"));
}

#[test]
fn cargo_4safer_defaults_to_tests_backend_and_writes_metadata() {
    let run = run_4safer_on_fixture("basic-lib", "harness", true, &[]);
    run.assert_success();

    let sequence_meta = run
        .harness_dir
        .join("foursafer_meta/basic_lib_api_sequences.json");
    let api_meta = run
        .harness_dir
        .join("foursafer_meta/basic_lib_api_functions.json");
    let harness_lib = run.harness_dir.join("src/lib.rs");

    assert!(
        sequence_meta.exists(),
        "missing {}",
        sequence_meta.display()
    );
    assert!(api_meta.exists(), "missing {}", api_meta.display());
    assert!(harness_lib.exists(), "missing {}", harness_lib.display());

    let lib_rs = fs::read_to_string(&harness_lib).expect("generated lib.rs should exist");
    assert!(
        !lib_rs.contains("#![cfg(kani)]"),
        "default backend should be literal Rust tests, not Kani-gated harnesses"
    );

    let manifest = run.manifest();
    assert!(manifest.contains("name = \"foursafer_test\""));
    assert!(manifest.contains("[lib]\npath = \"src/lib.rs\""));

    let sequence_metadata = run.sequence_metadata("basic_lib");
    assert!(
        sequence_metadata.is_empty(),
        "safe-only fixture should have no obligation targets"
    );
}

#[test]
fn cargo_4safer_accepts_budget_ablation_flag() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-indirect-field-lib",
        "unbudgeted-harness",
        true,
        &["--disable-budget-limit"],
    );
    run.assert_success();

    let sequence_metadata = run.sequence_metadata("unsafe_wrapper_indirect_field_lib");
    assert!(
        !sequence_metadata.is_empty(),
        "expected unbudgeted obligation-driven synthesis to emit sequences"
    );
}

#[test]
fn cargo_4safer_accepts_manifest_path() {
    let run = run_4safer_on_manifest("basic-lib", "manifest-harness", true, &[]);
    run.assert_success();

    assert!(
        run.sequence_metadata("basic_lib").is_empty(),
        "manifest-path flow should preserve safe-only empty synthesis"
    );
}

#[test]
fn cargo_4safer_supports_workspace_package_selection() {
    let run = run_4safer_on_fixture(
        "workspace",
        "workspace-harness",
        true,
        &["--package", "target-lib"],
    );
    run.assert_success();

    let harness_lock = run.lockfile();
    assert!(
        harness_lock.contains("[[package]]\nname = \"target-lib\"\nversion = \"0.1.0\""),
        "expected generated harness lockfile to include selected workspace member:\n{harness_lock}"
    );
    assert!(
        harness_lock.contains("[[package]]\nname = \"foursafer_test\"\nversion = \"0.1.0\"\ndependencies = [\n \"target-lib\",\n]"),
        "expected generated harness lockfile to include generated harness package:\n{harness_lock}"
    );
}
