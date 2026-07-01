#!/usr/bin/env python3
"""Analyze cargo-4safer synthesis and backend execution results.

The script combines three experiment artifacts:

* generation rows from ``all_results.csv``;
* per-crate backend execution ``result.json`` files;
* per-suite failure rows from ``unsoundness_cases.csv``.

Unsoundness is deduplicated by backend + crate + normalized target API.  That
keeps different target APIs separate even when they report the same UB source
location, while treating different monomorphizations of the same target API as
one finding.
"""

import argparse
import csv
import json
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

from experiment_common import (
    DEFAULT_EXPERIMENT,
    DEFAULT_EXPERIMENT_CONFIG,
    DEFAULT_EXPERIMENT_ROOT,
    REPO,
    analysis_out_path,
    execution_base_path,
    generation_results_path,
    load_experiment_config,
    repo_path,
    unsoundness_cases_path,
)

DEFAULT_GENERATION = DEFAULT_EXPERIMENT_ROOT / "runs" / DEFAULT_EXPERIMENT / "all_results.csv"
DEFAULT_EXEC_BASE = DEFAULT_EXPERIMENT_ROOT / "backend-execution" / DEFAULT_EXPERIMENT
DEFAULT_UNSOUNDNESS_CASES = (
    DEFAULT_EXPERIMENT_ROOT / "backend-execution" / DEFAULT_EXPERIMENT / "unsoundness_cases.csv"
)
DEFAULT_OUT_DIR = DEFAULT_EXPERIMENT_ROOT / "final_backend_summaries" / DEFAULT_EXPERIMENT
DEFAULT_GENERATION_BACKEND = "tests"

RUN_BACKENDS = {
    "asan": "tests-asan",
    "miri": "tests-miri",
    "fuzz": "fuzz-asan",
}

HIGH_CONFIDENCE_UNSOUNDNESS = {
    "address_sanitizer",
    "memory_sanitizer",
    "thread_sanitizer",
    "miri_undefined_behavior",
    "unsafe_precondition",
    "uninitialized",
    "out_of_bounds",
    "use_after_free",
    "misaligned_access",
    "undefined_behavior_sanitizer",
}


@dataclass(frozen=True)
class Config:
    generation: Path
    execution_base: Path
    unsoundness_cases: Path
    out_dir: Path
    generation_backend: str
    run_backends: dict[str, str]
    verify: bool
    audit_hidden_unsoundness: bool
    audit_full_logs: bool
    audit_csv: Path | None


def main():
    config = parse_args()
    config.out_dir.mkdir(parents=True, exist_ok=True)
    generation = load_generation_rows(config.generation, config.generation_backend)
    unsoundness = load_unsoundness_index(config, generation)
    outputs = []
    backend_rows = {}
    for label, backend in config.run_backends.items():
        rows = build_backend_rows(label, backend, generation, unsoundness, config)
        if config.verify:
            verify_backend_rows(label, backend, rows, generation, config)
        path = config.out_dir / f"{label}_summary.csv"
        write_csv(path, rows)
        outputs.append(path)
        backend_rows[label] = rows
    experiment_rows = build_experiment_rows(generation, backend_rows, unsoundness, config)
    experiment_path = config.out_dir / "experiment_summary.csv"
    write_experiment_csv(experiment_path, experiment_rows)
    outputs.append(experiment_path)
    if config.audit_hidden_unsoundness:
        audit_rows = audit_unsoundness_classification(config, generation)
        write_audit(config.audit_csv or config.out_dir / "unsoundness_classification_audit.csv", audit_rows)
    print("wrote " + " ".join(str(path) for path in outputs))


def parse_args():
    parser = argparse.ArgumentParser(
        description="Build final backend summaries from cargo-4safer experiment artifacts."
    )
    parser.add_argument(
        "--experiment-config",
        type=Path,
        default=DEFAULT_EXPERIMENT_CONFIG,
        help=f"experiment JSON containing analyze defaults (default: {DEFAULT_EXPERIMENT_CONFIG})",
    )
    parser.add_argument(
        "--generation",
        type=Path,
        default=None,
        help="generation all_results.csv; defaults to analyze.generation in --experiment-config",
    )
    parser.add_argument(
        "--execution-base",
        type=Path,
        default=None,
        help="directory containing backend result folders; defaults to analyze.execution_base",
    )
    parser.add_argument(
        "--unsoundness-cases",
        type=Path,
        default=None,
        help="per-suite failure CSV to scan; defaults to analyze.unsoundness_cases",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=None,
        help="output directory for <label>_summary.csv files; defaults to analyze.out_dir",
    )
    parser.add_argument(
        "--generation-backend",
        default=None,
        help="generation backend whose suites are executed by ASan/Miri (default: tests)",
    )
    parser.add_argument(
        "--run-backend",
        action="append",
        metavar="LABEL=DIR",
        help=(
            "backend result mapping, repeatable; default is "
            + ",".join(f"{label}={backend}" for label, backend in RUN_BACKENDS.items())
        ),
    )
    parser.add_argument(
        "--no-verify",
        action="store_true",
        help="skip consistency checks before writing summaries",
    )
    parser.add_argument(
        "--audit-hidden-unsoundness",
        action="store_true",
        help="write a scan of non-counted rows whose full logs contain unsoundness markers",
    )
    parser.add_argument(
        "--audit-full-logs",
        action="store_true",
        help="with --audit-hidden-unsoundness, inspect every non-timeout full log instead of only likely cases",
    )
    parser.add_argument(
        "--audit-csv",
        type=Path,
        default=None,
        help="path for --audit-hidden-unsoundness output (default: OUT_DIR/unsoundness_classification_audit.csv)",
    )
    args = parser.parse_args()
    defaults = load_analyze_defaults(args.experiment_config)
    return Config(
        generation=(args.generation or defaults["generation"]).resolve(),
        execution_base=(args.execution_base or defaults["execution_base"]).resolve(),
        unsoundness_cases=(args.unsoundness_cases or defaults["unsoundness_cases"]).resolve(),
        out_dir=(args.out_dir or defaults["out_dir"]).resolve(),
        generation_backend=args.generation_backend or defaults["generation_backend"],
        run_backends=parse_run_backends(args.run_backend) if args.run_backend else defaults["run_backends"],
        verify=not args.no_verify,
        audit_hidden_unsoundness=args.audit_hidden_unsoundness,
        audit_full_logs=args.audit_full_logs,
        audit_csv=args.audit_csv,
    )


def load_analyze_defaults(path):
    payload = load_experiment_config(path)
    analyze = payload.get("analyze", {})
    experiment = payload.get("experiment") or DEFAULT_EXPERIMENT
    generation = repo_path(
        analyze.get("generation")
        or generation_results_path(payload, experiment),
        DEFAULT_GENERATION,
    )
    execution_base = repo_path(
        analyze.get("execution_base")
        or execution_base_path(payload, experiment),
        DEFAULT_EXEC_BASE,
    )
    return {
        "generation": generation,
        "execution_base": execution_base,
        "unsoundness_cases": repo_path(
            analyze.get("unsoundness_cases") or unsoundness_cases_path(payload, experiment),
            DEFAULT_UNSOUNDNESS_CASES,
        ),
        "out_dir": repo_path(
            analyze.get("out_dir") or analysis_out_path(payload, experiment),
            DEFAULT_OUT_DIR,
        ),
        "generation_backend": analyze.get("generation_backend") or DEFAULT_GENERATION_BACKEND,
        "run_backends": parse_analyze_run_backends(analyze.get("run_backends")),
    }


def parse_analyze_run_backends(value):
    if value is None:
        return dict(RUN_BACKENDS)
    if not isinstance(value, dict):
        raise SystemExit("analyze.run_backends must be an object")
    return {str(label): str(backend) for label, backend in value.items()}


def parse_run_backends(values):
    if not values:
        return dict(RUN_BACKENDS)
    parsed = {}
    for value in values:
        label, sep, backend = value.partition("=")
        if not sep or not label or not backend:
            raise SystemExit(f"invalid --run-backend {value!r}; expected LABEL=DIR")
        parsed[label] = backend
    return parsed


def load_generation_rows(path, generation_backend):
    rows = {}
    with path.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            if row.get("backend") != generation_backend:
                continue
            key = crate_key(row)
            rows[key] = row
    return rows


def load_unsoundness_index(config, generation):
    suite_targets = load_suite_target_index(generation)
    raw_counts = defaultdict(int)
    unique = defaultdict(set)
    kind_counts = defaultdict(lambda: defaultdict(int))
    unique_kind_counts = defaultdict(lambda: defaultdict(set))
    unique_sources = defaultdict(set)
    crashes = defaultdict(int)
    panics = defaultdict(int)
    unique_panics = defaultdict(set)
    seen_cases = set()

    for row in iter_unsoundness_rows(config):
        case_id = unsoundness_case_id(row)
        if case_id in seen_cases:
            continue
        seen_cases.add(case_id)
        backend = row.get("backend") or ""
        key = row.get("crate_key") or crate_key(row)
        if is_crash_exit(row.get("status"), row.get("exit_code")):
            crashes[(backend, key)] += 1
        text = case_text(row, force_full=False)
        kind = classify_case(text, row.get("classification"), row.get("status"), row.get("exit_code"))
        if should_read_full_case_text(row, kind, text):
            text = case_text(row, force_full=True)
            kind = classify_case(text, row.get("classification"), row.get("status"), row.get("exit_code"))
        target_api = target_api_for_case(row, key, generation, suite_targets)
        if kind == "panic":
            panics[(backend, key)] += 1
            unique_panics[(backend, key)].add(panic_fingerprint(backend, key, target_api, text))
        if kind not in HIGH_CONFIDENCE_UNSOUNDNESS:
            continue
        source = unsoundness_source(row, key, kind, text)
        fingerprint = unsoundness_fingerprint(backend, key, kind, target_api, source)
        raw_counts[(backend, key)] += 1
        unique[(backend, key)].add(fingerprint)
        kind_counts[(backend, key)][kind] += 1
        unique_kind_counts[(backend, key)][kind].add(fingerprint)
        unique_sources[(backend, key)].add(f"{target_api}@{kind}@{source}")

    return {
        "raw_counts": raw_counts,
        "unique": unique,
        "kind_counts": kind_counts,
        "unique_kind_counts": unique_kind_counts,
        "unique_sources": unique_sources,
        "crashes": crashes,
        "panics": panics,
        "unique_panics": unique_panics,
    }


def iter_unsoundness_rows(config):
    if config.unsoundness_cases.exists():
        with config.unsoundness_cases.open(newline="", encoding="utf-8") as handle:
            yield from csv.DictReader(handle)
    for backend in config.run_backends.values():
        if backend.startswith("fuzz"):
            yield from iter_fuzz_unsoundness_rows(backend, config)


def iter_fuzz_unsoundness_rows(backend, config):
    for result_path in iter_backend_result_paths(backend, config):
        try:
            result = json.loads(result_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            continue
        target_results_path = result.get("fuzz_target_results_path")
        if not target_results_path:
            continue
        target_results_path = Path(target_results_path)
        try:
            target_results = json.loads(target_results_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            continue
        for target in target_results:
            yield from fuzz_target_unsoundness_rows(result, result_path, target_results_path, target)


def fuzz_target_unsoundness_rows(result, result_path, target_results_path, target):
    target_name = target.get("name") or ""
    for source, status, exit_code, stdout_tail, stderr_tail in fuzz_target_observations(target):
        if not fuzz_observation_failed(status, exit_code):
            continue
        kind = classify_case(
            "\n".join(part for part in (stdout_tail, stderr_tail) if part),
            target.get("unsoundness_kind"),
            status,
            exit_code,
        )
        if kind not in HIGH_CONFIDENCE_UNSOUNDNESS:
            continue
        yield {
            "crate": result.get("crate"),
            "version": result.get("version"),
            "crate_key": crate_key(result),
            "backend": result.get("backend") or "fuzz",
            "test": f"{target_name}:{source}",
            "status": status,
            "exit_code": exit_code,
            "elapsed_sec": target.get("elapsed_sec"),
            "classification": kind,
            "first_error": target.get("first_error") or "",
            "stderr_tail": (stderr_tail or "")[:1000],
            "stdout_tail": (stdout_tail or "")[:1000],
            "stdout_log": target.get("stdout_log"),
            "stderr_log": target.get("stderr_log"),
            "result_json": str(result_path),
            "test_results_json": str(target_results_path),
        }


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


def fuzz_observation_failed(status, exit_code):
    if status in {"fail", "crash", "timeout"}:
        return True
    try:
        code = int(float(exit_code or 0))
    except ValueError:
        return False
    return code != 0


def unsoundness_case_id(row):
    return (
        row.get("backend") or "",
        row.get("crate_key") or crate_key(row),
        row.get("test") or "",
        row.get("status") or "",
        str(row.get("exit_code") or ""),
        row.get("classification") or "",
    )


def load_suite_target_index(generation):
    index = {}
    for key, row in generation.items():
        harness = Path(row.get("harness") or "")
        meta = harness / "foursafer_meta"
        sequence_path = next(meta.glob("*_api_sequences.json"), None) if meta.exists() else None
        functions_path = next(meta.glob("*_api_functions.json"), None) if meta.exists() else None
        if sequence_path is None or functions_path is None:
            continue
        try:
            sequences = json.loads(sequence_path.read_text())
            functions = json.loads(functions_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            continue
        functions_by_index = {
            int(function["index"]): function
            for function in functions
            if "index" in function
        }
        for sequence in sequences:
            sequence_index = int(sequence.get("index"))
            target_index = sequence.get("target")
            target = functions_by_index.get(int(target_index)) if target_index is not None else None
            if target is None:
                target_api = f"target_index:{target_index}"
            else:
                target_api = normalize_target_api(
                    target.get("full_name")
                    or target.get("instantiated_path")
                    or f"target_index:{target_index}"
                )
            index[(key, sequence_index)] = target_api
    return index


def case_text(row, force_full=False):
    if force_full and should_allow_full_log(row):
        parts = [
            read_log(row.get("stderr_log")),
            read_log(row.get("stdout_log")),
            row.get("first_error") or "",
            row.get("stderr_tail") or "",
            row.get("stdout_tail") or "",
        ]
    else:
        parts = [
            row.get("first_error") or "",
            row.get("stderr_tail") or "",
            row.get("stdout_tail") or "",
        ]
    return "\n".join(part for part in parts if part)


def should_read_full_case_text(row, kind, text):
    if not should_allow_full_log(row):
        return False
    compact_lowered = (text or "").lower()
    if kind in HIGH_CONFIDENCE_UNSOUNDNESS:
        return True
    if row.get("backend") == "tests-asan" and row.get("classification") == "signal":
        return True
    if "aborting due to" in compact_lowered and row.get("classification") == "runtime_failure":
        return True
    if row.get("classification") in {"panic", "runtime_failure"} and any(
        marker in compact_lowered
        for marker in (
            "undefined behavior",
            "unsafe precondition",
            "uninitialized",
            "addresssanitizer",
            "memorysanitizer",
            "threadsanitizer",
        )
    ):
        return True
    return False


def should_allow_full_log(row):
    if row.get("status") == "timeout":
        return False
    backend = row.get("backend") or ""
    return backend in {"tests-miri", "tests-asan"} or backend.startswith("fuzz")


def target_api_for_case(row, key, generation, suite_targets):
    sequence_index = suite_index(row.get("test") or "")
    if sequence_index is not None:
        target = suite_targets.get((key, sequence_index))
        if target:
            return target
    return "unknown_target"


def suite_index(test_name):
    match = re.search(r"fuzz_target_(\d+)(?::|$)", test_name or "")
    if match:
        return int(match.group(1))
    match = re.search(r"generated_test_(\d+)", test_name or "")
    if match:
        return int(match.group(1))
    match = re.search(r"test_[A-Za-z0-9_]+?(\d+)(?:::|$)", test_name or "")
    if match:
        return int(match.group(1))
    return None


def normalize_target_api(api):
    value = api or "unknown_target"
    value = re.sub(r"::<'[^>]*>", "", value)
    value = re.sub(r"::<[^>]*>", "", value)
    value = re.sub(r"<[^<>]*>", "", value)
    value = re.sub(r"\s+", "", value)
    return value


def read_log(path_text):
    if not path_text:
        return ""
    try:
        return Path(path_text).read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def classify_case(text, original_kind, status, exit_code):
    if status == "timeout" or original_kind == "timeout":
        return "timeout"
    lowered = primary_diagnostic_text(text).lower()
    if "attempted to leave type" in lowered and "uninitialized" in lowered:
        return "uninitialized"
    if "does not permit being left uninitialized" in lowered:
        return "uninitialized"
    if "invalid value" in lowered and "uninitialized" in lowered:
        return "uninitialized"
    if "unsafe precondition(s) violated" in lowered:
        return "unsafe_precondition"
    if "addresssanitizer" in lowered:
        return "address_sanitizer"
    if "memorysanitizer" in lowered or "use-of-uninitialized-value" in lowered:
        return "memory_sanitizer"
    if "threadsanitizer" in lowered or "data race" in lowered:
        return "thread_sanitizer"
    if "undefinedbehaviorsanitizer" in lowered or "runtime error:" in lowered:
        return "undefined_behavior_sanitizer"
    if "undefined behavior" in lowered:
        if "misaligned" in lowered or "alignment" in lowered:
            return "misaligned_access"
        if "uninitialized" in lowered:
            return "uninitialized"
        if "out-of-bounds" in lowered or "out of bounds" in lowered:
            return "out_of_bounds"
        if "dangling" in lowered or "use-after-free" in lowered:
            return "use_after_free"
        return "miri_undefined_behavior"
    if "is uninitialized" in lowered or "using uninitialized data" in lowered:
        return "uninitialized"
    if "out-of-bounds" in lowered or "out of bounds" in lowered:
        return "out_of_bounds"
    if "dangling" in lowered or "use-after-free" in lowered:
        return "use_after_free"
    if "misaligned" in lowered:
        return "misaligned_access"
    if is_arithmetic_overflow(lowered):
        return "arithmetic_overflow"
    if "panicked at" in lowered:
        return "panic"
    try:
        code = int(float(exit_code or 0))
    except ValueError:
        code = 0
    if code < 0 and code != -999:
        return "signal"
    return original_kind or "runtime_failure"


def primary_diagnostic_text(text):
    lowered = (text or "").lower()
    markers = [
        "error: undefined behavior",
        "error: unsafe precondition(s) violated",
        "error: memorysanitizer",
        "error: addresssanitizer",
        "error: threadsanitizer",
        "addresssanitizer:",
        "memorysanitizer:",
        "threadsanitizer:",
        "runtime error:",
        "attempted to leave type",
    ]
    starts = [lowered.find(marker) for marker in markers if lowered.find(marker) >= 0]
    if not starts:
        return text or ""
    start = min(starts)
    return (text or "")[start : start + 5000]


def is_crash_exit(status, exit_code):
    if status == "timeout":
        return False
    try:
        code = int(float(exit_code or 0))
    except ValueError:
        return False
    return code < 0 and code != -999


def is_arithmetic_overflow(lowered):
    return any(
        marker in lowered
        for marker in (
            "attempt to add with overflow",
            "attempt to subtract with overflow",
            "attempt to multiply with overflow",
            "attempt to divide with overflow",
            "attempt to calculate the remainder with overflow",
            "arithmetic overflow",
        )
    )


def unsoundness_source(row, key, kind, text):
    source = diagnostic_source_location(text)
    if source:
        return source
    source = stack_source_location(text)
    if source:
        return source
    return "message:" + normalized_error_message(text)


def diagnostic_source_location(text):
    window = primary_diagnostic_text(text)
    match = re.search(r"(?m)^\s*-->\s+(?P<path>.+?):(?P<line>\d+):(?P<col>\d+)", window)
    if not match:
        return None
    return normalize_source_location(match.group("path"), match.group("line"), match.group("col"))


def stack_source_location(text):
    matches = re.finditer(
        r"(?m)^\s+\d+:\s+(?P<fn>.+?)\n\s+at (?P<path>.+?):(?P<line>\d+):(?P<col>\d+)",
        text or "",
    )
    fallback = None
    for match in matches:
        source = normalize_source_location(match.group("path"), match.group("line"), match.group("col"))
        if source.startswith("crate::") or source.startswith("registry::"):
            return source
        if fallback is None:
            fallback = source
    return fallback


def normalize_source_location(path_text, line, col):
    path = path_text.strip()
    trophy_match = re.search(r"/trophy_crates/([^/]+)/(.+)$", path)
    if trophy_match:
        return f"crate::{trophy_match.group(2)}:{line}:{col}"
    registry_match = re.search(r"/registry/src/[^/]+/([^/]+)/(.+)$", path)
    if registry_match:
        return f"registry::{registry_match.group(1)}/{registry_match.group(2)}:{line}:{col}"
    if path.startswith("src/"):
        return f"harness::{path}:{line}:{col}"
    return f"{path}:{line}:{col}"


def normalized_error_message(text):
    normalized = primary_diagnostic_text(text).lower()
    match = re.search(r"error:\s+(.+)", normalized)
    if match:
        normalized = match.group(1)
    normalized = re.sub(r"/[^\s:]+", "<path>", normalized)
    normalized = re.sub(r"test_[a-z0-9_]+::generated_test_\d+", "<test>", normalized)
    normalized = re.sub(r"generated_test_\d+", "generated_test_<n>", normalized)
    normalized = re.sub(r"alloc\d+", "alloc<n>", normalized)
    normalized = re.sub(r"<\d+>", "<n>", normalized)
    normalized = re.sub(r"0x[0-9a-f]+", "0x<n>", normalized)
    normalized = re.sub(r"\b\d+\b", "<n>", normalized)
    normalized = re.sub(r"\s+", " ", normalized).strip()
    return normalized[:500]


def target_api_key(key, target_api):
    return (key, normalize_target_api(target_api))


def unsoundness_fingerprint(backend, key, kind, target_api, source):
    return (backend, *target_api_key(key, target_api))


def cross_backend_unsoundness_key(fingerprint):
    if isinstance(fingerprint, tuple) and len(fingerprint) >= 3:
        return fingerprint[1:3]
    return fingerprint


def panic_fingerprint(backend, key, target_api, text):
    return (backend, key, target_api, normalized_error_message(text))


def build_backend_rows(label, backend, generation, unsoundness, config):
    result_rows = load_result_rows(backend, config)
    rows = []
    for key in sorted(generation, key=crate_sort_key):
        gen = generation[key]
        result = result_rows.get(key, {})
        rows.append(summary_row(label, backend, key, gen, result, unsoundness))
    rows.append(total_row(label, backend, rows))
    return rows


def load_result_rows(backend, config):
    rows = {}
    for path in iter_backend_result_paths(backend, config):
        row = json.loads(path.read_text())
        rows[crate_key(row)] = row
    return rows


def iter_backend_result_paths(backend, config):
    seen = set()
    for path in sorted((config.execution_base / backend).glob("*/result.json")):
        resolved = path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        yield resolved


def summary_row(label, backend, key, gen, result, unsoundness):
    execution_elapsed = fnum(result.get("elapsed_sec"))
    synthesis_elapsed = fnum(gen.get("elapsed_sec"))
    status = result.get("status") or "missing"
    return {
        "backend": label,
        "crate": gen.get("crate"),
        "version": gen.get("version"),
        "crate_key": key,
        "execution_status": status,
        "suites_synthesized": inum(gen.get("sequence_count")),
        "synthesis_time_sec": synthesis_elapsed,
        "execution_time_sec": execution_elapsed,
        "total_time_sec": round(synthesis_elapsed + execution_elapsed, 3),
        "available_suites": result_available_suites(backend, result),
        "selected_suites": result_selected_suites(backend, result),
        "passed": result_passed(backend, result),
        "failed": result_failed(backend, result),
        "timeout": result_timeout(backend, result),
        "run_failed": int(status == "fail"),
        "run_timeout": int(status == "timeout"),
        "crashes": result_crashes(backend, result, unsoundness["crashes"][(backend, key)]),
        "alarms": unsoundness["raw_counts"][(backend, key)],
        "unique_alarms": len(unsoundness["unique"][(backend, key)]),
        "panics": unsoundness["panics"][(backend, key)],
        "unique_panics": len(unsoundness["unique_panics"][(backend, key)]),
        "unsoundness": len(unsoundness["unique"][(backend, key)]),
        "unsoundness_kinds": format_kind_counts(unsoundness["unique_kind_counts"][(backend, key)]),
        "unsoundness_sources": format_sources(unsoundness["unique_sources"][(backend, key)]),
        "raw_unsoundness_kinds": format_kind_counts(unsoundness["kind_counts"][(backend, key)]),
        "compile_exit_code": result.get("compile_exit_code", ""),
        "exit_code": result.get("exit_code", ""),
        "test_timeout_sec": result_timeout_sec(backend, result),
        "first_error": result.get("first_error") or "",
    }


def result_available_suites(backend, result):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_targets_available"))
    return inum(result.get("available_generated_tests"))


def result_selected_suites(backend, result):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_targets_executed"))
    return inum(result.get("selected_generated_tests"))


def result_passed(backend, result):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_targets_ok"))
    return inum(result.get("tests_passed"))


def result_failed(backend, result):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_targets_fail"))
    return inum(result.get("tests_failed"))


def result_timeout(backend, result):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_targets_timeout"))
    return inum(result.get("tests_timeout"))


def result_timeout_sec(backend, result):
    if backend.startswith("fuzz"):
        return result.get("fuzz_target_timeout_sec", "")
    return result.get("test_timeout_sec", "")


def result_crashes(backend, result, fallback):
    if backend.startswith("fuzz"):
        return inum(result.get("fuzz_crashes"))
    return fallback


def total_row(label, backend, rows):
    total = {
        "backend": label,
        "crate": "total",
        "version": "",
        "crate_key": "total",
        "execution_status": format_status_counts(rows),
        "suites_synthesized": sum(inum(row["suites_synthesized"]) for row in rows),
        "synthesis_time_sec": round(sum(fnum(row["synthesis_time_sec"]) for row in rows), 3),
        "execution_time_sec": round(sum(fnum(row["execution_time_sec"]) for row in rows), 3),
        "total_time_sec": round(sum(fnum(row["total_time_sec"]) for row in rows), 3),
        "available_suites": sum(inum(row["available_suites"]) for row in rows),
        "selected_suites": sum(inum(row["selected_suites"]) for row in rows),
        "passed": sum(inum(row["passed"]) for row in rows),
        "failed": sum(inum(row["failed"]) for row in rows),
        "timeout": sum(inum(row["timeout"]) for row in rows),
        "run_failed": sum(inum(row["run_failed"]) for row in rows),
        "run_timeout": sum(inum(row["run_timeout"]) for row in rows),
        "crashes": sum(inum(row["crashes"]) for row in rows),
        "alarms": sum(inum(row["alarms"]) for row in rows),
        "unique_alarms": sum(inum(row["unique_alarms"]) for row in rows),
        "panics": sum(inum(row["panics"]) for row in rows),
        "unique_panics": sum(inum(row["unique_panics"]) for row in rows),
        "unsoundness": sum(inum(row["unsoundness"]) for row in rows),
        "unsoundness_kinds": merge_kind_count_column(rows, "unsoundness_kinds"),
        "unsoundness_sources": merge_sources(rows),
        "raw_unsoundness_kinds": merge_kind_count_column(rows, "raw_unsoundness_kinds"),
        "compile_exit_code": "",
        "exit_code": "",
        "test_timeout_sec": "",
        "first_error": "",
    }
    return total


def build_experiment_rows(generation, backend_rows, unsoundness, config):
    result_rows_by_label = {
        label: load_result_rows(backend, config)
        for label, backend in config.run_backends.items()
    }
    rows_by_label = {
        label: {row["crate_key"]: row for row in rows if row.get("crate_key") != "total"}
        for label, rows in backend_rows.items()
    }
    rows = []
    for key in sorted(generation, key=crate_sort_key):
        gen = generation[key]
        cross_backend_count = cross_backend_unsoundness_count(
            unsoundness,
            config.run_backends.values(),
            key,
        )
        row = {
            "crate": gen.get("crate"),
            "version": gen.get("version"),
            "crate_key": key,
            "suites": 0,
            "planner_sequences": inum(gen.get("sequence_count")),
            "synthesis_time_sec": fnum(gen.get("elapsed_sec")),
            "passed": 0,
            "failed": 0,
            "timeout": 0,
            "alarms": 0,
            "unique_alarms": 0,
            "panics": 0,
            "unique_panics": 0,
            "unsoundness": cross_backend_count,
            "cross_backend_unsoundness": cross_backend_count,
            "time_sec": fnum(gen.get("elapsed_sec")),
        }
        status_parts = []
        for label, backend in config.run_backends.items():
            backend_summary = rows_by_label.get(label, {}).get(key, {})
            result = result_rows_by_label.get(label, {}).get(key, {})
            prefix = sanitize_column_prefix(label)
            status = backend_summary.get("execution_status") or result.get("status") or "missing"
            row[f"{prefix}_status"] = status
            row[f"{prefix}_suites"] = result_selected_suites(backend, result) or inum(
                backend_summary.get("selected_suites")
            )
            row["suites"] = max(row["suites"], row[f"{prefix}_suites"])
            row[f"{prefix}_passed"] = result_passed(backend, result)
            row[f"{prefix}_failed"] = result_failed(backend, result)
            row[f"{prefix}_timeout"] = result_timeout(backend, result)
            row[f"{prefix}_alarms"] = unsoundness["raw_counts"][(backend, key)]
            row[f"{prefix}_unique_alarms"] = len(unsoundness["unique"][(backend, key)])
            row[f"{prefix}_panics"] = unsoundness["panics"][(backend, key)]
            row[f"{prefix}_unique_panics"] = len(unsoundness["unique_panics"][(backend, key)])
            row[f"{prefix}_unsoundness"] = len(unsoundness["unique"][(backend, key)])
            row[f"{prefix}_time_sec"] = fnum(result.get("elapsed_sec"))
            row[f"{prefix}_first_error"] = result.get("first_error") or ""

            status_parts.append(f"{prefix}={status}")
            row["passed"] += row[f"{prefix}_passed"]
            row["failed"] += row[f"{prefix}_failed"]
            row["timeout"] += row[f"{prefix}_timeout"]
            row["alarms"] += row[f"{prefix}_alarms"]
            row["unique_alarms"] += row[f"{prefix}_unique_alarms"]
            row["panics"] += row[f"{prefix}_panics"]
            row["unique_panics"] += row[f"{prefix}_unique_panics"]
            row["time_sec"] += row[f"{prefix}_time_sec"]
        row["status"] = ";".join(status_parts)
        row["time_sec"] = round(row["time_sec"], 3)
        row["synthesis_time_sec"] = round(row["synthesis_time_sec"], 3)
        rows.append(row)
    rows.append(total_experiment_row(rows, generation, unsoundness, config))
    return rows


def cross_backend_unsoundness_count(unsoundness, backends, key):
    unique_targets = set()
    for backend in backends:
        for fingerprint in unsoundness["unique"][(backend, key)]:
            unique_targets.add(cross_backend_unsoundness_key(fingerprint))
    return len(unique_targets)


def cross_backend_unsoundness_total(unsoundness, backends, keys):
    unique_targets = set()
    for key in keys:
        for backend in backends:
            for fingerprint in unsoundness["unique"][(backend, key)]:
                unique_targets.add(cross_backend_unsoundness_key(fingerprint))
    return len(unique_targets)


def total_experiment_row(rows, generation, unsoundness, config):
    cross_backend_count = cross_backend_unsoundness_total(
        unsoundness,
        config.run_backends.values(),
        generation.keys(),
    )
    total = {
        "crate": "total",
        "version": "",
        "crate_key": "total",
        "status": format_experiment_status_counts(rows, config),
        "suites": sum(inum(row["suites"]) for row in rows),
        "planner_sequences": sum(inum(row["planner_sequences"]) for row in rows),
        "synthesis_time_sec": round(sum(fnum(row["synthesis_time_sec"]) for row in rows), 3),
        "passed": sum(inum(row["passed"]) for row in rows),
        "failed": sum(inum(row["failed"]) for row in rows),
        "timeout": sum(inum(row["timeout"]) for row in rows),
        "alarms": sum(inum(row["alarms"]) for row in rows),
        "unique_alarms": sum(inum(row["unique_alarms"]) for row in rows),
        "panics": sum(inum(row["panics"]) for row in rows),
        "unique_panics": sum(inum(row["unique_panics"]) for row in rows),
        "unsoundness": cross_backend_count,
        "cross_backend_unsoundness": cross_backend_count,
        "time_sec": round(sum(fnum(row["time_sec"]) for row in rows), 3),
    }
    for label in config.run_backends:
        prefix = sanitize_column_prefix(label)
        total[f"{prefix}_status"] = format_backend_status_counts(rows, prefix)
        total[f"{prefix}_suites"] = sum(inum(row.get(f"{prefix}_suites")) for row in rows)
        total[f"{prefix}_passed"] = sum(inum(row.get(f"{prefix}_passed")) for row in rows)
        total[f"{prefix}_failed"] = sum(inum(row.get(f"{prefix}_failed")) for row in rows)
        total[f"{prefix}_timeout"] = sum(inum(row.get(f"{prefix}_timeout")) for row in rows)
        total[f"{prefix}_alarms"] = sum(inum(row.get(f"{prefix}_alarms")) for row in rows)
        total[f"{prefix}_unique_alarms"] = sum(inum(row.get(f"{prefix}_unique_alarms")) for row in rows)
        total[f"{prefix}_panics"] = sum(inum(row.get(f"{prefix}_panics")) for row in rows)
        total[f"{prefix}_unique_panics"] = sum(inum(row.get(f"{prefix}_unique_panics")) for row in rows)
        total[f"{prefix}_unsoundness"] = sum(inum(row.get(f"{prefix}_unsoundness")) for row in rows)
        total[f"{prefix}_time_sec"] = round(sum(fnum(row.get(f"{prefix}_time_sec")) for row in rows), 3)
        total[f"{prefix}_first_error"] = ""
    return total


def sanitize_column_prefix(label):
    return re.sub(r"[^A-Za-z0-9]+", "_", label).strip("_")


def format_experiment_status_counts(rows, config):
    parts = []
    for label in config.run_backends:
        prefix = sanitize_column_prefix(label)
        parts.append(f"{prefix}: {format_backend_status_counts(rows, prefix)}")
    return "; ".join(parts)


def format_backend_status_counts(rows, prefix):
    counts = defaultdict(int)
    for row in rows:
        counts[row.get(f"{prefix}_status") or "missing"] += 1
    return ",".join(f"{key}={counts[key]}" for key in sorted(counts))


def verify_backend_rows(label, backend, rows, generation, config):
    if not rows:
        raise SystemExit(f"{label}: no rows generated")
    data_rows = rows[:-1]
    total = rows[-1]
    numeric_fields = [
        "suites_synthesized",
        "synthesis_time_sec",
        "execution_time_sec",
        "total_time_sec",
        "available_suites",
        "selected_suites",
        "passed",
        "failed",
        "timeout",
        "run_failed",
        "run_timeout",
        "crashes",
        "alarms",
        "unique_alarms",
        "panics",
        "unique_panics",
        "unsoundness",
    ]
    errors = []
    for field in numeric_fields:
        expected = sum(fnum(row.get(field)) for row in data_rows)
        got = fnum(total.get(field))
        if abs(expected - got) > 0.001:
            errors.append(f"{field}: row sum {expected} != total {got}")

    result_rows = load_result_rows(backend, config)
    result_sums = {
        "execution_time_sec": sum(fnum(row.get("elapsed_sec")) for row in result_rows.values()),
        "passed": sum(result_passed(backend, row) for row in result_rows.values()),
        "failed": sum(result_failed(backend, row) for row in result_rows.values()),
        "timeout": sum(result_timeout(backend, row) for row in result_rows.values()),
    }
    for field, expected in result_sums.items():
        got = fnum(total.get(field))
        if abs(expected - got) > 0.001:
            errors.append(f"{field}: result.json sum {expected} != total {got}")

    expected_suites = sum(inum(row.get("sequence_count")) for row in generation.values())
    if inum(total.get("suites_synthesized")) != expected_suites:
        errors.append(
            f"suites_synthesized: generation sum {expected_suites} != total {total.get('suites_synthesized')}"
        )

    unsoundness_text = str(total.get("unsoundness_kinds") or "").lower()
    if "panic" in unsoundness_text or "overflow" in unsoundness_text:
        errors.append("panic/overflow appeared in counted unsoundness columns")

    if errors:
        raise SystemExit(f"{label}: verification failed\n" + "\n".join(errors))


def audit_unsoundness_classification(config, generation):
    suite_targets = load_suite_target_index(generation)
    rows = []
    if not config.unsoundness_cases.exists():
        return rows
    with config.unsoundness_cases.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            compact = case_text(row, force_full=False)
            compact_kind = classify_case(
                compact,
                row.get("classification"),
                row.get("status"),
                row.get("exit_code"),
            )
            if compact_kind in HIGH_CONFIDENCE_UNSOUNDNESS:
                continue
            full = compact
            full_kind = compact_kind
            full_has_marker = contains_unsoundness_marker(compact)
            if (
                config.audit_full_logs
                or full_has_marker
                or should_read_full_case_text(row, compact_kind, compact)
            ):
                full = case_text(row, force_full=True)
                full_kind = classify_case(
                    full,
                    row.get("classification"),
                    row.get("status"),
                    row.get("exit_code"),
                )
                full_has_marker = contains_unsoundness_marker(full)
            if full_kind in HIGH_CONFIDENCE_UNSOUNDNESS:
                continue
            if not full_has_marker:
                continue
            key = row.get("crate_key") or crate_key(row)
            rows.append(
                {
                    "backend": row.get("backend") or "",
                    "crate_key": key,
                    "test": row.get("test") or "",
                    "status": row.get("status") or "",
                    "classification": row.get("classification") or "",
                    "compact_kind": compact_kind,
                    "full_kind": full_kind,
                    "target_api": target_api_for_case(row, key, generation, suite_targets),
                    "first_error": normalized_error_message(full),
                    "stderr_log": row.get("stderr_log") or "",
                    "stdout_log": row.get("stdout_log") or "",
                }
            )
    return rows


def contains_unsoundness_marker(text):
    lowered = (text or "").lower()
    return any(
        marker in lowered
        for marker in (
            "undefined behavior",
            "unsafe precondition",
            "uninitialized",
            "addresssanitizer",
            "memorysanitizer",
            "threadsanitizer",
            "use-after-free",
            "out-of-bounds",
            "out of bounds",
            "misaligned",
        )
    )


def format_status_counts(rows):
    counts = defaultdict(int)
    for row in rows:
        counts[row["execution_status"]] += 1
    return ";".join(f"{key}={counts[key]}" for key in sorted(counts))


def format_kind_counts(counts):
    if not counts:
        return ""
    items = []
    for key in sorted(counts):
        value = counts[key]
        if isinstance(value, set):
            value = len(value)
        items.append(f"{key}={value}")
    return ";".join(items)


def merge_kind_count_column(rows, column):
    counts = defaultdict(int)
    for row in rows:
        for part in str(row.get(column) or "").split(";"):
            if not part:
                continue
            key, _, value = part.partition("=")
            counts[key] += int(value or 0)
    return format_kind_counts(counts)


def format_sources(sources):
    return ";".join(sorted(sources))


def merge_sources(rows):
    sources = set()
    for row in rows:
        for source in str(row.get("unsoundness_sources") or "").split(";"):
            if source:
                sources.add(source)
    return format_sources(sources)


def write_csv(path, rows):
    fields = [
        "backend",
        "crate",
        "version",
        "crate_key",
        "execution_status",
        "suites_synthesized",
        "synthesis_time_sec",
        "execution_time_sec",
        "total_time_sec",
        "available_suites",
        "selected_suites",
        "passed",
        "failed",
        "timeout",
        "run_failed",
        "run_timeout",
        "crashes",
        "alarms",
        "unique_alarms",
        "panics",
        "unique_panics",
        "unsoundness",
        "unsoundness_kinds",
        "unsoundness_sources",
        "raw_unsoundness_kinds",
        "compile_exit_code",
        "exit_code",
        "test_timeout_sec",
        "first_error",
    ]
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def write_experiment_csv(path, rows):
    fields = [
        "crate",
        "version",
        "crate_key",
        "status",
        "suites",
        "planner_sequences",
        "passed",
        "failed",
        "timeout",
        "alarms",
        "unique_alarms",
        "panics",
        "unique_panics",
        "unsoundness",
        "cross_backend_unsoundness",
        "synthesis_time_sec",
        "time_sec",
    ]
    backend_fields = []
    for row in rows:
        for key in row:
            if key in fields or key in backend_fields:
                continue
            if key.endswith("_first_error"):
                continue
            backend_fields.append(key)
    ordered_backend_fields = sorted(
        backend_fields,
        key=lambda field: (
            field.split("_", 1)[0],
            backend_field_order(field),
            field,
        ),
    )
    fields.extend(ordered_backend_fields)
    fields.extend(sorted({key for row in rows for key in row if key.endswith("_first_error")}))
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def backend_field_order(field):
    suffix_order = [
        "_status",
        "_suites",
        "_passed",
        "_failed",
        "_timeout",
        "_alarms",
        "_unique_alarms",
        "_panics",
        "_unique_panics",
        "_unsoundness",
        "_time_sec",
    ]
    for index, suffix in enumerate(suffix_order):
        if field.endswith(suffix):
            return index
    return len(suffix_order)


def write_audit(path, rows):
    fields = [
        "backend",
        "crate_key",
        "test",
        "status",
        "classification",
        "compact_kind",
        "full_kind",
        "target_api",
        "first_error",
        "stderr_log",
        "stdout_log",
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def crate_key(row):
    return f"{row.get('crate')}-{row.get('version')}"


def crate_sort_key(key):
    name, _, version = key.rpartition("-")
    return name, version


def inum(value):
    if value in (None, ""):
        return 0
    return int(float(value))


def fnum(value):
    if value in (None, ""):
        return 0.0
    return float(value)


if __name__ == "__main__":
    main()
