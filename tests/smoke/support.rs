use serde::Deserialize;
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
pub(super) struct SequenceMetadataView {}

pub(super) struct HarnessRun {
    _temp: TempDir,
    pub(super) harness_dir: PathBuf,
    output: Output,
}

impl HarnessRun {
    pub(super) fn assert_success(&self) {
        assert!(
            self.output.status.success(),
            "cargo-4safer exited with {}\nstdout:\n{}\nstderr:\n{}",
            self.output.status,
            String::from_utf8_lossy(&self.output.stdout),
            String::from_utf8_lossy(&self.output.stderr)
        );
    }

    pub(super) fn manifest(&self) -> String {
        fs::read_to_string(self.harness_dir.join("Cargo.toml"))
            .expect("generated manifest should exist")
    }

    pub(super) fn lockfile(&self) -> String {
        fs::read_to_string(self.harness_dir.join("Cargo.lock"))
            .expect("generated harness lockfile should exist")
    }

    pub(super) fn sequence_metadata(&self, crate_name: &str) -> Vec<SequenceMetadataView> {
        read_json(
            self.harness_dir
                .join(format!("foursafer_meta/{}_api_sequences.json", crate_name)),
        )
    }
}

pub(super) fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

pub(super) fn fixture_manifest_path(name: &str) -> PathBuf {
    fixture_dir(name).join("Cargo.toml")
}

pub(super) fn run_4safer_on_fixture(
    fixture_name: &str,
    harness_name: &str,
    check_harness: bool,
    extra_args: &[&str],
) -> HarnessRun {
    let mut args = vec![
        OsString::from("--dir"),
        fixture_dir(fixture_name).into_os_string(),
    ];
    args.extend(extra_args.iter().map(OsString::from));
    run_cargo_4safer(harness_name, check_harness, args)
}

pub(super) fn run_4safer_on_manifest(
    fixture_name: &str,
    harness_name: &str,
    check_harness: bool,
    extra_args: &[&str],
) -> HarnessRun {
    let mut args = vec![
        OsString::from("--manifest-path"),
        fixture_manifest_path(fixture_name).into_os_string(),
    ];
    args.extend(extra_args.iter().map(OsString::from));
    run_cargo_4safer(harness_name, check_harness, args)
}

pub(super) fn read_json<T>(path: PathBuf) -> T
where
    T: for<'de> Deserialize<'de>,
    T: Debug,
{
    let raw = fs::read_to_string(&path).expect("json file should exist");
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
}

pub(super) fn run_4safer_raw(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-4safer"))
        .args(args)
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH")
        .output()
        .expect("cargo-4safer should execute")
}

fn run_cargo_4safer(
    harness_name: &str,
    check_harness: bool,
    mut args: Vec<OsString>,
) -> HarnessRun {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let harness_dir = temp.path().join(harness_name);
    let target_dir = temp.path().join("target");
    let passthrough_index = args.iter().position(|arg| arg == "--");
    let passthrough = passthrough_index.map(|index| args.split_off(index));

    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-4safer"));
    command.arg("4safer");
    command.args(args);
    if check_harness {
        command.arg("--check-harness");
    }
    command
        .arg("--harness-crate")
        .arg(&harness_dir)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH");
    if let Some(passthrough) = passthrough {
        command.args(passthrough);
    }

    let output = command.output().expect("cargo-4safer should execute");
    HarnessRun {
        _temp: temp,
        harness_dir,
        output,
    }
}
