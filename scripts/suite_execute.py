#!/usr/bin/env python3
"""Execute generated cargo-4safer backend harnesses.

This script consumes generation results produced by suite_synthesize.py and
executes the generated harness crates per crate/backend. Execution results are
kept separate from synthesis metrics so failed backend runs do not overwrite the
synthesis records.
"""

import argparse
import csv
import json
import os
import re
import signal
import shlex
import shutil
import subprocess
import sys
import threading
import time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from experiment_common import (
    DEFAULT_EXPERIMENT,
    DEFAULT_EXPERIMENT_CONFIG,
    DEFAULT_EXPERIMENT_ROOT,
    REPO,
    configured_path,
    generation_results_path,
    load_experiment_config,
    repo_path,
)

DEFAULT_GENERATION = DEFAULT_EXPERIMENT_ROOT / "runs" / DEFAULT_EXPERIMENT / "all_results.csv"
DEFAULT_OUT = DEFAULT_EXPERIMENT_ROOT / "backend-execution"
DEFAULT_KANI_REPO = "https://github.com/model-checking/kani.git"
DEFAULT_KANI_DIR = repo_path(
    os.environ.get("KANI_DIR"),
    DEFAULT_EXPERIMENT_ROOT / "tools" / "kani",
)
BACKENDS = ("tests", "fuzz", "kani")
AFL_LEGACY_SAFE_SEED_GUARD = '        if data == b"\\0" {\n            return;\n        }\n'
AFL_FUZZ_ENTRY = "    afl::fuzz!(|data: &[u8]| {\n"
FUZZ_CRASH_REPLAY_TIMEOUT = 10
FUZZ_SEED_VALIDATION_TIMEOUT = 10


def configured_generation_results(config, execute, experiment):
    if execute.get("generation_results") is not None:
        return repo_path(execute.get("generation_results"), DEFAULT_GENERATION)
    return generation_results_path(config, experiment, DEFAULT_GENERATION)


def apply_run_defaults(args, require_backend=True):
    config = load_experiment_config(args.experiment_config)
    execute = config.get("execute", {})
    if not isinstance(execute, dict):
        raise ValueError(f"{args.experiment_config} execute must be an object")
    runs = execute.get("runs", {})
    if not isinstance(runs, dict):
        raise ValueError(f"{args.experiment_config} execute.runs must be an object")

    run = runs.get(args.run) if args.run else None
    if args.run and run is None:
        raise ValueError(f"{args.experiment_config} does not define execute run {args.run!r}")
    if run is None:
        run = {}
    if not isinstance(run, dict):
        raise ValueError(f"{args.experiment_config} execute run {args.run!r} must be an object")

    if args.experiment is None:
        args.experiment = config.get("experiment") or DEFAULT_EXPERIMENT
    if args.generation_results is None:
        args.generation_results = configured_generation_results(config, execute, args.experiment)
    if args.out is None:
        args.out = repo_path(execute.get("out"), configured_path(config, "execution_out", DEFAULT_OUT))
    if args.config is None:
        args.config = run.get("config") or args.experiment
    args.kani_verify_cmd = run.get("verify_cmd") or execute.get("verify_cmd")
    for name, default in (
        ("backend", None),
        ("timeout", 0),
        ("test_timeout", 300),
        ("test_jobs", 1),
        ("tests_runner", "asan"),
        ("sanitizer", "address"),
        ("sanitizer_target_triple", "x86_64-unknown-linux-gnu"),
        ("fuzz_targets", "first"),
        ("fuzz_jobs", 1),
        ("fuzz_time_limit", 0),
        ("fuzz_target_timeout", 0),
        ("fuzz_sanitizer", "address"),
        ("kani_mode", "config"),
        ("kani_command", None),
        ("kani_repo", DEFAULT_KANI_REPO),
        ("kani_jobs", 1),
        ("kani_proof_timeout", 1200),
        ("kani_extra_args", ""),
    ):
        if getattr(args, name) is None:
            setattr(args, name, run.get(name, default))
    if args.kani_dir is None:
        args.kani_dir = repo_path(run.get("kani_dir"), DEFAULT_KANI_DIR)
    for name in (
        "force",
        "keep_target",
        "prepare_package_tmp",
        "fuzz_exit_when_done",
        "fuzz_stop_on_ub",
        "reuse_fuzz_target_results",
        "build_kani",
        "prepare_kani",
    ):
        if getattr(args, name) is None:
            setattr(args, name, bool(run.get(name, default_bool_for_execution_flag(name))))
    if require_backend and args.backend is None:
        raise ValueError("--backend is required unless selected by --run")


def default_bool_for_execution_flag(name):
    return {
        "force": False,
        "keep_target": False,
        "prepare_package_tmp": True,
        "fuzz_exit_when_done": False,
        "fuzz_stop_on_ub": True,
        "reuse_fuzz_target_results": True,
        "build_kani": False,
        "prepare_kani": False,
    }[name]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--experiment-config", type=Path, default=DEFAULT_EXPERIMENT_CONFIG)
    parser.add_argument("--run", help="Named execute.runs entry from --experiment-config.")
    parser.add_argument("--generation-results", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--experiment")
    parser.add_argument("--config")
    parser.add_argument("--backend", choices=BACKENDS)
    parser.add_argument("--crate", action="append", default=[], help="Crate name or crate-version key.")
    parser.add_argument("--timeout", type=int)
    parser.add_argument("--test-timeout", type=int)
    parser.add_argument("--test-jobs", type=int, help="Number of generated tests to execute in parallel per crate.")
    parser.add_argument(
        "--tests-runner",
        choices=("asan", "sanitizer", "native", "miri"),
        help="Execution mode for generated tests. Defaults to ASan; native is plain execution.",
    )
    parser.add_argument(
        "--sanitizer",
        help="Sanitizer passed to -Zsanitizer when --tests-runner sanitizer is used; asan implies address.",
    )
    parser.add_argument("--sanitizer-target-triple")
    parser.add_argument("--force", action="store_true", default=None)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--keep-target", action="store_true", default=None)
    parser.add_argument("--prepare-package-tmp", action="store_true", default=None)
    parser.add_argument("--no-prepare-package-tmp", dest="prepare_package_tmp", action="store_false")
    parser.add_argument("--env", action="append", default=[], help="Extra environment override, KEY=VALUE.")
    parser.add_argument("--fuzz-targets", choices=("first", "all"))
    parser.add_argument("--fuzz-jobs", type=int, help="Number of fuzz targets to execute in parallel per crate.")
    parser.add_argument("--fuzz-time-limit", type=int, help="Per-fuzz-target AFL -V time limit in seconds; 0 means use --timeout.")
    parser.add_argument("--fuzz-target-timeout", type=int, help="Outer timeout in seconds for each fuzz target command; 0 means AFL -V time plus 600s.")
    parser.add_argument("--fuzz-exit-when-done", action="store_true", default=None, help="Set AFL_EXIT_WHEN_DONE=1 so AFL can stop after exhausting its queue before -V.")
    parser.add_argument("--no-fuzz-exit-when-done", dest="fuzz_exit_when_done", action="store_false")
    parser.add_argument("--fuzz-stop-on-ub", action="store_true", default=None, help="Stop the current fuzz target early after the first high-confidence UB finding.")
    parser.add_argument("--no-fuzz-stop-on-ub", dest="fuzz_stop_on_ub", action="store_false")
    parser.add_argument("--reuse-fuzz-target-results", action="store_true", default=None, help="When rerunning with --force, reuse existing non-skipped fuzz target results and only execute missing/skipped targets.")
    parser.add_argument("--no-reuse-fuzz-target-results", dest="reuse_fuzz_target_results", action="store_false")
    parser.add_argument(
        "--fuzz-sanitizer",
        choices=("none", "address", "memory", "thread"),
        help="Sanitizer used for AFL fuzz targets. Defaults to address (ASan); use none for native AFL.",
    )
    parser.add_argument(
        "--kani-mode",
        choices=("config", "codegen", "proof", "official-proof"),
        help=(
            "Kani execution mode. config uses inline verify_cmd; "
            "codegen/proof use --kani-command; official-proof runs each generated "
            "kani::proof harness with cargo kani."
        ),
    )
    parser.add_argument(
        "--kani-command",
        default=None,
        help="Kani command for codegen/proof mode, or override for the first verify_cmd token.",
    )
    parser.add_argument("--kani-repo")
    parser.add_argument("--kani-dir", type=Path)
    parser.add_argument("--build-kani", action="store_true", default=None)
    parser.add_argument("--prepare-kani", action="store_true", default=None, help="Clone/build Kani under --kani-dir and prepend its cargo-kani to PATH.")
    parser.add_argument("--no-prepare-kani", dest="prepare_kani", action="store_false")
    parser.add_argument("--kani-jobs", type=int, help="Number of Kani proof harnesses to execute in parallel per crate.")
    parser.add_argument("--kani-proof-timeout", type=int, help="Per-Kani-proof timeout in seconds for --kani-mode official-proof.")
    parser.add_argument("--kani-arg", action="append", default=[], help="Extra argument passed to each official cargo kani invocation.")
    parser.add_argument("--kani-extra-args", help="Shell-style extra arguments passed to each official cargo kani invocation.")
    parser.add_argument("--aggregate-existing", action="store_true")
    args = parser.parse_args()

    args.experiment_config = args.experiment_config.resolve()
    try:
        apply_run_defaults(args, require_backend=not args.aggregate_existing)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2

    args.out = args.out.resolve()
    args.kani_dir = args.kani_dir.resolve()

    if args.aggregate_existing:
        rows = gather_existing_results(args.out)
        write_summaries(rows, args.out)
        write_experiment_summaries(rows, args.out)
        return 0

    if args.backend is None:
        parser.error("--backend is required unless --aggregate-existing is used")
    if (
        args.backend == "kani"
        and args.kani_mode != "official-proof"
        and args.prepare_kani
        and not args.dry_run
    ):
        prepare_kani(args)

    generation_rows = load_generation_rows(args)
    if not generation_rows:
        print("no matching generated harnesses", file=sys.stderr)
        return 2

    rows = []
    for completed, generation in enumerate(generation_rows, start=1):
        row = execute_one_generation(generation, args, completed, len(generation_rows))
        if row is not None:
            rows.append(row)
            if not args.dry_run:
                existing = gather_existing_results(args.out)
                write_summaries(existing, args.out)
                write_experiment_summaries(existing, args.out)

    if rows and not args.dry_run:
        existing = gather_existing_results(args.out)
        write_summaries(existing, args.out)
        write_experiment_summaries(existing, args.out)
    return 0


def execute_one_generation(generation, args, completed, total):
    key = crate_key(generation)
    print(
        f"[{completed}/{total}] execute {key} "
        f"{generation['experiment']}/{generation['config']} backend={result_backend(args)}",
        flush=True,
    )
    try:
        return run_backend(generation, args)
    except Exception as error:
        print(f"[execute-error] {key}: {error}", file=sys.stderr, flush=True)
        return None


def load_generation_rows(args):
    with args.generation_results.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))
    requested = set(args.crate)
    selected = []
    for row in rows:
        if row.get("experiment") != args.experiment:
            continue
        if row.get("config") != args.config:
            continue
        if row.get("backend") != args.backend:
            continue
        if row.get("status") != "ok" or row.get("generation_status") != "generated":
            continue
        if requested and row.get("crate") not in requested and crate_key(row) not in requested:
            continue
        selected.append(row)
    selected.sort(key=lambda row: (int(row.get("csv_index") or 0), crate_key(row)))
    return selected


def run_backend(generation, args):
    key = crate_key(generation)
    run_dir = args.out / generation["experiment"] / result_backend(args) / key
    result_path = run_dir / "result.json"
    if result_path.exists() and not args.force:
        return json.loads(result_path.read_text())

    if (
        args.force
        and run_dir.exists()
        and not (args.backend == "kani" and args.kani_mode == "official-proof")
        and not (args.backend == "fuzz" and args.reuse_fuzz_target_results)
    ):
        shutil.rmtree(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)

    harness = Path(generation["harness"])
    manifest = harness / "Cargo.toml"
    if not manifest.exists():
        raise RuntimeError(f"missing generated harness manifest: {manifest}")

    if args.prepare_package_tmp and not args.dry_run:
        prepare_package_tmp_dirs(manifest)

    env = os.environ.copy()
    apply_default_dynamic_library_paths(env)
    env.update(parse_env_overrides(args.env))
    if args.backend == "tests" and is_sanitizer_runner(args):
        sanitizer = selected_sanitizer(args)
        env["RUSTFLAGS"] = append_space_flag(env.get("RUSTFLAGS"), f"-Zsanitizer={sanitizer}")
        apply_sanitizer_env(env, sanitizer)
    if args.backend == "tests" and args.tests_runner == "miri":
        env["MIRIFLAGS"] = append_space_flag(env.get("MIRIFLAGS"), "-Zmiri-ignore-leaks")
    if args.backend == "fuzz":
        if args.fuzz_exit_when_done:
            env["AFL_EXIT_WHEN_DONE"] = "1"
        else:
            env.pop("AFL_EXIT_WHEN_DONE", None)
        env["AFL_NO_AFFINITY"] = "1"
        sanitizer = selected_fuzz_sanitizer(args)
        if sanitizer:
            env["RUSTFLAGS"] = append_space_flag(env.get("RUSTFLAGS"), f"-Zsanitizer={sanitizer}")
            apply_sanitizer_env(env, sanitizer)
            apply_afl_sanitizer_env(env, sanitizer)
    if args.backend == "kani":
        apply_kani_env(args, env)
    target_dir = run_dir / "target"
    env["CARGO_TARGET_DIR"] = str(target_dir)

    if args.backend == "fuzz":
        if args.dry_run:
            for target in fuzz_targets(harness, args.fuzz_targets):
                input_dir = prepared_fuzz_input_dir(run_dir, target)
                print(" ".join(afl_build_command(manifest, target, args)))
                print(
                    " ".join(
                        fuzz_target_command(
                            target,
                            args,
                            input_dir,
                            run_dir / "targets" / target / "afl-out",
                            target_dir,
                        )
                    )
                )
            return {
                **base_result(generation, run_dir, [], result_backend(args)),
                "status": "dry-run",
                "exit_code": 0,
            }
        result = run_fuzz_backend(generation, args, run_dir, harness, manifest, env)
        if not args.keep_target:
            shutil.rmtree(target_dir, ignore_errors=True)
        return result

    command = backend_command(args, manifest)
    stdout_path = run_dir / "stdout.log"
    stderr_path = run_dir / "stderr.log"

    if args.dry_run:
        print(" ".join(command))
        return {
            **base_result(generation, run_dir, command, result_backend(args)),
            "status": "dry-run",
            "exit_code": 0,
        }

    if args.backend == "tests":
        result = run_tests_backend(generation, args, run_dir, harness, env, command)
        if not args.keep_target:
            shutil.rmtree(target_dir, ignore_errors=True)
        return result

    if args.backend == "kani" and args.kani_mode == "official-proof":
        result = run_official_kani_backend(generation, args, run_dir, harness, env, command)
        if not args.keep_target:
            shutil.rmtree(target_dir, ignore_errors=True)
        return result

    started = time.monotonic()
    status = "ok"
    exit_code = 0
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            completed = run_command(
                command,
                cwd=harness,
                env=env,
                stdout=stdout,
                stderr=stderr,
                timeout=args.timeout,
            )
        exit_code = completed.returncode
        if exit_code != 0:
            status = "fail"
    except subprocess.TimeoutExpired:
        status = "timeout"
        exit_code = -999

    elapsed_sec = round(time.monotonic() - started, 3)
    kani_log_path = None
    if args.backend == "kani":
        kani_log_path = collect_kani_artifacts(harness, run_dir)
    result = {
        **base_result(generation, run_dir, command, result_backend(args)),
        "status": status,
        "exit_code": exit_code,
        "elapsed_sec": elapsed_sec,
        "timeout_sec": args.timeout,
        "stdout_tail": tail(stdout_path),
        "stderr_tail": tail(stderr_path),
        "first_error": first_error(stderr_path)
        or first_error(stdout_path)
        or first_error(kani_log_path),
    }
    if kani_log_path is not None:
        result["kani_verify_log"] = str(kani_log_path)
        result["kani_verify_tail"] = tail(kani_log_path)
    result.update(
        execution_metrics(args.backend, stdout_path, stderr_path, generation, args, kani_log_path)
    )
    result_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")

    if not args.keep_target:
        shutil.rmtree(target_dir, ignore_errors=True)
    return result


def base_result(generation, run_dir, command, backend_name):
    return {
        "crate": generation.get("crate"),
        "version": generation.get("version"),
        "experiment": generation.get("experiment"),
        "config": generation.get("config"),
        "generation_backend": generation.get("backend"),
        "backend": backend_name,
        "generation_harness": generation.get("harness"),
        "generation_sequence_count": int(float(generation.get("sequence_count") or 0)),
        "generation_target_count": int(float(generation.get("covered_target_count") or 0)),
        "generation_unsafe_wrapper_count": int(float(generation.get("covered_unsafe_wrapper_count") or 0)),
        "run_dir": str(run_dir),
        "command": command,
    }


def backend_command(args, manifest):
    if args.backend == "tests":
        if args.tests_runner == "miri":
            return [
                "cargo",
                "miri",
                "test",
                "--manifest-path",
                str(manifest),
                "--locked",
            ]
        return [
            "cargo",
            "test",
            *(
                ["-Zbuild-std"]
                if sanitizer_needs_build_std(selected_sanitizer(args))
                else []
            ),
            "--manifest-path",
            str(manifest),
            "--locked",
            *(
                ["--target", args.sanitizer_target_triple]
                if is_sanitizer_runner(args)
                else []
            ),
            "--no-run",
            "--message-format",
            "json",
        ]
    if args.backend == "fuzz":
        targets = fuzz_targets(manifest.parent, args.fuzz_targets)
        input_dir = prepared_fuzz_input_dir(manifest.parent, targets[0])
        return fuzz_target_command(
            targets[0],
            args,
            input_dir,
            manifest.parent / "afl-out",
            manifest.parent / "target",
        )
    if args.backend == "kani":
        if args.kani_mode == "official-proof":
            command = shlex.split(args.kani_command) if args.kani_command else ["cargo", "kani"]
            if "--manifest-path" not in command and not any(
                arg.startswith("--manifest-path=") for arg in command
            ):
                command.extend(["--manifest-path", str(manifest)])
            command.extend(args.kani_arg)
            command.extend(shlex.split(args.kani_extra_args))
            return command
        if args.kani_mode == "config":
            command = configured_kani_command(args)
        else:
            command = [args.kani_command or "cargo-kani"]
            if args.kani_mode == "codegen":
                command.append("--only-codegen")
        if "--manifest-path" not in command and not any(
            arg.startswith("--manifest-path=") for arg in command
        ):
            command.extend(["--manifest-path", str(manifest)])
        return command
    raise AssertionError(args.backend)


def result_backend(args):
    if args.backend == "tests" and is_sanitizer_runner(args):
        sanitizer = selected_sanitizer(args)
        return "tests-asan" if sanitizer == "address" else f"tests-{sanitizer}"
    if args.backend == "tests" and args.tests_runner == "miri":
        return "tests-miri"
    if args.backend == "fuzz":
        sanitizer = selected_fuzz_sanitizer(args)
        if sanitizer == "address":
            return "fuzz-asan"
        if sanitizer:
            return f"fuzz-{sanitizer}"
    return args.backend


def command_backend_name(command):
    if len(command) >= 3 and command[:3] == ["cargo", "miri", "test"]:
        return "tests-miri"
    if len(command) >= 2 and command[:2] == ["cargo", "test"]:
        return "tests"
    if len(command) >= 2 and command[:2] == ["cargo", "kani"]:
        return "kani"
    if command and Path(command[0]).name == "cargo-kani":
        return "kani"
    if len(command) >= 3 and command[:3] == ["cargo", "afl", "fuzz"]:
        return "fuzz"
    return "unknown"


def configured_kani_command(args):
    command = list(args.kani_verify_cmd or [])
    if not command:
        raise RuntimeError("Kani config mode requires a non-empty inline verify_cmd")
    if args.kani_command:
        command[0] = args.kani_command
    return command


def prepare_kani(args):
    source = args.kani_dir
    if not source.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "clone", args.kani_repo, str(source)],
            check=True,
        )
    cargo_kani = source / "target" / "kani" / "bin" / "cargo-kani"
    if args.build_kani:
        subprocess.run(["cargo", "build-dev", "--", "--release"], cwd=source, check=True)
    elif not cargo_kani.exists():
        raise RuntimeError(
            f"missing cargo-kani binary: {cargo_kani}. "
            "Run with --build-kani or point KANI_DIR at a built Kani checkout."
        )

def apply_kani_env(args, env):
    if args.kani_mode == "official-proof":
        return
    if not args.prepare_kani:
        return
    bin_dir = args.kani_dir / "target" / "kani" / "bin"
    scripts_dir = args.kani_dir / "scripts"
    env["PATH"] = prepend_path(env.get("PATH", ""), [scripts_dir, bin_dir])


def collect_kani_artifacts(harness, run_dir):
    log_source = harness / "varies_verify.log"
    script_source = harness / "varies-ver.sh"
    log_dest = None
    if log_source.exists():
        log_dest = run_dir / "varies_verify.log"
        shutil.copyfile(log_source, log_dest)
    if script_source.exists():
        shutil.copyfile(script_source, run_dir / "varies-ver.sh")
    return log_dest


def prepend_path(existing, paths):
    prefix = [str(path) for path in paths if path.exists()]
    if not prefix:
        return existing
    if not existing:
        return os.pathsep.join(prefix)
    return os.pathsep.join([*prefix, existing])


def fuzz_targets(harness, mode):
    targets = sorted(path.stem for path in (harness / "fuzz_targets").glob("*.rs"))
    if not targets:
        raise RuntimeError(f"no fuzz targets found under {harness / 'fuzz_targets'}")
    if mode == "first":
        return targets[:1]
    return targets


def prepared_fuzz_input_dir(run_dir, target):
    return run_dir / "targets" / target / "afl-in"


def afl_build_command(manifest, target, args):
    sanitizer = selected_fuzz_sanitizer(args)
    return [
        "cargo",
        "afl",
        "build",
        *(
            ["-Zbuild-std"]
            if sanitizer and sanitizer_needs_build_std(sanitizer)
            else []
        ),
        "--manifest-path",
        str(manifest),
        "--locked",
        *(["--target", args.sanitizer_target_triple] if sanitizer else []),
        "--bin",
        target,
    ]


def fuzz_target_command(target, args, input_dir, output_dir, target_dir, time_limit=None):
    binary = fuzz_binary_path(target_dir, target, args)
    if time_limit is None:
        time_limit = fuzz_target_time_limit(args)
    return [
        "cargo",
        "afl",
        "fuzz",
        "-i",
        str(input_dir),
        "-o",
        str(output_dir),
        "-V",
        str(max(1, int(time_limit))),
        "--",
        str(binary),
    ]


def fuzz_binary_path(target_dir, target, args):
    sanitizer = selected_fuzz_sanitizer(args)
    if sanitizer:
        return target_dir / args.sanitizer_target_triple / "debug" / target
    return target_dir / "debug" / target


def prepare_afl_input_dir(input_dir, valid_seed_results):
    if input_dir.exists():
        shutil.rmtree(input_dir)
    input_dir.mkdir(parents=True, exist_ok=True)
    for index, seed_result in enumerate(valid_seed_results):
        seed_name = f"seed_{index}_{safe_log_stem(seed_result['name'])}"
        (input_dir / seed_name).write_bytes(bytes.fromhex(seed_result["hex"]))


def remove_legacy_fuzz_target_safe_seed_guard(harness, target):
    target_path = harness / "fuzz_targets" / f"{target}.rs"
    source = target_path.read_text(encoding="utf-8")
    if AFL_LEGACY_SAFE_SEED_GUARD not in source:
        return False
    target_path.write_text(source.replace(AFL_LEGACY_SAFE_SEED_GUARD, "", 1), encoding="utf-8")
    return True


def fuzz_target_min_data_len(harness, target):
    target_path = harness / "fuzz_targets" / f"{target}.rs"
    source = target_path.read_text(encoding="utf-8")
    matches = [int(value) for value in re.findall(r"if data\.len\(\) < (\d+) \{", source)]
    if not matches:
        return 0
    return max(matches)


def fuzz_usize_byte_width(harness):
    support_path = harness / "src" / "lib.rs"
    if not support_path.exists():
        return 8
    source = support_path.read_text(encoding="utf-8")
    for helper, width in (
        ("_to_u8(data, index) as usize", 1),
        ("_to_u16(data, index) as usize", 2),
        ("_to_u32(data, index) as usize", 4),
        ("_to_u64(data, index) as usize", 8),
        ("_to_u128(data, index) as usize", 16),
    ):
        if helper in source:
            return width
    return 8


def fuzz_target_usize_offsets(source):
    return sorted({int(value) for value in re.findall(r"_to_usize\(data,\s*(\d+)\)", source)})


def write_little_endian_usize(seed, offset, value, width):
    if offset >= len(seed):
        return
    encoded = int(value).to_bytes(width, byteorder="little", signed=False)
    end = min(len(seed), offset + width)
    seed[offset:end] = encoded[: end - offset]


def structured_fuzz_seed_candidates(harness, target, required_len):
    target_path = harness / "fuzz_targets" / f"{target}.rs"
    source = target_path.read_text(encoding="utf-8")
    usize_offsets = fuzz_target_usize_offsets(source)
    usize_width = fuzz_usize_byte_width(harness)
    candidates = []

    for name, fill, usize_value in (("all_one", 1, None), ("ascii", 65, None)):
        data = bytearray([fill] * required_len)
        candidates.append((name, bytes(data)))

    for usize_value in (1, 2, 3, 4, 8):
        for fill_name, fill in (("zero", 0), ("ascii", 65)):
            data = bytearray([fill] * required_len)
            for offset in usize_offsets:
                write_little_endian_usize(data, offset, usize_value, usize_width)
            candidates.append((f"{fill_name}_usize_{usize_value}", bytes(data)))

    return candidates


def fuzz_seed_candidates(harness, target):
    target_path = harness / "fuzz_targets" / f"{target}.rs"
    source = target_path.read_text(encoding="utf-8")
    matches = [int(value) for value in re.findall(r"if data\.len\(\) < (\d+) \{", source)]
    min_data_len = max(matches) if matches else 0
    required_len = max(1, min_data_len)
    candidates = [("zero", bytes(required_len))]
    if required_len > 1:
        candidates.append(("one", bytes([1]) + bytes(required_len - 1)))
    candidates.extend(structured_fuzz_seed_candidates(harness, target, required_len))

    corpus_dir = harness / "corpus" / target
    if corpus_dir.exists():
        for path in sorted(corpus_dir.iterdir()):
            if not path.is_file():
                continue
            data = path.read_bytes()
            if len(data) >= min_data_len and data:
                candidates.append((f"corpus_{path.name}", data))

    deduped = []
    seen = set()
    for name, data in candidates:
        if data in seen:
            continue
        seen.add(data)
        deduped.append((name, data))
    return min_data_len, deduped


def validate_fuzz_seed_candidates(binary, harness, env, candidates):
    validation_results = []
    valid_results = []
    command = direct_fuzz_binary_command(binary)
    for name, data in candidates:
        exit_code, stdout, stderr, timed_out = run_command_with_stdin_capture(
            command,
            cwd=harness,
            env=env,
            stdin_bytes=data,
            timeout=FUZZ_SEED_VALIDATION_TIMEOUT,
        )
        status = "timeout" if timed_out else "ok" if exit_code == 0 else "crash"
        row = {
            "name": name,
            "len": len(data),
            "hex": data.hex(),
            "status": status,
            "exit_code": exit_code,
            "stdout_tail": decode_tail(stdout),
            "stderr_tail": decode_tail(stderr),
        }
        validation_results.append(row)
        if status == "ok":
            valid_results.append(row)
    return validation_results, valid_results


def direct_fuzz_binary_command(binary):
    return ["bash", "-lc", 'ulimit -c 0; exec "$1"', "bash", str(binary)]


def run_fuzz_backend(generation, args, run_dir, harness, manifest, env):
    started = time.monotonic()
    targets = fuzz_targets(harness, args.fuzz_targets)
    existing_results = (
        load_existing_fuzz_target_results(run_dir)
        if args.reuse_fuzz_target_results
        else {}
    )
    run_targets = []
    for target in targets:
        input_dir = prepared_fuzz_input_dir(run_dir, target)
        output_dir = run_dir / "targets" / target / "afl-out"
        command = fuzz_target_command(target, args, input_dir, output_dir, run_dir / "target")
        run_targets.append(
            {
                "name": target,
                "build_command": afl_build_command(manifest, target, args),
                "command": command,
                "binary": str(fuzz_binary_path(run_dir / "target", target, args)),
                "afl_input_dir": str(input_dir),
                "afl_output_dir": str(output_dir),
                "stdout_log": str(run_dir / "targets" / target / "stdout.log"),
                "stderr_log": str(run_dir / "targets" / target / "stderr.log"),
                "existing_result": existing_results.get(target),
            }
        )

    commands_path = run_dir / "commands.sh"
    commands_path.write_text(
        "\n".join(
            [
                "#!/usr/bin/env bash",
                "set -euo pipefail",
                *fuzz_command_env_exports(env),
                *(
                    command
                    for target in run_targets
                    for command in (
                        f"mkdir -p {target['afl_input_dir']}",
                        "# AFL input seeds are validated and written by suite_execute.py",
                        " ".join(target["build_command"]),
                        " ".join(target["command"]),
                    )
                ),
                "",
            ]
        ),
        encoding="utf-8",
    )
    commands_path.chmod(0o755)

    if args.fuzz_jobs <= 1 or len(run_targets) <= 1:
        target_results = run_fuzz_targets_serial(run_targets, harness, env, args, run_dir)
    else:
        target_results = run_fuzz_targets_parallel(run_targets, harness, env, args, run_dir)

    status = fuzz_group_status(target_results)
    result = {
        **base_result(generation, run_dir, [str(commands_path)], result_backend(args)),
        "status": status,
        "exit_code": fuzz_group_exit_code(target_results),
        "elapsed_sec": round(time.monotonic() - started, 3),
        "timeout_sec": args.timeout,
        "timeout_scope": "fuzz-target",
        "fuzz_target_timeout_sec": fuzz_outer_timeout(args, fuzz_target_time_limit(args)),
        "fuzz_time_limit_sec": fuzz_target_time_limit(args),
        "fuzz_jobs": args.fuzz_jobs,
        "fuzz_exit_when_done": args.fuzz_exit_when_done,
        "fuzz_stop_on_ub": args.fuzz_stop_on_ub,
        "fuzz_targets_mode": args.fuzz_targets,
        "fuzz_sanitizer": selected_fuzz_sanitizer(args),
        "fuzz_sanitizer_build_std": bool(
            selected_fuzz_sanitizer(args)
            and sanitizer_needs_build_std(selected_fuzz_sanitizer(args))
        ),
        "sanitizer_target_triple": args.sanitizer_target_triple
        if selected_fuzz_sanitizer(args)
        else None,
        "rustflags": env.get("RUSTFLAGS"),
        "asan_options": env.get("ASAN_OPTIONS")
        if selected_fuzz_sanitizer(args) == "address"
        else None,
        "msan_options": env.get("MSAN_OPTIONS")
        if selected_fuzz_sanitizer(args) == "memory"
        else None,
        "tsan_options": env.get("TSAN_OPTIONS")
        if selected_fuzz_sanitizer(args) == "thread"
        else None,
        "afl_use_asan": env.get("AFL_USE_ASAN"),
        "afl_use_msan": env.get("AFL_USE_MSAN"),
        "afl_use_tsan": env.get("AFL_USE_TSAN"),
        "afl_exit_when_done": "1",
        "afl_no_affinity": "1",
        "fuzz_targets_available": len(run_targets),
        "fuzz_targets_executed": sum(1 for row in target_results if row["status"] != "skipped"),
        "fuzz_targets_ok": sum(1 for row in target_results if row["status"] == "ok"),
        "fuzz_targets_fail": sum(1 for row in target_results if row["status"] == "fail"),
        "fuzz_targets_timeout": sum(1 for row in target_results if row["status"] == "timeout"),
        "fuzz_targets_skipped": sum(1 for row in target_results if row["status"] == "skipped"),
        "fuzz_targets_reused": sum(1 for row in target_results if row.get("reused")),
        "fuzz_crashes": sum(1 for row in target_results if row.get("crash_detected")),
        "fuzz_early_stop_targets": sum(1 for row in target_results if row.get("fuzz_early_stop")),
        "fuzz_execs_done": sum(int(row.get("fuzz_execs_done") or 0) for row in target_results),
        "fuzz_target_results_path": str(run_dir / "fuzz_target_results.json"),
        "fuzz_target_results": target_results,
        "first_error": first_fuzz_error(target_results),
        "stdout_tail": "",
        "stderr_tail": "",
    }
    (run_dir / "fuzz_target_results.json").write_text(
        json.dumps(target_results, indent=2, sort_keys=True) + "\n"
    )
    (run_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def run_fuzz_targets_serial(run_targets, harness, env, args, run_dir):
    results = []
    for target in run_targets:
        result = run_one_fuzz_target(target, harness, env, args, run_dir)
        results.append(result)
    return results


def run_fuzz_targets_parallel(run_targets, harness, env, args, run_dir):
    results = [None] * len(run_targets)
    next_index = 0
    index_lock = threading.Lock()

    def worker():
        nonlocal next_index
        while True:
            with index_lock:
                if next_index >= len(run_targets):
                    return
                index = next_index
                next_index += 1
            results[index] = run_one_fuzz_target(run_targets[index], harness, env, args, run_dir)

    with ThreadPoolExecutor(max_workers=max(1, args.fuzz_jobs)) as executor:
        futures = [executor.submit(worker) for _ in range(min(max(1, args.fuzz_jobs), len(run_targets)))]
        for future in as_completed(futures):
            future.result()
    return [result if result is not None else skipped_fuzz_result(run_targets[index]["name"]) for index, result in enumerate(results)]


def run_one_fuzz_target(target, harness, env, args, run_dir):
    existing_result = target.get("existing_result")
    if reusable_fuzz_target_result(existing_result):
        return existing_result

    target_name = target["name"]
    build_command = target["build_command"]
    command = target["command"]
    legacy_safe_seed_guard_removed = remove_legacy_fuzz_target_safe_seed_guard(harness, target_name)
    input_dir = Path(target["afl_input_dir"])
    afl_output_dir = Path(target["afl_output_dir"])
    target_dir = run_dir / "targets" / target_name
    target_dir.mkdir(parents=True, exist_ok=True)
    binary = Path(target["binary"])
    stdout_path = target_dir / "stdout.log"
    stderr_path = target_dir / "stderr.log"
    started = time.monotonic()
    status = "ok"
    build_exit_code = 0
    fuzz_exit_code = None
    exit_code = 0
    fuzz_ran = False
    seed_min_data_len = None
    seed_validation_results = []
    valid_seed_results = []
    seed_validation_failed = False
    seed_validation_error = None
    fuzz_early_stop_info = None
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            write_log(stdout, f"\n[suite-execute] running {' '.join(build_command)}\n", None)
            completed = run_command(
                build_command,
                cwd=harness,
                env=env,
                stdout=stdout,
                stderr=stderr,
                timeout=None,
            )
            build_exit_code = completed.returncode
            if build_exit_code != 0:
                status = "fail"
                exit_code = build_exit_code
            else:
                seed_min_data_len, seed_candidates = fuzz_seed_candidates(harness, target_name)
                seed_validation_results, valid_seed_results = validate_fuzz_seed_candidates(
                    binary, harness, env, seed_candidates
                )
                write_log(
                    stdout,
                    (
                        "\n[suite-execute] validated "
                        f"{len(valid_seed_results)}/{len(seed_validation_results)} AFL seeds "
                        f"for {target_name} (min_data_len={seed_min_data_len})\n"
                    ),
                    None,
                )
                if not valid_seed_results:
                    status = "fail"
                    exit_code = 1
                    seed_validation_failed = True
                    seed_validation_error = (
                        f"{target_name}: no non-crashing AFL seed candidate "
                        f"(min_data_len={seed_min_data_len})"
                    )
                    write_log(stdout, f"[suite-execute] {seed_validation_error}\n", None)
                else:
                    prepare_afl_input_dir(input_dir, valid_seed_results)
                    fuzz_time_limit = fuzz_target_time_limit(args)
                    if fuzz_time_limit is not None and fuzz_time_limit <= 0:
                        raise subprocess.TimeoutExpired(command, fuzz_time_limit)
                    command = fuzz_target_command(
                        target_name,
                        args,
                        input_dir,
                        afl_output_dir,
                        run_dir / "target",
                        fuzz_time_limit,
                    )
                    write_log(stdout, f"\n[suite-execute] running {' '.join(command)}\n", None)
                    fuzz_ran = True
                    completed, fuzz_early_stop_info = run_fuzz_command(
                        command,
                        cwd=harness,
                        env=env,
                        stdout=stdout,
                        stderr=stderr,
                        timeout=fuzz_outer_timeout(args, fuzz_time_limit),
                        binary=binary,
                        afl_output_dir=afl_output_dir,
                        stop_on_ub=args.fuzz_stop_on_ub,
                    )
                    fuzz_exit_code = completed.returncode
                    exit_code = fuzz_exit_code
                    if fuzz_exit_code != 0:
                        status = "fail"
    except subprocess.TimeoutExpired:
        status = "timeout"
        exit_code = -999

    stdout_tail = tail(stdout_path)
    stderr_tail = tail(stderr_path)
    combined = "\n".join([tail(stdout_path, 200000), tail(stderr_path, 200000)])
    crash_count = afl_crash_count(afl_output_dir)
    crash_detected = crash_count > 0 or (fuzz_ran and has_fuzz_crash(combined, exit_code))
    if crash_detected and status == "ok":
        status = "fail"
        exit_code = 1
    result = {
        "name": target_name,
        "status": status,
        "exit_code": exit_code,
        "build_exit_code": build_exit_code,
        "fuzz_exit_code": fuzz_exit_code,
        "elapsed_sec": round(time.monotonic() - started, 3),
        "build_command": build_command,
        "command": command,
        "binary": str(binary),
        "afl_input_dir": str(input_dir),
        "afl_output_dir": str(afl_output_dir),
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
        "first_error": seed_validation_error or first_error(stderr_path) or first_error(stdout_path),
        "fuzz_execs_done": read_afl_stat(afl_output_dir, "execs_done"),
        "fuzz_crash_count": crash_count,
        "crash_detected": crash_detected,
        "legacy_safe_seed_guard_removed": legacy_safe_seed_guard_removed,
        "seed_min_data_len": seed_min_data_len,
        "seed_candidates": len(seed_validation_results),
        "seed_valid": len(valid_seed_results),
        "seed_validation_failed": seed_validation_failed,
        "seed_validation_results": seed_validation_results,
        "timeout_scope": "fuzz-target",
        "fuzz_time_limit_sec": fuzz_target_time_limit(args),
        "fuzz_target_timeout_sec": fuzz_outer_timeout(args, fuzz_target_time_limit(args)),
        "fuzz_early_stop_info": fuzz_early_stop_info,
    }
    result["unsoundness_kind"] = fuzz_target_primary_unsoundness_kind(result)
    result["fuzz_early_stop"] = fuzz_result_is_high_confidence_unsoundness(result)
    return result


def load_existing_fuzz_target_results(run_dir):
    path = run_dir / "fuzz_target_results.json"
    try:
        rows = json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return {}
    results = {}
    for row in rows:
        if not isinstance(row, dict):
            continue
        name = row.get("name")
        if not name:
            continue
        normalized = normalize_existing_fuzz_target_result(row)
        results[name] = normalized
    return results


def normalize_existing_fuzz_target_result(row):
    normalized = dict(row)
    if normalized.get("status") == "skipped":
        normalized.setdefault("unsoundness_kind", None)
        normalized.setdefault("fuzz_early_stop", False)
        return normalized
    normalized.setdefault("timeout_scope", "fuzz-target")
    normalized["unsoundness_kind"] = fuzz_target_primary_unsoundness_kind(normalized)
    normalized["fuzz_early_stop"] = fuzz_result_is_high_confidence_unsoundness(normalized)
    normalized["reused"] = True
    return normalized


def reusable_fuzz_target_result(result):
    return isinstance(result, dict) and result.get("status") != "skipped"


def fuzz_target_time_limit(args):
    if args.fuzz_time_limit and args.fuzz_time_limit > 0:
        return args.fuzz_time_limit
    return args.timeout


def fuzz_outer_timeout(args, fuzz_time_limit):
    if args.fuzz_target_timeout and args.fuzz_target_timeout > 0:
        return args.fuzz_target_timeout
    if fuzz_time_limit and fuzz_time_limit > 0:
        return fuzz_time_limit + 600
    return None


def skipped_fuzz_result(name):
    return {
        "name": name,
        "status": "skipped",
        "exit_code": None,
        "elapsed_sec": 0.0,
        "build_command": [],
        "command": [],
        "binary": "",
        "afl_output_dir": "",
        "stdout_log": "",
        "stderr_log": "",
        "stdout_tail": "",
        "stderr_tail": "",
        "first_error": None,
        "fuzz_execs_done": 0,
        "fuzz_crash_count": 0,
        "crash_detected": False,
        "unsoundness_kind": None,
        "fuzz_early_stop": False,
    }


HIGH_CONFIDENCE_FUZZ_UNSOUNDNESS = {
    "address_sanitizer",
    "memory_sanitizer",
    "thread_sanitizer",
    "undefined_behavior_sanitizer",
    "unsafe_precondition",
}


def has_fuzz_crash(text, exit_code):
    lowered = text.lower()
    if "Test unit written to" in text or "crashes saved as" in text or "Saved as" in text:
        return True
    if "addresssanitizer" in lowered or "runtime error:" in lowered:
        return True
    return exit_code not in (0, None, -999)


def run_fuzz_command(
    command,
    cwd,
    env,
    stdout,
    stderr,
    timeout,
    binary,
    afl_output_dir,
    stop_on_ub,
):
    if timeout is not None and timeout <= 0:
        timeout = None
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=stdout,
        stderr=stderr,
        start_new_session=True,
    )
    process_started_at = process_start_time(process.pid)
    monitor_stop = threading.Event()
    early_stop_info = {}
    monitor = None
    if stop_on_ub:
        monitor = threading.Thread(
            target=monitor_fuzz_crashes_for_ub,
            args=(process, binary, cwd, env, afl_output_dir, monitor_stop, early_stop_info),
            daemon=True,
        )
        monitor.start()
    try:
        exit_code = process.wait(timeout=timeout)
        completed = subprocess.CompletedProcess(command, exit_code)
    except subprocess.TimeoutExpired:
        kill_process_tree(process, process_started_at)
        raise
    finally:
        monitor_stop.set()
        if monitor is not None:
            monitor.join(timeout=1)
    return completed, early_stop_info or None


def monitor_fuzz_crashes_for_ub(process, binary, cwd, env, afl_output_dir, stop_event, early_stop_info):
    seen = set()
    while not stop_event.is_set() and process.poll() is None:
        for crash_path in afl_crash_files(afl_output_dir):
            if crash_path in seen:
                continue
            seen.add(crash_path)
            replay = replay_fuzz_crash(binary, cwd, env, crash_path)
            kind = classify_unsoundness(
                replay["stdout_tail"],
                replay["stderr_tail"],
                replay["status"],
                replay["exit_code"],
            )
            if kind not in HIGH_CONFIDENCE_FUZZ_UNSOUNDNESS:
                continue
            early_stop_info.update(
                {
                    "reason": "high_confidence_ub",
                    "classification": kind,
                    "crash_input": str(crash_path),
                    "replay_exit_code": replay["exit_code"],
                    "replay_stdout_tail": replay["stdout_tail"],
                    "replay_stderr_tail": replay["stderr_tail"],
                }
            )
            kill_process_tree(process)
            return
        stop_event.wait(1)


def afl_crash_files(output_dir):
    paths = []
    for crash_dir in afl_crash_dirs(output_dir):
        if not crash_dir.exists():
            continue
        paths.extend(
            path
            for path in crash_dir.iterdir()
            if path.is_file() and path.name.startswith("id:")
        )
    return sorted(paths)


def replay_fuzz_crash(binary, cwd, env, crash_path):
    data = crash_path.read_bytes()
    exit_code, stdout, stderr, timed_out = run_command_with_stdin_capture(
        direct_fuzz_binary_command(binary),
        cwd=cwd,
        env=env,
        stdin_bytes=data,
        timeout=FUZZ_CRASH_REPLAY_TIMEOUT,
    )
    return {
        "status": "timeout" if timed_out else "ok" if exit_code == 0 else "fail",
        "exit_code": exit_code,
        "stdout_tail": decode_tail(stdout),
        "stderr_tail": decode_tail(stderr),
    }


def afl_crash_dirs(output_dir):
    return [
        output_dir / "default" / "crashes",
        output_dir / "crashes",
    ]


def afl_crash_count(output_dir):
    count = 0
    for crash_dir in afl_crash_dirs(output_dir):
        if not crash_dir.exists():
            continue
        count += sum(
            1
            for path in crash_dir.iterdir()
            if path.is_file() and path.name.startswith("id:")
        )
    return count


def read_afl_stat(output_dir, name):
    stat_paths = [
        output_dir / "default" / "fuzzer_stats",
        output_dir / "fuzzer_stats",
    ]
    for stat_path in stat_paths:
        if not stat_path.exists():
            continue
        for line in stat_path.read_text(encoding="utf-8", errors="replace").splitlines():
            key, sep, value = line.partition(":")
            if sep and key.strip() == name:
                try:
                    return int(value.strip())
                except ValueError:
                    return 0
    return 0


def fuzz_group_status(target_results):
    statuses = [row["status"] for row in target_results]
    if any(status == "timeout" for status in statuses):
        return "timeout"
    if any(status == "fail" for status in statuses):
        return "fail"
    if any(status == "ok" for status in statuses):
        return "ok"
    return "fail"


def fuzz_group_exit_code(target_results):
    if any(row["status"] == "timeout" for row in target_results):
        return -999
    for row in target_results:
        if row["status"] == "fail":
            return row["exit_code"] or 1
    return 0


def first_fuzz_error(target_results):
    for row in target_results:
        if row.get("first_error"):
            return row["first_error"]
        if row["status"] in ("fail", "timeout"):
            return f"{row['name']}: {row['status']}"
    return None


def fuzz_result_is_high_confidence_unsoundness(result):
    return fuzz_target_high_confidence_unsoundness_kind(result) is not None


def fuzz_target_primary_unsoundness_kind(target):
    high_confidence = fuzz_target_high_confidence_unsoundness_kind(target)
    if high_confidence is not None:
        return high_confidence
    for _source, status, exit_code, stdout_tail, stderr_tail in fuzz_target_observations(target):
        kind = classify_unsoundness(stdout_tail, stderr_tail, status, int(exit_code or 0))
        if kind:
            return kind
    return None


def fuzz_target_high_confidence_unsoundness_kind(target):
    for _source, status, exit_code, stdout_tail, stderr_tail in fuzz_target_observations(target):
        kind = classify_unsoundness(stdout_tail, stderr_tail, status, int(exit_code or 0))
        if kind in HIGH_CONFIDENCE_FUZZ_UNSOUNDNESS:
            return kind
    return None


def run_official_kani_backend(generation, args, run_dir, harness, env, command):
    stdout_path = run_dir / "stdout.log"
    stderr_path = run_dir / "stderr.log"
    proof_results_path = run_dir / "kani_proof_results.json"
    proof_result_dir = run_dir / "proof_results"
    proof_log_dir = run_dir / "proof_logs"
    started = time.monotonic()
    available_proofs = generated_kani_proof_names(harness)
    # Official Kani runs have their own per-harness timeout (`--harness-timeout`).
    # Do not apply a crate-level deadline here: large crates with many generated
    # proofs should be able to run all harnesses, bounded individually.
    suite_deadline = None
    log_lock = threading.Lock()
    proof_result_dir.mkdir(parents=True, exist_ok=True)
    existing_results = load_existing_official_kani_results(
        available_proofs, proof_result_dir, proof_log_dir, stdout_path, stderr_path
    )
    for proof_name, result in existing_results.items():
        write_official_kani_proof_result(proof_result_dir, proof_name, result)
    pending_proofs = [name for name in available_proofs if name not in existing_results]

    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        if existing_results:
            write_log(
                stdout,
                (
                    "[suite-execute] reusing "
                    f"{len(existing_results)} existing Kani proof results\n"
                ),
                None,
            )
        proof_results_by_name = dict(existing_results)
        proof_results_by_name.update(
            run_official_kani_proofs(
                pending_proofs,
                args.kani_jobs,
                lambda proof_name, timeout: run_one_official_kani_proof(
                    command,
                    proof_name,
                    harness,
                    env,
                    stdout,
                    stderr,
                    timeout,
                    proof_log_dir,
                    log_lock,
                ),
                proof_result_dir,
                suite_deadline,
                args.kani_proof_timeout,
                proof_log_dir,
            )
        )
    proof_results = [proof_results_by_name[name] for name in available_proofs]

    status = first_non_ok_status(proof_results) or "ok"
    elapsed_sec = round(time.monotonic() - started, 3)
    proof_results_path.write_text(json.dumps(proof_results, indent=2, sort_keys=True) + "\n")
    result = {
        **base_result(generation, run_dir, command, result_backend(args)),
        "status": status,
        "exit_code": test_exit_code(proof_results),
        "elapsed_sec": elapsed_sec,
        "timeout_sec": args.timeout,
        "kani_mode": args.kani_mode,
        "kani_jobs": args.kani_jobs,
        "kani_proof_timeout_sec": args.kani_proof_timeout,
        "kani_args": [*args.kani_arg, *shlex.split(args.kani_extra_args)],
        "kani_proofs_reused": len(existing_results),
        "kani_proofs_executed_this_run": len(pending_proofs),
        "kani_proof_result_path": str(proof_results_path),
        "stdout_tail": tail(stdout_path),
        "stderr_tail": tail(stderr_path),
        "first_error": first_error(stderr_path) or first_error(stdout_path) or first_result_error(proof_results),
        "backend_output_count": int(float(generation.get("generated_test_module_count") or 0)),
        "available_generated_tests": len(available_proofs),
        "selected_generated_tests": len(available_proofs),
        "cargo_test_result_sections": len(proof_results),
        "cargo_test_ok_sections": sum(1 for result in proof_results if result["status"] == "ok"),
        "tests_passed": sum(1 for result in proof_results if result["status"] == "ok"),
        "tests_failed": sum(1 for result in proof_results if result["status"] == "fail"),
        "tests_timeout": sum(1 for result in proof_results if result["status"] == "timeout"),
        "tests_ignored": 0,
        "tests_measured": 0,
        "tests_filtered_out": 0,
    }
    (run_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def run_official_kani_proofs(
    proof_names, proof_jobs, run_one, proof_result_dir, deadline=None, proof_timeout=0, log_dir=None
):
    if proof_jobs <= 1 or len(proof_names) <= 1:
        results = {}
        for index, proof_name in enumerate(proof_names):
            timeout = bounded_test_timeout(proof_timeout, deadline)
            if proof_timeout is not None and proof_timeout <= 0 and deadline is None:
                timeout = None
            if timeout is not None and timeout <= 0:
                for name in proof_names[index:]:
                    result = suite_timeout_result(name, log_dir)
                    write_official_kani_proof_result(proof_result_dir, name, result)
                    results[name] = result
                break
            result = run_one(proof_name, timeout)
            write_official_kani_proof_result(proof_result_dir, proof_name, result)
            results[proof_name] = result
        return results

    results = {}
    next_index = 0
    index_lock = threading.Lock()

    def worker():
        nonlocal next_index
        worker_results = {}
        while True:
            with index_lock:
                if next_index >= len(proof_names):
                    return worker_results
                index = next_index
                next_index += 1
            proof_name = proof_names[index]
            timeout = bounded_test_timeout(proof_timeout, deadline)
            if proof_timeout is not None and proof_timeout <= 0 and deadline is None:
                timeout = None
            if timeout is not None and timeout <= 0:
                result = suite_timeout_result(proof_name, log_dir)
            else:
                result = run_one(proof_name, timeout)
            write_official_kani_proof_result(proof_result_dir, proof_name, result)
            worker_results[proof_name] = result

    with ThreadPoolExecutor(max_workers=proof_jobs) as executor:
        futures = [executor.submit(worker) for _ in range(min(proof_jobs, len(proof_names)))]
        for future in as_completed(futures):
            results.update(future.result())
    return results


def write_official_kani_proof_result(proof_result_dir, proof_name, result):
    proof_result_dir.mkdir(parents=True, exist_ok=True)
    (proof_result_dir / f"{safe_log_stem(proof_name)}.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n"
    )


def load_existing_official_kani_results(
    proof_names, proof_result_dir, proof_log_dir, stdout_path, stderr_path
):
    results = {}
    for proof_name in proof_names:
        path = proof_result_dir / f"{safe_log_stem(proof_name)}.json"
        if path.exists():
            try:
                results[proof_name] = json.loads(path.read_text())
                continue
            except json.JSONDecodeError:
                pass
        legacy = infer_legacy_official_kani_result(
            proof_name, proof_log_dir, stdout_path, stderr_path
        )
        if legacy is not None:
            results[proof_name] = legacy
    return results


def infer_legacy_official_kani_result(proof_name, proof_log_dir, stdout_path, stderr_path):
    combined = "\n".join([tail(stdout_path, 200000), tail(stderr_path, 200000)])
    if "Failed to get cargo metadata" in combined:
        return None
    timeout_marker = f"kani timeout after 1200s: {proof_name}"
    exit_marker = f"kani exit "
    proof_marker = f": {proof_name}"
    if timeout_marker in combined:
        status = "timeout"
        exit_code = -999
    else:
        exit_match = re.search(rf"kani exit (?P<code>-?\d+): {re.escape(proof_name)}\\b", combined)
        if not exit_match:
            return None
        status = "fail"
        exit_code = int(exit_match.group("code"))
    stdout_log = proof_log_dir / f"{safe_log_stem(proof_name)}.stdout.log"
    stderr_log = proof_log_dir / f"{safe_log_stem(proof_name)}.stderr.log"
    stdout_tail = tail(stdout_log)
    stderr_tail = tail(stderr_log)
    row = {
        "name": proof_name,
        "status": status,
        "exit_code": exit_code,
        "elapsed_sec": None,
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
        "first_error": first_error_text(stderr_tail) or first_error_text(stdout_tail),
        "unsoundness_kind": classify_unsoundness(stdout_tail, stderr_tail, status, exit_code),
    }
    if stdout_log.exists():
        row["stdout_log"] = str(stdout_log)
    if stderr_log.exists():
        row["stderr_log"] = str(stderr_log)
    return row


def run_one_official_kani_proof(
    base_command, proof_name, harness, env, stdout, stderr, timeout, log_dir, log_lock=None
):
    command = [*base_command, "--harness", proof_name]
    write_log(stdout, f"\n[suite-execute] running {' '.join(command)}\n", log_lock)
    started = time.monotonic()
    status = "ok"
    exit_code, captured_stdout, captured_stderr, timed_out = run_command_capture(
        command,
        cwd=harness,
        env=env,
        timeout=timeout,
    )
    if timed_out:
        status = "timeout"
        write_log(
            stderr,
            f"\n[suite-execute] kani timeout after {timeout}s: {proof_name}\n",
            log_lock,
        )
    elif exit_code != 0:
        status = "fail"
        write_log(
            stderr,
            f"\n[suite-execute] kani exit {exit_code}: {proof_name}\n",
            log_lock,
        )
    return test_result_row(
        proof_name,
        status,
        exit_code,
        round(time.monotonic() - started, 3),
        captured_stdout,
        captured_stderr,
        log_dir,
    )


def run_tests_backend(generation, args, run_dir, harness, env, command):
    stdout_path = run_dir / "stdout.log"
    stderr_path = run_dir / "stderr.log"
    test_results_path = run_dir / "test_results.json"
    started = time.monotonic()
    compile_exit_code = 0
    status = "ok"
    test_results = []
    binary = None
    suite_deadline = suite_deadline_from_timeout(started, args.timeout)
    available_test_names = generated_test_names(harness)
    test_names = available_test_names
    log_lock = threading.Lock()

    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        if args.tests_runner == "miri":
            compile_exit_code = 0
            test_log_dir = run_dir / "test_logs"
            test_results = run_generated_tests(
                test_names,
                args.test_jobs,
                lambda test_name, timeout: run_one_generated_test_with_miri(
                    command,
                    test_name,
                    harness,
                    env,
                    stdout,
                    stderr,
                    timeout,
                    test_log_dir,
                    log_lock,
                ),
                suite_deadline,
                args.test_timeout,
                test_log_dir,
            )
            status = first_non_ok_status(test_results) or status
        else:
            try:
                compile_timeout = remaining_suite_timeout(suite_deadline)
                completed = run_command(
                    command,
                    cwd=harness,
                    env=env,
                    stdout=stdout,
                    stderr=stderr,
                    timeout=compile_timeout,
                )
                compile_exit_code = completed.returncode
            except subprocess.TimeoutExpired:
                compile_exit_code = -999
                status = "timeout"

            if compile_exit_code == 0:
                binary = discover_test_binary(stdout_path, run_dir / "target")
                if binary is None:
                    status = "fail"
                    stderr.write(
                        b"\n[suite-execute] failed to discover generated test binary\n"
                    )
                else:
                    test_log_dir = run_dir / "test_logs"
                    test_results = run_generated_tests(
                        test_names,
                        args.test_jobs,
                        lambda test_name, timeout: run_one_generated_test(
                            binary,
                            test_name,
                            harness,
                            env,
                            stdout,
                            stderr,
                            timeout,
                            test_log_dir,
                            log_lock,
                        ),
                        suite_deadline,
                        args.test_timeout,
                        test_log_dir,
                    )
                    status = first_non_ok_status(test_results) or status
            elif status != "timeout":
                status = "fail"

    elapsed_sec = round(time.monotonic() - started, 3)
    test_results_path.write_text(json.dumps(test_results, indent=2, sort_keys=True) + "\n")
    result = {
        **base_result(generation, run_dir, command, result_backend(args)),
        "status": status,
        "exit_code": compile_exit_code if compile_exit_code != 0 else test_exit_code(test_results),
        "compile_exit_code": compile_exit_code,
        "elapsed_sec": elapsed_sec,
        "timeout_sec": args.timeout,
        "test_timeout_sec": args.test_timeout,
        "test_jobs": args.test_jobs,
        "tests_runner": args.tests_runner,
        "sanitizer": selected_sanitizer(args) if is_sanitizer_runner(args) else None,
        "sanitizer_build_std": sanitizer_needs_build_std(selected_sanitizer(args))
        if is_sanitizer_runner(args)
        else False,
        "rustflags": env.get("RUSTFLAGS"),
        "asan_options": env.get("ASAN_OPTIONS") if selected_sanitizer(args) == "address" else None,
        "tsan_options": env.get("TSAN_OPTIONS") if selected_sanitizer(args) == "thread" else None,
        "miri_flags": env.get("MIRIFLAGS") if args.tests_runner == "miri" else None,
        "test_binary": str(binary) if binary else None,
        "test_result_path": str(test_results_path),
        "stdout_tail": tail(stdout_path),
        "stderr_tail": tail(stderr_path),
        "first_error": first_error(stderr_path) or first_error(stdout_path),
        "backend_output_count": int(float(generation.get("generated_test_module_count") or 0)),
        "available_generated_tests": len(available_test_names),
        "selected_generated_tests": len(test_names),
        "cargo_test_result_sections": len(test_results),
        "cargo_test_ok_sections": sum(1 for result in test_results if result["status"] == "ok"),
        "tests_passed": sum(1 for result in test_results if result["status"] == "ok"),
        "tests_failed": sum(1 for result in test_results if result["status"] == "fail"),
        "tests_timeout": sum(1 for result in test_results if result["status"] == "timeout"),
        "tests_ignored": 0,
        "tests_measured": 0,
        "tests_filtered_out": 0,
    }
    (run_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def suite_deadline_from_timeout(started, timeout):
    if timeout is None or timeout <= 0:
        return None
    return started + timeout


def remaining_suite_timeout(deadline):
    if deadline is None:
        return None
    return max(0.0, deadline - time.monotonic())


def suite_timed_out(deadline):
    remaining = remaining_suite_timeout(deadline)
    return remaining is not None and remaining <= 0


def bounded_test_timeout(test_timeout, deadline):
    remaining = remaining_suite_timeout(deadline)
    if remaining is None:
        return test_timeout
    if test_timeout is None or test_timeout <= 0:
        return remaining
    return min(test_timeout, remaining)


def suite_timeout_result(test_name, log_dir):
    return test_result_row(
        test_name,
        "timeout",
        -999,
        0.0,
        b"",
        b"[suite-execute] suite timeout before running this generated test\n",
        log_dir,
    )


def run_generated_tests(test_names, test_jobs, run_one, deadline=None, test_timeout=0, log_dir=None):
    if test_jobs <= 1 or len(test_names) <= 1:
        results = []
        for index, test_name in enumerate(test_names):
            timeout = bounded_test_timeout(test_timeout, deadline)
            if timeout is not None and timeout <= 0:
                results.extend(suite_timeout_result(name, log_dir) for name in test_names[index:])
                break
            results.append(run_one(test_name, timeout))
        return results

    results = [None] * len(test_names)
    next_index = 0
    index_lock = threading.Lock()

    def worker():
        nonlocal next_index
        while True:
            with index_lock:
                if next_index >= len(test_names):
                    return
                index = next_index
                next_index += 1
            timeout = bounded_test_timeout(test_timeout, deadline)
            if timeout is not None and timeout <= 0:
                results[index] = suite_timeout_result(test_names[index], log_dir)
                continue
            results[index] = run_one(test_names[index], timeout)

    with ThreadPoolExecutor(max_workers=test_jobs) as executor:
        futures = [executor.submit(worker) for _ in range(min(test_jobs, len(test_names)))]
        for future in as_completed(futures):
            future.result()
    return [
        result if result is not None else suite_timeout_result(test_names[index], log_dir)
        for index, result in enumerate(results)
    ]


def first_non_ok_status(test_results):
    for result in test_results:
        if result["status"] != "ok":
            return result["status"]
    return None


def run_one_generated_test_with_miri(
    base_command, test_name, harness, env, stdout, stderr, timeout, log_dir, log_lock=None
):
    command = [*base_command, test_name, "--", "--exact", "--nocapture"]
    write_log(stdout, f"\n[suite-execute] running {' '.join(command)}\n", log_lock)
    started = time.monotonic()
    status = "ok"
    exit_code, captured_stdout, captured_stderr, timed_out = run_command_capture(
        command,
        cwd=harness,
        env=env,
        timeout=timeout,
    )
    if timed_out:
        status = "timeout"
        write_log(
            stderr,
            f"\n[suite-execute] miri timeout after {timeout}s: {test_name}\n",
            log_lock,
        )
    elif exit_code != 0:
        status = "fail"
        write_log(
            stderr,
            f"\n[suite-execute] miri exit {exit_code}: {test_name}\n",
            log_lock,
        )
    return test_result_row(
        test_name,
        status,
        exit_code,
        round(time.monotonic() - started, 3),
        captured_stdout,
        captured_stderr,
        log_dir,
    )


def run_one_generated_test(binary, test_name, harness, env, stdout, stderr, timeout, log_dir, log_lock=None):
    command = [str(binary), test_name, "--exact", "--nocapture"]
    write_log(stdout, f"\n[suite-execute] running {' '.join(command)}\n", log_lock)
    started = time.monotonic()
    status = "ok"
    exit_code, captured_stdout, captured_stderr, timed_out = run_command_capture(
        command,
        cwd=harness,
        env=env,
        timeout=timeout,
    )
    if timed_out:
        status = "timeout"
        write_log(
            stderr,
            f"\n[suite-execute] timeout after {timeout}s: {test_name}\n",
            log_lock,
        )
    elif exit_code != 0:
        status = "fail"
        write_log(
            stderr,
            f"\n[suite-execute] exit {exit_code}: {test_name}\n",
            log_lock,
        )
    return test_result_row(
        test_name,
        status,
        exit_code,
        round(time.monotonic() - started, 3),
        captured_stdout,
        captured_stderr,
        log_dir,
    )


def test_result_row(
    test_name, status, exit_code, elapsed_sec, captured_stdout, captured_stderr, log_dir
):
    stdout_text = decode_tail(captured_stdout)
    stderr_text = decode_tail(captured_stderr)
    row = {
        "name": test_name,
        "status": status,
        "exit_code": exit_code,
        "elapsed_sec": elapsed_sec,
        "stdout_tail": stdout_text,
        "stderr_tail": stderr_text,
        "first_error": first_error_text(stderr_text) or first_error_text(stdout_text),
        "unsoundness_kind": classify_unsoundness(stdout_text, stderr_text, status, exit_code),
    }
    if status != "ok":
        row.update(write_test_failure_logs(log_dir, test_name, captured_stdout, captured_stderr))
    return row


def write_log(handle, text, log_lock):
    data = text.encode()
    if log_lock is None:
        handle.write(data)
        handle.flush()
        return
    with log_lock:
        handle.write(data)
        handle.flush()


def run_command(command, cwd, env, stdout, stderr, timeout):
    if timeout is not None and timeout <= 0:
        timeout = None
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=stdout,
        stderr=stderr,
        start_new_session=True,
    )
    process_started_at = process_start_time(process.pid)
    try:
        exit_code = process.wait(timeout=timeout)
        return subprocess.CompletedProcess(command, exit_code)
    except subprocess.TimeoutExpired:
        kill_process_tree(process, process_started_at)
        raise


def run_command_capture(command, cwd, env, timeout):
    if timeout is not None and timeout <= 0:
        timeout = None
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    process_started_at = process_start_time(process.pid)
    try:
        captured_stdout, captured_stderr = process.communicate(timeout=timeout)
        return process.returncode, captured_stdout, captured_stderr, False
    except subprocess.TimeoutExpired:
        kill_process_tree(process, process_started_at)
        captured_stdout, captured_stderr = process.communicate()
        return -999, captured_stdout, captured_stderr, True


def run_command_with_stdin_capture(command, cwd, env, stdin_bytes, timeout):
    if timeout is not None and timeout <= 0:
        timeout = None
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    process_started_at = process_start_time(process.pid)
    try:
        captured_stdout, captured_stderr = process.communicate(
            input=stdin_bytes, timeout=timeout
        )
        return process.returncode, captured_stdout, captured_stderr, False
    except subprocess.TimeoutExpired:
        kill_process_tree(process, process_started_at)
        captured_stdout, captured_stderr = process.communicate()
        return -999, captured_stdout, captured_stderr, True


def kill_process_group(process):
    kill_process_tree(process)


def kill_process_tree(process, process_started_at=None):
    descendants = process_descendants(process.pid)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    for pid in sorted(descendants, reverse=True):
        if process_started_at is not None and not process_is_same_or_newer(pid, process_started_at):
            continue
        kill_pid(pid)
    kill_pid(process.pid)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        kill_pid(process.pid)
        process.wait()


def process_descendants(root_pid):
    children = defaultdict(list)
    for pid in proc_pids():
        ppid = proc_ppid(pid)
        if ppid is not None:
            children[ppid].append(pid)

    descendants = set()
    stack = list(children.get(root_pid, []))
    while stack:
        pid = stack.pop()
        if pid in descendants:
            continue
        descendants.add(pid)
        stack.extend(children.get(pid, []))
    return descendants


def proc_pids():
    proc = Path("/proc")
    if not proc.exists():
        return []
    return [int(path.name) for path in proc.iterdir() if path.name.isdigit()]


def proc_ppid(pid):
    try:
        stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
    except OSError:
        return None
    try:
        return int(stat.rsplit(") ", 1)[1].split()[1])
    except (IndexError, ValueError):
        return None


def process_start_time(pid):
    try:
        stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
    except OSError:
        return None
    try:
        return int(stat.rsplit(") ", 1)[1].split()[19])
    except (IndexError, ValueError):
        return None


def process_is_same_or_newer(pid, started_at):
    current_started_at = process_start_time(pid)
    return current_started_at is not None and current_started_at >= started_at


def kill_pid(pid):
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except PermissionError:
        pass


def discover_test_binary(stdout_path, target_dir):
    for line in stdout_path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        if not message.get("executable"):
            continue
        target = message.get("target") or {}
        executable = Path(message["executable"])
        profile = message.get("profile") or {}
        if "test" in target.get("kind", []) or profile.get("test") or executable.name.startswith("foursafer_test-"):
            return executable

    deps = target_dir / "debug" / "deps"
    candidates = [
        path
        for path in deps.glob("foursafer_test-*")
        if path.is_file() and os.access(path, os.X_OK) and "." not in path.name
    ]
    candidates.extend(
        path
        for path in target_dir.glob("*/debug/deps/foursafer_test-*")
        if path.is_file() and os.access(path, os.X_OK) and "." not in path.name
    )
    return sorted(candidates)[-1] if candidates else None


def generated_test_names(harness):
    tests = []
    for path in sorted((harness / "src").glob("test_*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        for name in re.findall(r"fn (generated_test_\d+)\s*\(", text):
            tests.append(f"{path.stem}::{name}")
    return tests


def generated_kani_proof_names(harness):
    proofs = []
    for path in sorted((harness / "src").glob("test_*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        proofs.extend(re.findall(r"fn (kani_test_\d+)\s*\(", text))
    return proofs


def test_exit_code(test_results):
    if any(result["status"] == "timeout" for result in test_results):
        return -999
    if any(result["status"] == "fail" for result in test_results):
        return 1
    return 0


def first_result_error(results):
    for result in results:
        if result.get("first_error"):
            return result["first_error"]
        if result.get("status") in ("fail", "timeout"):
            return f"{result.get('name')}: {result.get('status')}"
    return None


def prepare_package_tmp_dirs(manifest):
    output = subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--manifest-path",
            str(manifest),
            "--locked",
            "--format-version",
            "1",
        ],
        cwd=manifest.parent,
    )
    metadata = json.loads(output)
    for package in metadata.get("packages", []):
        package_manifest = Path(package["manifest_path"])
        (package_manifest.parent / "target" / "tmp").mkdir(parents=True, exist_ok=True)


def execution_metrics(backend, stdout_path, stderr_path, generation, args, kani_log_path=None):
    text = "\n".join(
        [
            tail(stdout_path, 200000),
            tail(stderr_path, 200000),
            tail(kani_log_path, 200000),
        ]
    )
    metrics = {
        "backend_output_count": int(float(generation.get("generated_fuzz_target_count") or 0))
        if backend == "fuzz"
        else int(float(generation.get("generated_test_module_count") or 0)),
    }
    if backend == "tests":
        metrics.update(parse_cargo_test_metrics(text))
    elif backend == "fuzz":
        metrics.update(
            {
                "fuzz_targets_executed": 1 if args.fuzz_targets == "first" else 0,
                "fuzz_execs_done": parse_afl_execs_done(text),
            }
        )
    elif backend == "kani":
        metrics.update(
            {
                "kani_mode": args.kani_mode,
                "kani_unsupported_constructs": parse_kani_unsupported_constructs(text),
            }
        )
    return metrics


def parse_cargo_test_metrics(text):
    results = re.findall(
        r"test result: (?P<status>\w+)\. (?P<passed>\d+) passed; (?P<failed>\d+) failed; "
        r"(?P<ignored>\d+) ignored; (?P<measured>\d+) measured; (?P<filtered>\d+) filtered out",
        text,
    )
    passed = failed = ignored = measured = filtered = 0
    ok_results = 0
    for status, p, f, i, m, flt in results:
        passed += int(p)
        failed += int(f)
        ignored += int(i)
        measured += int(m)
        filtered += int(flt)
        ok_results += int(status == "ok")
    return {
        "cargo_test_result_sections": len(results),
        "cargo_test_ok_sections": ok_results,
        "tests_passed": passed,
        "tests_failed": failed,
        "tests_ignored": ignored,
        "tests_measured": measured,
        "tests_filtered_out": filtered,
    }


def parse_afl_execs_done(text):
    match = re.search(r"execs_done\s*:\s*(?P<execs>\d+)", text)
    if not match:
        return 0
    return int(match.group("execs"))


def parse_kani_unsupported_constructs(text):
    return sorted(set(re.findall(r"- ([a-zA-Z0-9_ ]+) \(\d+\)", text)))


def parse_env_overrides(values):
    overrides = {}
    for value in values:
        if "=" not in value:
            raise ValueError(f"--env expects KEY=VALUE, got {value!r}")
        key, env_value = value.split("=", 1)
        overrides[key] = env_value
    return overrides


def apply_default_dynamic_library_paths(env):
    default_paths = [Path("/opt/anaconda3/lib")]
    existing_paths = [path for path in env.get("LD_LIBRARY_PATH", "").split(":") if path]
    prepend = [str(path) for path in default_paths if path.is_dir() and str(path) not in existing_paths]
    if prepend:
        env["LD_LIBRARY_PATH"] = ":".join([*prepend, *existing_paths])


def is_sanitizer_runner(args):
    return args.tests_runner in ("asan", "sanitizer")


def selected_sanitizer(args):
    if args.tests_runner == "asan":
        return "address"
    return args.sanitizer


def selected_fuzz_sanitizer(args):
    sanitizer = getattr(args, "fuzz_sanitizer", "address")
    if sanitizer == "none":
        return None
    return sanitizer


def sanitizer_needs_build_std(sanitizer):
    return sanitizer in {"thread", "memory"}


def apply_sanitizer_env(env, sanitizer):
    if sanitizer == "address":
        env["ASAN_OPTIONS"] = merge_colon_options(
            env.get("ASAN_OPTIONS"),
            {
                "abort_on_error": "1",
                "detect_leaks": "0",
                "symbolize": "0",
            },
        )
    elif sanitizer == "memory":
        env["MSAN_OPTIONS"] = merge_colon_options(
            env.get("MSAN_OPTIONS"),
            {
                "halt_on_error": "1",
            },
        )
    elif sanitizer == "thread":
        env["TSAN_OPTIONS"] = merge_colon_options(
            env.get("TSAN_OPTIONS"),
            {
                "halt_on_error": "1",
            },
        )


def apply_afl_sanitizer_env(env, sanitizer):
    if sanitizer == "address":
        env["AFL_USE_ASAN"] = "1"
    elif sanitizer == "memory":
        env["AFL_USE_MSAN"] = "1"
    elif sanitizer == "thread":
        env["AFL_USE_TSAN"] = "1"


def fuzz_command_env_exports(env):
    names = [
        "AFL_EXIT_WHEN_DONE",
        "AFL_NO_AFFINITY",
        "AFL_USE_ASAN",
        "AFL_USE_MSAN",
        "AFL_USE_TSAN",
        "CARGO_TARGET_DIR",
        "RUSTFLAGS",
        "ASAN_OPTIONS",
        "MSAN_OPTIONS",
        "TSAN_OPTIONS",
    ]
    return [
        f"export {name}={shlex.quote(env[name])}"
        for name in names
        if env.get(name)
    ]


def append_space_flag(existing, flag):
    if not existing:
        return flag
    parts = existing.split()
    if flag in parts:
        return existing
    return f"{existing} {flag}"


def merge_colon_options(existing, overrides):
    options = {}
    if existing:
        for part in existing.split(":"):
            if not part:
                continue
            key, _, value = part.partition("=")
            options[key] = value
    options.update(overrides)
    return ":".join(f"{key}={value}" for key, value in options.items())


def crate_key(row):
    return f"{row['crate']}-{row['version']}"


def tail(path, limit=5000):
    if path is None or not path.exists():
        return ""
    with path.open("rb") as handle:
        handle.seek(0, os.SEEK_END)
        size = handle.tell()
        handle.seek(max(0, size - limit), os.SEEK_SET)
        return handle.read(limit).decode("utf-8", "replace")


def first_error(path, limit=5_000_000):
    if path is None or not path.exists():
        return None
    with path.open("rb") as handle:
        data = handle.read(limit)
    for line in data.decode("utf-8", "replace").splitlines():
        stripped = line.strip()
        if stripped.startswith("error:") or re.match(r"^error\[[A-Za-z0-9]+\]:", stripped):
            return stripped
    return None


def decode_tail(data, limit=5000):
    if not data:
        return ""
    return data[-limit:].decode("utf-8", "replace")


def first_error_text(text):
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("error:") or re.match(r"^error\[[A-Za-z0-9]+\]:", stripped):
            return stripped
    return None


def classify_unsoundness(stdout_text, stderr_text, status, exit_code):
    text = "\n".join([stdout_text or "", stderr_text or ""])
    lowered = text.lower()
    if status == "timeout":
        return "timeout"
    if "unsafe precondition(s) violated" in lowered:
        return "unsafe_precondition"
    if "memorysanitizer" in lowered or "use-of-uninitialized-value" in lowered:
        return "memory_sanitizer"
    if "addresssanitizer" in lowered:
        return "address_sanitizer"
    if "threadsanitizer" in lowered or "data race" in lowered:
        return "thread_sanitizer"
    if "undefinedbehaviorsanitizer" in lowered:
        return "undefined_behavior_sanitizer"
    if "error: undefined behavior:" in lowered or "miri has detected undefined behavior" in lowered:
        if "uninitialized" in lowered or "invalid value" in lowered:
            return "miri_ub_uninitialized_or_invalid_value"
        if "out-of-bounds" in lowered or "out of bounds" in lowered:
            return "miri_ub_out_of_bounds"
        if "dangling" in lowered or "use-after-free" in lowered:
            return "miri_ub_use_after_free"
        if "misaligned" in lowered:
            return "miri_ub_misaligned"
        return "miri_undefined_behavior"
    if "is uninitialized" in lowered or "using uninitialized data" in lowered:
        return "uninitialized"
    if "out-of-bounds" in lowered or "out of bounds" in lowered:
        return "out_of_bounds"
    if "dangling" in lowered or "use-after-free" in lowered:
        return "use_after_free"
    if "misaligned" in lowered:
        return "misaligned_access"
    if exit_code and exit_code < 0:
        return "signal"
    if "attempt to" in lowered and "overflow" in lowered:
        return "arithmetic_overflow"
    if "panicked at" in lowered:
        return "panic"
    if status != "ok":
        return "runtime_failure"
    return None


def write_test_failure_logs(log_dir, test_name, captured_stdout, captured_stderr):
    log_dir.mkdir(parents=True, exist_ok=True)
    stem = safe_log_stem(test_name)
    stdout_path = log_dir / f"{stem}.stdout.log"
    stderr_path = log_dir / f"{stem}.stderr.log"
    stdout_path.write_bytes(captured_stdout or b"")
    stderr_path.write_bytes(captured_stderr or b"")
    return {
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
    }


def safe_log_stem(test_name):
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", test_name)[:180]


def gather_existing_results(out_dir):
    rows = []
    paths = {
        *out_dir.glob("*/*/*/result.json"),
        *out_dir.glob("*/*/*/*/result.json"),
    }
    for path in sorted(paths):
        try:
            rows.append(json.loads(path.read_text()))
        except (FileNotFoundError, json.JSONDecodeError):
            continue
    return rows


def write_summaries(rows, out_dir):
    out_dir.mkdir(parents=True, exist_ok=True)
    write_json(out_dir / "all_results.json", rows)
    write_csv(out_dir / "all_results.csv", rows)
    summary_rows = summarize(rows)
    write_csv(out_dir / "summary_by_backend.csv", summary_rows)
    write_markdown_summary(out_dir / "summary.md", summary_rows, rows)
    unsoundness_cases = gather_unsoundness_cases(rows)
    write_json(out_dir / "unsoundness_cases.json", unsoundness_cases)
    write_csv(out_dir / "unsoundness_cases.csv", unsoundness_cases)
    write_unsoundness_markdown(out_dir / "unsoundness_cases.md", unsoundness_cases)


def write_experiment_summaries(rows, out_dir):
    groups = defaultdict(list)
    for row in rows:
        experiment = row.get("experiment")
        if experiment:
            groups[experiment].append(row)
    for experiment, group in groups.items():
        write_summaries(group, out_dir / experiment)


def summarize(rows):
    groups = defaultdict(list)
    for row in rows:
        groups[(row.get("experiment"), row.get("config"), row.get("backend"))].append(row)
    summary = []
    for (experiment, config, backend), group in sorted(groups.items()):
        status_counts = defaultdict(int)
        for row in group:
            status_counts[row.get("status", "unknown")] += 1
        summary.append(
            {
                "experiment": experiment,
                "config": config,
                "backend": backend,
                "runs": len(group),
                "ok": status_counts["ok"],
                "fail": status_counts["fail"],
                "timeout": status_counts["timeout"],
                "median_elapsed_sec": median(row.get("elapsed_sec") for row in group),
                "total_generation_sequences": sum(int(row.get("generation_sequence_count") or 0) for row in group),
                "tests_passed": sum(int(row.get("tests_passed") or 0) for row in group),
                "tests_failed": sum(int(row.get("tests_failed") or 0) for row in group),
                "tests_timeout": sum(int(row.get("tests_timeout") or 0) for row in group),
                "fuzz_targets_available": sum(int(row.get("fuzz_targets_available") or 0) for row in group),
                "fuzz_targets_executed": sum(int(row.get("fuzz_targets_executed") or 0) for row in group),
                "fuzz_targets_ok": sum(int(row.get("fuzz_targets_ok") or 0) for row in group),
                "fuzz_targets_fail": sum(int(row.get("fuzz_targets_fail") or 0) for row in group),
                "fuzz_targets_timeout": sum(int(row.get("fuzz_targets_timeout") or 0) for row in group),
                "fuzz_targets_skipped": sum(int(row.get("fuzz_targets_skipped") or 0) for row in group),
                "fuzz_crashes": sum(int(row.get("fuzz_crashes") or 0) for row in group),
                "fuzz_execs_done": sum(int(row.get("fuzz_execs_done") or 0) for row in group),
            }
        )
    return summary


def median(values):
    values = sorted(float(value) for value in values if value is not None)
    if not values:
        return None
    midpoint = len(values) // 2
    if len(values) % 2:
        return round(values[midpoint], 3)
    return round((values[midpoint - 1] + values[midpoint]) / 2.0, 3)


def write_json(path, rows):
    path.write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")


def write_csv(path, rows):
    if not rows:
        path.write_text("")
        return
    fieldnames = sorted({key for row in rows for key in row})
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=fieldnames,
            quoting=csv.QUOTE_MINIMAL,
            quotechar='"',
            escapechar="\\",
            lineterminator="\n",
        )
        writer.writeheader()
        for row in rows:
            writer.writerow(sanitize_csv_row(row))


def sanitize_csv_row(row):
    return {key: sanitize_csv_value(value) for key, value in row.items()}


def sanitize_csv_value(value):
    if isinstance(value, str):
        return value.replace("\0", "\\0")
    return value


def write_markdown_summary(path, rows, result_rows):
    lines = [
        "# Backend Execution Summary",
        "",
        "| experiment | config | backend | ok/fail/timeout | median sec | gen seq | tests passed/failed | fuzz ok/fail/timeout/skipped | fuzz crashes | AFL execs |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in rows:
        lines.append(
            f"| {row['experiment']} | {row['config']} | {row['backend']} | "
            f"{row['ok']}/{row['fail']}/{row['timeout']} | {row['median_elapsed_sec']} | "
            f"{row['total_generation_sequences']} | {row['tests_passed']}/{row['tests_failed']} "
            f"(timeout {row['tests_timeout']}) | "
            f"{row['fuzz_targets_ok']}/{row['fuzz_targets_fail']}/{row['fuzz_targets_timeout']}/{row['fuzz_targets_skipped']} "
            f"(of {row['fuzz_targets_available']}) | "
            f"{row['fuzz_crashes']} | {row['fuzz_execs_done']} |"
        )
    issue_rows = [
        row
        for row in result_rows
        if row.get("status") != "ok"
        or int(row.get("tests_failed") or 0) > 0
        or int(row.get("tests_timeout") or 0) > 0
    ]
    if issue_rows:
        lines.extend(
            [
                "",
                "## Non-Ok Runs",
                "",
                "| crate | backend | status | tests passed/failed/timeout | elapsed sec | first error |",
                "|---|---|---:|---:|---:|---|",
            ]
        )
        for row in issue_rows:
            lines.append(
                f"| {row.get('crate')}-{row.get('version')} | {row.get('backend')} | "
                f"{row.get('status')} | {row.get('tests_passed', 0)}/"
                f"{row.get('tests_failed', 0)}/{row.get('tests_timeout', 0)} | "
                f"{row.get('elapsed_sec')} | {markdown_cell(row.get('first_error') or '')} |"
            )
    path.write_text("\n".join(lines) + "\n")


def gather_unsoundness_cases(rows):
    cases = []
    for row in rows:
        backend = row.get("backend") or ""
        if backend.startswith("fuzz"):
            cases.extend(gather_fuzz_unsoundness_cases(row))
            continue
        if not backend.startswith("tests-"):
            continue
        test_result_path = row.get("test_result_path")
        if not test_result_path:
            continue
        path = Path(test_result_path)
        try:
            test_results = json.loads(path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            continue
        for test in test_results:
            if test.get("status") == "ok":
                continue
            stdout_tail = test.get("stdout_tail") or ""
            stderr_tail = test.get("stderr_tail") or ""
            kind = test.get("unsoundness_kind") or classify_unsoundness(
                stdout_tail,
                stderr_tail,
                test.get("status"),
                int(test.get("exit_code") or 0),
            )
            cases.append(
                {
                    "crate": row.get("crate"),
                    "version": row.get("version"),
                    "crate_key": f"{row.get('crate')}-{row.get('version')}",
                    "backend": backend,
                    "test": test.get("name"),
                    "status": test.get("status"),
                    "exit_code": test.get("exit_code"),
                    "elapsed_sec": test.get("elapsed_sec"),
                    "classification": kind,
                    "first_error": test.get("first_error")
                    or first_error_text(stderr_tail)
                    or first_error_text(stdout_tail),
                    "stderr_tail": stderr_tail[:1000],
                    "stdout_tail": stdout_tail[:1000],
                    "stdout_log": test.get("stdout_log"),
                    "stderr_log": test.get("stderr_log"),
                    "result_json": str(path.parent / "result.json"),
                    "test_results_json": str(path),
                }
            )
    cases.sort(key=lambda row: (row["crate_key"], row["backend"], row["test"] or ""))
    return cases


def gather_fuzz_unsoundness_cases(row):
    path_text = row.get("fuzz_target_results_path")
    if not path_text:
        return []
    path = Path(path_text)
    try:
        target_results = json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return []

    cases = []
    for target in target_results:
        cases.extend(fuzz_target_unsoundness_cases(row, path, target))
    return cases


def fuzz_target_unsoundness_cases(row, path, target):
    cases = []
    target_name = target.get("name")
    for source, status, exit_code, stdout_tail, stderr_tail in fuzz_target_observations(target):
        kind = classify_unsoundness(stdout_tail, stderr_tail, status, int(exit_code or 0))
        if kind not in HIGH_CONFIDENCE_FUZZ_UNSOUNDNESS:
            continue
        cases.append(
            {
                "crate": row.get("crate"),
                "version": row.get("version"),
                "crate_key": f"{row.get('crate')}-{row.get('version')}",
                "backend": row.get("backend"),
                "test": f"{target_name}:{source}",
                "status": status,
                "exit_code": exit_code,
                "elapsed_sec": target.get("elapsed_sec"),
                "classification": kind,
                "first_error": target.get("first_error")
                or first_error_text(stderr_tail)
                or first_error_text(stdout_tail),
                "stderr_tail": stderr_tail[:1000],
                "stdout_tail": stdout_tail[:1000],
                "stdout_log": target.get("stdout_log"),
                "stderr_log": target.get("stderr_log"),
                "result_json": str(path.parent / "result.json"),
                "test_results_json": str(path),
            }
        )
    return cases


def fuzz_target_observations(target):
    observations = [
        (
            "fuzz",
            target.get("status"),
            target.get("exit_code"),
            target.get("stdout_tail") or "",
            target.get("stderr_tail") or "",
        )
    ]
    early_stop_info = target.get("fuzz_early_stop_info") or {}
    if early_stop_info:
        observations.append(
            (
                "early-stop-replay",
                "fail",
                early_stop_info.get("replay_exit_code"),
                early_stop_info.get("replay_stdout_tail") or "",
                early_stop_info.get("replay_stderr_tail") or "",
            )
        )
    for seed in target.get("seed_validation_results") or []:
        observations.append(
            (
                f"seed:{seed.get('name')}",
                seed.get("status"),
                seed.get("exit_code"),
                seed.get("stdout_tail") or "",
                seed.get("stderr_tail") or "",
            )
        )
    return observations


def write_unsoundness_markdown(path, cases):
    lines = ["# Unsoundness Cases", ""]
    if not cases:
        lines.append("No non-ok generated test executions were recorded.")
    else:
        lines.extend(
            [
                "| crate | backend | classification | status | test | first error |",
                "|---|---|---|---:|---|---|",
            ]
        )
        for case in cases:
            lines.append(
                f"| {case['crate_key']} | {case['backend']} | {case['classification']} | "
                f"{case['status']} | `{markdown_cell(case.get('test') or '')}` | "
                f"{markdown_cell(case.get('first_error') or case.get('stderr_tail') or case.get('stdout_tail') or '')} |"
            )
    path.write_text("\n".join(lines) + "\n")


def markdown_cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ")[:240]


if __name__ == "__main__":
    raise SystemExit(main())
