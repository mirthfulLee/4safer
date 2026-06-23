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
    assert!(stdout.contains("--max-sequences <COUNT>"));
    assert!(stdout.contains("--max-sequences-per-wrapper <COUNT>"));
    assert!(stdout.contains("--max-sequences-per-target <COUNT>"));
    assert!(stdout.contains("--floor-sequences-per-target <COUNT>"));
    assert!(stdout.contains("--wrapper-target-max-distance <DISTANCE>"));
    assert!(stdout.contains("--disable-ranking"));
    assert!(stdout.contains("--r-flow <VALUE>"));
    assert!(stdout.contains("--w-c <VALUE>"));
    assert!(stdout.contains("--w-p <VALUE>"));
    assert!(stdout.contains("--disable-reuse-penalty"));
    assert!(stdout.contains("--disable-depth-penalty"));
    assert!(!stdout.contains("--disable-sequence-limits"));
    assert!(!stdout.contains("--disable-budget-limit"));
    assert!(!stdout.contains("--unbudgeted"));
    assert!(!stdout.contains("--max-basic-sequences-per-target"));
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
fn cargo_4safer_limits_default_planner_by_wrapper_sequence_count() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-indirect-field-lib",
        "sequence-capped-harness",
        true,
        &["--max-sequences-per-wrapper", "1"],
    );
    run.assert_success();

    let sequence_metadata = run.sequence_metadata("unsafe_wrapper_indirect_field_lib");
    assert_eq!(
        sequence_metadata.len(),
        1,
        "expected wrapper sequence cap to retain exactly one sequence"
    );
}

#[test]
fn cargo_4safer_accepts_ranking_ablation_flag() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-indirect-field-lib",
        "ranking-ablation-harness",
        true,
        &["--disable-ranking"],
    );
    run.assert_success();

    let sequence_metadata = run.sequence_metadata("unsafe_wrapper_indirect_field_lib");
    assert!(
        !sequence_metadata.is_empty(),
        "expected ranking-ablation synthesis to emit sequences"
    );
}

#[test]
fn cargo_4safer_ranking_ablation_keeps_sequence_caps() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-indirect-field-lib",
        "ranking-ablation-capped-harness",
        true,
        &["--disable-ranking", "--max-sequences-per-wrapper", "1"],
    );
    run.assert_success();

    let sequence_metadata = run.sequence_metadata("unsafe_wrapper_indirect_field_lib");
    assert_eq!(
        sequence_metadata.len(),
        1,
        "ranking ablation should preserve wrapper sequence caps"
    );
}

#[test]
fn cargo_4safer_fuzz_backend_treats_fallible_outputs_as_discarded_inputs() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-multi-lib",
        "unsafe-wrapper-multi-fuzz-fallible-harness",
        true,
        &["--backend", "fuzz"],
    );
    run.assert_success();

    let fuzz_targets = run.generated_fuzz_targets();
    assert!(
        fuzz_targets.iter().any(|target| {
            target.contains("unsafe_wrapper_multi_lib::Counter::recover(")
                && target.contains("::core::result::Result::Ok(value) => value")
                && target.contains(
                    "::core::result::Result::Err(_) => return ::core::option::Option::None",
                )
        }),
        "expected fuzz Result adapters to discard Err inputs instead of crashing"
    );
    assert!(
        fuzz_targets.iter().any(|target| {
            target.contains("unsafe_wrapper_multi_lib::Counter::restore(")
                && target.contains("::core::option::Option::Some(value) => value")
                && target
                    .contains("::core::option::Option::None => return ::core::option::Option::None")
        }),
        "expected fuzz Option adapters to discard None inputs instead of crashing"
    );
    assert!(
        fuzz_targets
            .iter()
            .all(|target| !target.contains("fallible API returned")),
        "expected fuzz targets not to classify ordinary Err/None as crashes"
    );
    assert!(
        fuzz_targets.iter().any(|target| {
            target.contains("fn main()")
                && target.contains("afl::fuzz_nohook!(|data: &[u8]|")
                && target.contains("::std::panic::set_hook")
                && target.contains("::std::panic::catch_unwind")
                && target.contains("::std::panic::AssertUnwindSafe")
        }),
        "expected fuzz targets to keep target panics from aborting AFL seed validation"
    );
}

#[test]
fn cargo_4safer_tests_backend_includes_non_ascii_literal_seeds() {
    let run = run_4safer_on_fixture(
        "unsafe-wrapper-symbolic-std-lib",
        "symbolic-std-literal-seeds-harness",
        true,
        &["--max-sequences", "0"],
    );
    run.assert_success();

    let generated_sources = fs::read_dir(run.harness_dir.join("src"))
        .expect("generated src directory should exist")
        .map(|entry| {
            let path = entry
                .expect("generated source entry should be readable")
                .path();
            fs::read_to_string(path).expect("generated source should be readable")
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        generated_sources.contains("'🦀'"),
        "expected char literal seeds to include a non-ASCII emoji:\n{generated_sources}"
    );
    assert!(
        generated_sources.contains("[240u8, 159u8, 166u8, 128u8, 0u8, 0u8, 0u8, 0u8], 4usize"),
        "expected string-like literal seeds to include an emoji UTF-8 byte sequence:\n{generated_sources}"
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
