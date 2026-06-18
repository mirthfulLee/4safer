#!/usr/bin/env python3
"""Run paper-oriented cargo-4safer generation experiments.

The script reads misc/selected_crates.csv, fetches crate sources when needed,
runs cargo-4safer with one selected synthesis configuration and one or more
backends, and records per-run plus per-configuration metrics from foursafer_meta
JSON files.
"""

import argparse
import csv
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
DEFAULT_CSV = REPO / "misc/selected_crates.csv"
DEFAULT_SOURCES = REPO / "tmp/paper-experiments/sources"
DEFAULT_OUT = REPO / "tmp/paper-experiments/runs"
BIN = REPO / "target/debug/cargo-4safer"

BACKENDS = ("kani", "tests", "fuzz")
BACKEND_CHOICES = (*BACKENDS, "all")

EXPERIMENTS = {
    "obligation-sequence": [
        {
            "name": "od_sequence_m4_d0",
            "max_mutators": 4,
            "max_depth": 0,
            "max_sequences_per_target": 8,
            "max_basic_sequences_per_target": 8,
        },
    ],
    "ablation": [
        {
            "name": "od_sequence_full",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {},
        },
        {
            "name": "od_no_reuse_penalty",
            "max_mutators": 4,
            "max_depth": 0,
            "disable_reuse": True,
            "env": {},
        },
        {
            "name": "od_no_depth_penalty",
            "max_mutators": 4,
            "max_depth": 0,
            "disable_depth": True,
            "env": {},
        },
        {
            "name": "od_no_reuse_no_depth_penalty",
            "max_mutators": 4,
            "max_depth": 0,
            "disable_reuse": True,
            "disable_depth": True,
            "env": {},
        },
        {
            "name": "od_sequence_unlimited",
            "max_mutators": 4,
            "max_depth": 0,
            "disable_sequence_limits": True,
            "env": {},
        },
    ],
}


def config_names():
    return sorted({config["name"] for configs in EXPERIMENTS.values() for config in configs})


def load_selected_crates(csv_path):
    with csv_path.open("r", encoding="utf-8-sig", newline="") as handle:
        reader = csv.DictReader(handle)
        rows = []
        for index, row in enumerate(reader, start=1):
            name = (row.get("crate") or "").strip()
            version = (row.get("version") or "").strip()
            if not name or not version:
                continue
            rows.append(
                {
                    "crate": name,
                    "version": version,
                    "note": (row.get("note") or "").strip(),
                    "csv_index": index,
                }
            )
        return rows


def crate_key(crate):
    return f"{crate['crate']}-{crate['version']}"


def source_dir_for(sources_dir, crate):
    return sources_dir / crate_key(crate)


def ensure_source(sources_dir, crate, fetch):
    source_dir = source_dir_for(sources_dir, crate)
    if (source_dir / "Cargo.toml").exists():
        return source_dir
    if not fetch:
        raise RuntimeError(f"missing source for {crate_key(crate)} at {source_dir}")
    fetch_crate_source(sources_dir, crate)
    if not (source_dir / "Cargo.toml").exists():
        raise RuntimeError(f"downloaded {crate_key(crate)} but Cargo.toml was not found")
    return source_dir


def apply_source_preprocessing(source_dir, crate):
    steps = []
    ensure_rustc_tmp_dir(source_dir)
    steps.append("ensure-rustc-target-tmp-dir")
    if crate["crate"] == "hyper" and crate["version"] == "0.10.4":
        if preprocess_hyper_0_10_4(source_dir):
            steps.append("hyper-0.10.4-disable-deprecated-impl-attr")
        else:
            steps.append("hyper-0.10.4-disable-deprecated-impl-attr-already-applied")
    return steps


def ensure_rustc_tmp_dir(source_dir):
    (source_dir / "target" / "tmp").mkdir(parents=True, exist_ok=True)


def preprocess_hyper_0_10_4(source_dir):
    lib_rs = source_dir / "src" / "lib.rs"
    text = lib_rs.read_text()
    original = """macro_rules! deprecated {
    (#[$note:meta] $i:item) => (
        #[cfg_attr(has_deprecated, $note)]
        $i
    );
}
"""
    patched = """macro_rules! deprecated {
    (#[$note:meta] $i:item) => (
        $i
    );
}
"""
    if patched in text:
        return False
    if original not in text:
        raise RuntimeError("hyper 0.10.4 deprecated! macro did not match expected source")
    lib_rs.write_text(text.replace(original, patched))
    return True


def fetch_crate_source(sources_dir, crate):
    sources_dir.mkdir(parents=True, exist_ok=True)
    encoded_name = urllib.parse.quote(crate["crate"], safe="")
    encoded_version = urllib.parse.quote(crate["version"], safe="")
    url = f"https://crates.io/api/v1/crates/{encoded_name}/{encoded_version}/download"
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": "cargo-4safer-paper-experiments/0.1",
            "Accept": "application/x-tar",
        },
    )
    target_dir = source_dir_for(sources_dir, crate)
    with tempfile.TemporaryDirectory(prefix=".download-", dir=sources_dir) as tmp:
        archive = Path(tmp) / "crate.tar.gz"
        extract_dir = Path(tmp) / "extract"
        extract_dir.mkdir()
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                archive.write_bytes(response.read())
        except Exception:
            try:
                download_with_curl(url, archive)
            except Exception:
                download_with_cargo_download(crate, target_dir)
                return
        with tarfile.open(archive, "r:gz") as tar:
            safe_extract(tar, extract_dir)
        roots = [path for path in extract_dir.iterdir() if path.is_dir()]
        if len(roots) != 1:
            raise RuntimeError(f"unexpected archive layout for {crate_key(crate)}")
        install_source_root(roots[0], target_dir)


def download_with_cargo_download(crate, target_dir):
    cargo_download = shutil.which("cargo-download")
    if cargo_download is None:
        cargo = shutil.which("cargo")
        if cargo is None:
            raise RuntimeError("Python/curl downloads failed and cargo is not available")
        command = [cargo, "download"]
    else:
        command = [cargo_download]

    parent = target_dir.parent
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".cargo-download-", dir=parent) as tmp:
        subprocess.run(
            [
                *command,
                f"{crate['crate']}=={crate['version']}",
                "-x",
            ],
            cwd=tmp,
            check=True,
        )
        roots = [path for path in Path(tmp).iterdir() if path.is_dir()]
        if len(roots) != 1:
            raise RuntimeError(f"unexpected cargo-download layout for {crate_key(crate)}")
        install_source_root(roots[0], target_dir)


def install_source_root(source_root, target_dir):
    if target_dir.exists():
        shutil.rmtree(target_dir)
    shutil.move(str(source_root), target_dir)


def download_with_curl(url, archive):
    curl = shutil.which("curl")
    if curl is None:
        raise RuntimeError("Python download failed and curl is not available")
    subprocess.run(
        [
            curl,
            "--silent",
            "--show-error",
            "-L",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "-o",
            str(archive),
            url,
        ],
        check=True,
    )


def safe_extract(tar, destination):
    destination = destination.resolve()
    for member in tar.getmembers():
        member_path = (destination / member.name).resolve()
        if os.path.commonpath([str(destination), str(member_path)]) != str(destination):
            raise RuntimeError(f"refusing unsafe archive path: {member.name}")
        if member.issym() or member.islnk():
            raise RuntimeError(f"refusing archive link: {member.name}")
    tar.extractall(destination)


def build_tool(skip_build):
    if skip_build:
        if not BIN.exists():
            raise RuntimeError(f"missing {BIN}; run cargo build first")
        return
    subprocess.run(["cargo", "build", "--bin", "cargo-4safer"], cwd=REPO, check=True)


def run_generation(crate, source_dir, source_preprocessing, experiment, config, backends, args):
    backends = tuple(backends)
    result_paths = {
        backend: args.out / experiment / config["name"] / backend / crate_key(crate) / "result.json"
        for backend in backends
    }
    if not args.dry_run and not args.force and all(path.exists() for path in result_paths.values()):
        return [json.loads(path.read_text()) for path in result_paths.values()]

    multi_backend = len(backends) > 1
    if multi_backend:
        backend_label = "multi-" + "-".join(backends)
        work_dir = args.out / experiment / config["name"] / backend_label / crate_key(crate)
    else:
        work_dir = result_paths[backends[0]].parent

    if args.force and work_dir.exists():
        shutil.rmtree(work_dir)
    if args.force:
        for result_path in result_paths.values():
            result_dir = result_path.parent
            if result_dir.exists() and result_dir != work_dir:
                shutil.rmtree(result_dir)

    work_dir.mkdir(parents=True, exist_ok=True)
    for result_path in result_paths.values():
        result_path.parent.mkdir(parents=True, exist_ok=True)

    harness_root = work_dir / "harness"
    target_dir = work_dir / "target"
    stdout_path = work_dir / "stdout.log"
    stderr_path = work_dir / "stderr.log"

    max_mutators = args.max_mutators if args.max_mutators is not None else config["max_mutators"]
    max_depth = args.max_depth if args.max_depth is not None else config["max_depth"]
    max_sequences = args.max_sequences or 0
    max_sequences_per_wrapper = (
        args.max_sequences_per_wrapper
        if args.max_sequences_per_wrapper is not None
        else config.get("max_sequences_per_wrapper")
    )
    max_sequences_per_target = (
        args.max_sequences_per_target
        if args.max_sequences_per_target is not None
        else config.get("max_sequences_per_target")
    )
    max_basic_sequences_per_target = (
        args.max_basic_sequences_per_target
        if args.max_basic_sequences_per_target is not None
        else config.get("max_basic_sequences_per_target")
    )
    disable_sequence_limits = bool(args.disable_sequence_limits or config.get("disable_sequence_limits"))
    sequence_limit_mode = "sequence-unlimited" if disable_sequence_limits else "sequence-limited"

    command = [
        str(BIN),
        "4safer",
        "-C",
        str(source_dir),
        "-o",
        str(harness_root),
        "--max-mutators",
        str(max_mutators),
        "--max-depth",
        str(max_depth),
        "--max-sequences",
        str(max_sequences),
        "--timeout",
        str(args.timeout),
        "--log-level",
        args.log_level,
    ]
    if max_sequences_per_wrapper is not None:
        command.extend(["--max-sequences-per-wrapper", str(max_sequences_per_wrapper)])
    if max_sequences_per_target is not None:
        command.extend(["--max-sequences-per-target", str(max_sequences_per_target)])
    if max_basic_sequences_per_target is not None:
        command.extend([
            "--max-basic-sequences-per-target",
            str(max_basic_sequences_per_target),
        ])
    for backend in backends:
        command.extend(["--backend", backend])
    if args.check_harness:
        command.append("--check-harness")
    if config.get("disable_reuse"):
        command.append("--disable-reuse-penalty")
    if config.get("disable_depth"):
        command.append("--disable-depth-penalty")
    if disable_sequence_limits:
        command.append("--disable-sequence-limits")

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env.update(config.get("env", {}))
    env.update(parse_env_overrides(args.env))

    if args.dry_run:
        print(" ".join(command))
        return [
            {
                "crate": crate["crate"],
                "version": crate["version"],
                "note": crate["note"],
                "experiment": experiment,
                "config": config["name"],
                "backend": backend,
                "planner": "obligation-driven",
                "sequence_limit_mode": sequence_limit_mode,
                "status": "dry-run",
                "command": command,
            }
            for backend in backends
        ]

    started = time.monotonic()
    status = "ok"
    exit_code = 0
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            completed = subprocess.run(
                command,
                cwd=REPO,
                env=env,
                stdout=stdout,
                stderr=stderr,
                timeout=args.timeout + args.outer_timeout_grace,
            )
        exit_code = completed.returncode
        if exit_code != 0:
            status = "fail"
    except subprocess.TimeoutExpired:
        status = "outer-timeout"
        exit_code = -999

    elapsed_sec = round(time.monotonic() - started, 3)
    rows = []
    for backend in backends:
        harness = harness_root / backend if multi_backend else harness_root
        result = {
            "crate": crate["crate"],
            "version": crate["version"],
            "note": crate["note"],
            "source": str(source_dir),
            "source_preprocessing": source_preprocessing,
            "experiment": experiment,
            "config": config["name"],
            "backend": backend,
            "planner": "obligation-driven",
            "sequence_limit_mode": sequence_limit_mode,
            "max_mutators": max_mutators,
            "max_depth": max_depth,
            "max_sequences": max_sequences,
            "disable_sequence_limits": disable_sequence_limits,
            "disable_reuse": bool(config.get("disable_reuse")),
            "disable_depth": bool(config.get("disable_depth")),
            "env": {**config.get("env", {}), **parse_env_overrides(args.env)},
            "harness": str(harness),
            "status": status,
            "exit_code": exit_code,
            "elapsed_sec": elapsed_sec,
            "command": command,
            "stderr_tail": tail(stderr_path),
            "first_error": first_error(stderr_path),
        }
        result.update(read_metrics(harness, backend))
        result["generation_status"] = generation_status(result)
        result_paths[backend].write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        rows.append(result)
    return rows


def parse_env_overrides(values):
    overrides = {}
    for value in values:
        if "=" not in value:
            raise ValueError(f"--env expects KEY=VALUE, got {value!r}")
        key, env_value = value.split("=", 1)
        overrides[key] = env_value
    return overrides


def tail(path, limit=5000):
    if not path.exists():
        return ""
    return path.read_bytes()[-limit:].decode("utf-8", "replace")


def first_error(path):
    if not path.exists():
        return None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip()
        if stripped.startswith("error:") or re.match(r"^error\[[A-Za-z0-9]+\]:", stripped):
            return stripped
    return None


def generation_status(result):
    if result.get("status") == "dry-run":
        return "dry-run"
    if not result.get("meta_present"):
        return "no-metadata"
    if result.get("sequence_count", 0) > 0:
        return "generated"
    if result.get("target_summary_count", 0) > 0 or result.get("candidate_unsafe_wrapper_count", 0) > 0:
        return "generated-empty"
    return "metadata-only"


def find_meta(harness):
    meta_dir = harness / "foursafer_meta"
    if not meta_dir.exists():
        return None, None, None
    seqs = sorted(meta_dir.glob("*_api_sequences.json"))
    apis = sorted(meta_dir.glob("*_api_functions.json"))
    summaries = sorted(meta_dir.glob("*_target_summaries.json"))
    return (
        seqs[0] if seqs else None,
        apis[0] if apis else None,
        summaries[0] if summaries else None,
    )


def load_json(path, default):
    if path is None or not path.exists():
        return default
    return json.loads(path.read_text())


def read_metrics(harness, backend):
    seq_path, api_path, summary_path = find_meta(harness)
    metrics = empty_metrics()
    metrics.update(count_backend_outputs(harness, backend))
    if seq_path is None:
        return metrics

    sequences = load_json(seq_path, [])
    apis = load_json(api_path, [])
    summaries = load_json(summary_path, [])
    api_paths = {api.get("index"): api.get("full_name", str(api.get("index"))) for api in apis}
    target_counts = Counter(seq.get("target") for seq in sequences)
    mutator_counts = sequence_mutator_counts(sequences)

    chains = [
        [api_paths.get(index, str(index)) for index in seq.get("functions", [])]
        for seq in sequences
    ]
    shape_chains = [tuple(shape_path(part) for part in chain) for chain in chains]
    api_call_counter = Counter(api for chain in chains for api in chain)
    total_api_calls = sum(api_call_counter.values())
    covered_wrappers = unique_sequence_wrappers(sequences)
    target_wrappers = unique_api_wrappers(apis)
    api_safety_counts = Counter(api.get("api_safety", "unknown") for api in apis)

    metrics.update(
        {
            "meta_present": True,
            "api_count": len(apis),
            "supported_api_count": sum(1 for api in apis if api.get("supported")),
            "mono_api_count": sum(1 for api in apis if api.get("is_mono")),
            "api_safety_counts": dict(sorted(api_safety_counts.items())),
            "unsafe_block_api_count": api_safety_counts.get("unsafe_block", 0),
            "indirect_unsafe_api_count": api_safety_counts.get("indirect_unsafe", 0),
            "candidate_unsafe_wrapper_count": len(target_wrappers),
            "covered_unsafe_wrapper_count": len(covered_wrappers),
            "target_summary_count": len(summaries),
            "covered_target_count": len(target_counts),
            "sequence_count": len(sequences),
            "basic_sequence_count": sum(1 for seq in sequences if seq.get("is_basic")),
            "expanded_sequence_count": sum(1 for seq in sequences if not seq.get("is_basic")),
            "avg_sequence_length": rounded_mean(seq.get("length", 0) for seq in sequences),
            "max_sequence_length": max((seq.get("length", 0) for seq in sequences), default=0),
            "mutator_count_sequence_counts": dict(sorted(Counter(mutator_counts).items())),
            "mutator_count_target_counts": mutator_target_counts(sequences, mutator_counts),
            "target_sequence_counts": stringify_counter(target_counts),
            "top_target_share": rounded_share(max(target_counts.values(), default=0), len(sequences)),
            "target_evenness": round(evenness(target_counts), 3),
            "unique_sequence_shape_count": len(set(shape_chains)),
            "sequence_shape_unique_ratio": rounded_share(len(set(shape_chains)), len(sequences)),
            "api_call_total": total_api_calls,
            "unique_api_call_count": len(api_call_counter),
            "api_global_repeat_ratio": round(
                1.0 - (len(api_call_counter) / total_api_calls), 3
            )
            if total_api_calls
            else 0,
            "sequence_internal_api_repeat_ratio": rounded_share(
                sum(1 for chain in chains if len(set(chain)) < len(chain)), len(chains)
            ),
            "target_reappears_before_final_ratio": rounded_share(
                sum(
                    1
                    for seq in sequences
                    if seq.get("target") in seq.get("functions", [])[:-1]
                ),
                len(sequences),
            ),
            "avg_ranking_value": rounded_mean(seq.get("ranking_value") for seq in sequences),
            "avg_total_distance": rounded_mean(seq.get("total_distance") for seq in sequences),
            "avg_reuse_penalty": rounded_mean(seq.get("reuse_penalty") for seq in sequences),
        }
    )
    metrics.update(summary_sequence_limit_metrics(summaries))
    return metrics


def empty_metrics():
    return {
        "meta_present": False,
        "api_count": 0,
        "supported_api_count": 0,
        "mono_api_count": 0,
        "api_safety_counts": {},
        "unsafe_block_api_count": 0,
        "indirect_unsafe_api_count": 0,
        "candidate_unsafe_wrapper_count": 0,
        "covered_unsafe_wrapper_count": 0,
        "target_summary_count": 0,
        "covered_target_count": 0,
        "sequence_count": 0,
        "basic_sequence_count": 0,
        "expanded_sequence_count": 0,
        "avg_sequence_length": None,
        "max_sequence_length": 0,
        "mutator_count_sequence_counts": {},
        "mutator_count_target_counts": {},
        "target_sequence_counts": {},
        "top_target_share": 0,
        "target_evenness": 0,
        "unique_sequence_shape_count": 0,
        "sequence_shape_unique_ratio": 0,
        "api_call_total": 0,
        "unique_api_call_count": 0,
        "api_global_repeat_ratio": 0,
        "sequence_internal_api_repeat_ratio": 0,
        "target_reappears_before_final_ratio": 0,
        "avg_ranking_value": None,
        "avg_total_distance": None,
        "avg_reuse_penalty": None,
        "target_sequence_cap_total": 0,
        "target_sequences_accepted": 0,
        "basic_sequence_cap_total": 0,
        "basic_sequences_accepted": 0,
        "sequence_cap_rejected_candidates": 0,
        "mutator_round_raw_candidates": 0,
        "mutator_round_selected_candidates": 0,
        "mutator_round_duplicate_candidates": 0,
        "mutator_round_depth_pruned_candidates": 0,
        "generated_rust_file_count": 0,
        "generated_rust_loc": 0,
        "generated_test_module_count": 0,
        "generated_fuzz_target_count": 0,
    }


def count_backend_outputs(harness, backend):
    rust_files = list(harness.glob("**/*.rs"))
    return {
        "generated_rust_file_count": len(rust_files),
        "generated_rust_loc": sum(count_lines(path) for path in rust_files),
        "generated_test_module_count": len(list((harness / "src").glob("test_*.rs"))),
        "generated_fuzz_target_count": len(list((harness / "fuzz_targets").glob("*.rs")))
        if backend == "fuzz"
        else 0,
    }


def count_lines(path):
    try:
        return sum(1 for _ in path.open("r", encoding="utf-8", errors="replace"))
    except OSError:
        return 0


def unique_api_wrappers(apis):
    wrappers = set()
    for api in apis:
        wrappers.update(wrapper for wrapper in api.get("unsafe_wrappers", []) if wrapper)
    return wrappers


def unique_sequence_wrappers(sequences):
    wrappers = set()
    for seq in sequences:
        if seq.get("unsafe_wrapper"):
            wrappers.add(seq["unsafe_wrapper"])
        wrappers.update(wrapper for wrapper in seq.get("unsafe_wrappers", []) if wrapper)
    return wrappers


def sequence_mutator_counts(sequences):
    counts = []
    for index, seq in enumerate(sequences):
        if seq.get("is_basic"):
            counts.append(0)
            continue
        depth = 0
        seen = set()
        current = index
        while True:
            if current in seen or current < 0 or current >= len(sequences):
                break
            seen.add(current)
            parent = sequences[current].get("predecessor")
            if not isinstance(parent, int) or sequences[current].get("is_basic"):
                break
            depth += 1
            current = parent
        counts.append(depth)
    return counts


def mutator_target_counts(sequences, mutator_counts):
    by_mutator_count = defaultdict(set)
    for seq, mutator_count in zip(sequences, mutator_counts):
        by_mutator_count[mutator_count].add(seq.get("target"))
    return {
        str(mutator_count): len(targets)
        for mutator_count, targets in sorted(by_mutator_count.items())
    }


def summary_sequence_limit_metrics(summaries):
    round_totals = Counter()
    for summary in summaries:
        for round_summary in summary.get("mutator_rounds", []):
            round_totals["raw"] += round_summary.get("raw_candidates", 0)
            round_totals["selected"] += round_summary.get("selected_candidates", 0)
            round_totals["duplicates"] += round_summary.get("duplicate_candidates", 0)
            round_totals["depth_pruned"] += round_summary.get("depth_pruned_candidates", 0)
    return {
        "target_sequence_cap_total": sum(s.get("target_sequence_cap", 0) for s in summaries),
        "target_sequences_accepted": sum(
            s.get("target_sequences_accepted", 0) for s in summaries
        ),
        "basic_sequence_cap_total": sum(s.get("basic_sequence_cap", 0) for s in summaries),
        "sequence_cap_rejected_candidates": sum(
            s.get("rejected_candidates_target_sequence_cap", 0)
            + s.get("rejected_candidates_wrapper_sequence_cap", 0)
            for s in summaries
        ),
        "mutator_round_raw_candidates": round_totals["raw"],
        "mutator_round_selected_candidates": round_totals["selected"],
        "mutator_round_duplicate_candidates": round_totals["duplicates"],
        "mutator_round_depth_pruned_candidates": round_totals["depth_pruned"],
    }


def shape_path(path):
    path = re.sub(r"::\{.*?\}", "", path)
    path = re.sub(r"<[^<>]*>", "<_>", path)
    path = re.sub(r"::h[0-9a-f]{8,}", "", path)
    return path


def rounded_mean(values):
    values = [value for value in values if isinstance(value, (int, float))]
    if not values:
        return None
    return round(sum(values) / len(values), 3)


def rounded_share(numerator, denominator):
    if not denominator:
        return 0
    return round(numerator / denominator, 3)


def stringify_counter(counter):
    return {str(key): value for key, value in sorted(counter.items(), key=lambda item: str(item[0]))}


def evenness(counter):
    if not counter:
        return 0.0
    total = sum(counter.values())
    if len(counter) == 1:
        return 1.0
    entropy = 0.0
    for value in counter.values():
        p = value / total
        entropy -= p * math.log(p)
    return entropy / math.log(len(counter))


def summarize(rows, out_dir):
    rows = [normalize_result(row) for row in rows]
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "all_results.json").write_text(
        json.dumps(rows, indent=2, sort_keys=True) + "\n"
    )
    write_csv(out_dir / "all_results.csv", rows)
    grouped = summarize_groups(rows)
    write_csv(out_dir / "summary_by_config_backend.csv", grouped)
    write_markdown(out_dir / "summary.md", grouped)


def summarize_slices(rows, out_dir):
    rows = [normalize_result(row) for row in rows]
    groups = defaultdict(list)
    for row in rows:
        groups[(row.get("experiment"), row.get("config"), row.get("backend"))].append(row)
    for (experiment, config, backend), group in groups.items():
        if experiment and config and backend:
            summarize(group, out_dir / experiment / config / backend)


def summarize_groups(rows):
    rows = [normalize_result(row) for row in rows]
    groups = defaultdict(list)
    for row in rows:
        groups[(row.get("experiment"), row.get("config"), row.get("backend"))].append(row)

    summaries = []
    for (experiment, config, backend), group in sorted(groups.items()):
        meta = [row for row in group if row.get("meta_present")]
        summaries.append(
            {
                "experiment": experiment,
                "config": config,
                "backend": backend,
                "runs": len(group),
                "ok": sum(1 for row in group if row.get("status") == "ok"),
                "fail": sum(1 for row in group if row.get("status") == "fail"),
                "outer_timeout": sum(1 for row in group if row.get("status") == "outer-timeout"),
                "generated": sum(1 for row in group if row.get("generation_status") == "generated"),
                "generated_empty": sum(
                    1 for row in group if row.get("generation_status") == "generated-empty"
                ),
                "no_metadata": sum(
                    1 for row in group if row.get("generation_status") == "no-metadata"
                ),
                "meta_runs": len(meta),
                "median_elapsed_sec": median(row.get("elapsed_sec") for row in group),
                "total_sequences": sum(row.get("sequence_count", 0) for row in meta),
                "median_sequences": median(row.get("sequence_count") for row in meta),
                "total_covered_targets": sum(row.get("covered_target_count", 0) for row in meta),
                "total_target_summaries": sum(row.get("target_summary_count", 0) for row in meta),
                "total_covered_unsafe_wrappers": sum(
                    row.get("covered_unsafe_wrapper_count", 0) for row in meta
                ),
                "median_api_global_repeat_ratio": median(
                    row.get("api_global_repeat_ratio") for row in meta
                ),
                "median_sequence_shape_unique_ratio": median(
                    row.get("sequence_shape_unique_ratio") for row in meta
                ),
                "median_internal_api_repeat_ratio": median(
                    row.get("sequence_internal_api_repeat_ratio") for row in meta
                ),
                "mutator_count_sequence_counts": merge_counter_field(
                    meta, "mutator_count_sequence_counts"
                ),
                "generated_rust_files": sum(row.get("generated_rust_file_count", 0) for row in meta),
                "generated_rust_loc": sum(row.get("generated_rust_loc", 0) for row in meta),
                "generated_test_modules": sum(
                    row.get("generated_test_module_count", 0) for row in meta
                ),
                "generated_fuzz_targets": sum(
                    row.get("generated_fuzz_target_count", 0) for row in meta
                ),
                "sequence_cap_rejected_candidates": sum(
                    row.get("sequence_cap_rejected_candidates", 0) for row in meta
                ),
            }
        )
    return summaries


def write_csv(path, rows):
    if not rows:
        path.write_text("")
        return
    fields = sorted({field for row in rows for field in row.keys()})
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({field: csv_value(row.get(field)) for field in fields})


def csv_value(value):
    if isinstance(value, (dict, list)):
        return json.dumps(value, sort_keys=True)
    return value


def write_markdown(path, rows):
    lines = ["# Paper Experiment Generation Summary", ""]
    lines.append(
        "| experiment | config | backend | ok/fail/timeout | generated/empty/no-meta | meta | median sec | total seq | "
        "covered targets | covered wrappers | backend outputs | API repeat | shape unique | mutators |"
    )
    lines.append("|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|")
    for row in rows:
        status = f"{row['ok']}/{row['fail']}/{row['outer_timeout']}"
        generation = f"{row['generated']}/{row['generated_empty']}/{row['no_metadata']}"
        backend_outputs = (
            f"rs={row['generated_rust_files']}, "
            f"tests={row['generated_test_modules']}, "
            f"fuzz={row['generated_fuzz_targets']}"
        )
        lines.append(
            f"| {row['experiment']} | {row['config']} | {row['backend']} | {status} | "
            f"{generation} | {row['meta_runs']} | {row['median_elapsed_sec']} | {row['total_sequences']} | "
            f"{row['total_covered_targets']} | {row['total_covered_unsafe_wrappers']} | "
            f"{backend_outputs} | "
            f"{row['median_api_global_repeat_ratio']} | {row['median_sequence_shape_unique_ratio']} | "
            f"`{row['mutator_count_sequence_counts']}` |"
        )
    path.write_text("\n".join(lines) + "\n")


def median(values):
    values = sorted(value for value in values if isinstance(value, (int, float)))
    if not values:
        return None
    mid = len(values) // 2
    if len(values) % 2:
        return values[mid]
    return round((values[mid - 1] + values[mid]) / 2, 3)


def merge_counter_field(rows, field):
    merged = Counter()
    for row in rows:
        merged.update({str(k): v for k, v in row.get(field, {}).items()})
    return dict(sorted(merged.items(), key=lambda item: int(item[0]) if item[0].isdigit() else item[0]))


def select_crates(all_crates, args):
    selected = all_crates
    if args.crate:
        wanted = set(args.crate)
        selected = [
            crate
            for crate in selected
            if crate["crate"] in wanted or crate_key(crate) in wanted
        ]
    if args.note:
        wanted_notes = set(args.note)
        selected = [crate for crate in selected if crate["note"] in wanted_notes]
    if args.limit is not None:
        selected = selected[: args.limit]
    return selected


def select_configs(experiment, requested_configs):
    configs = EXPERIMENTS[experiment]
    if not requested_configs:
        return configs
    requested = set(requested_configs)
    selected = [config for config in configs if config["name"] in requested]
    missing = requested - {config["name"] for config in selected}
    if missing:
        raise ValueError(
            f"{experiment} does not define config(s): {', '.join(sorted(missing))}"
        )
    return selected


def selected_backends(args):
    requested = args.backend or ["tests"]
    backends = []
    for backend in requested:
        if backend == "all":
            candidates = BACKENDS
        else:
            candidates = (backend,)
        for candidate in candidates:
            if candidate not in backends:
                backends.append(candidate)
    return backends


def gather_existing_results(out_dir):
    rows = []
    for path in sorted(out_dir.glob("*/*/*/*/result.json")):
        try:
            rows.append(json.loads(path.read_text()))
        except FileNotFoundError:
            continue
    return rows


def normalize_result(row):
    if "generation_status" not in row:
        row = dict(row)
        row["generation_status"] = generation_status(row)
    if "first_error" not in row:
        row = dict(row)
        run_dir = row.get("harness")
        stderr_path = Path(run_dir).parent / "stderr.log" if run_dir else None
        row["first_error"] = first_error(stderr_path) if stderr_path else None
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, default=DEFAULT_CSV)
    parser.add_argument("--sources-dir", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument(
        "--experiment",
        choices=sorted(EXPERIMENTS),
        action="append",
        default=[],
        help="Experiment suite to run. Defaults to obligation-sequence.",
    )
    parser.add_argument(
        "--config",
        choices=config_names(),
        action="append",
        default=[],
        help="Run only the named experiment configuration. Repeat to select several.",
    )
    parser.add_argument(
        "--backend",
        choices=BACKEND_CHOICES,
        action="append",
        default=[],
        help="Backend to render. Repeat, pass --backend all, or omit for tests.",
    )
    parser.add_argument(
        "--disable-sequence-limits",
        action="store_true",
        help="Force sequence-unlimited obligation-driven generation for selected configs.",
    )
    parser.add_argument("--max-mutators", type=int)
    parser.add_argument("--max-depth", type=int)
    parser.add_argument("--max-sequences", type=int, default=0)
    parser.add_argument("--max-sequences-per-wrapper", type=int)
    parser.add_argument("--max-sequences-per-target", type=int)
    parser.add_argument("--max-basic-sequences-per-target", type=int)
    parser.add_argument("--crate", action="append", default=[], help="Crate name or crate-version key.")
    parser.add_argument("--note", action="append", default=[], help="Filter selected_crates.csv by note.")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--timeout", type=int, default=7200)
    parser.add_argument("--outer-timeout-grace", type=int, default=300)
    parser.add_argument("--jobs", type=int, default=1, help="Number of crates to process in parallel.")
    parser.add_argument("--log-level", default="info", choices=("error", "warn", "info", "debug", "trace"))
    parser.add_argument("--env", action="append", default=[], help="Extra environment override, KEY=VALUE.")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--skip-fetch", action="store_true")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--check-harness", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--aggregate-existing", action="store_true")
    args = parser.parse_args()

    args.out = args.out.resolve()
    args.sources_dir = args.sources_dir.resolve()

    if args.aggregate_existing:
        rows = gather_existing_results(args.out)
        summarize(rows, args.out)
        summarize_slices(rows, args.out)
        return 0

    if not args.dry_run:
        try:
            build_tool(args.skip_build)
        except Exception as error:
            print(error, file=sys.stderr)
            return 2

    selected_crates = select_crates(load_selected_crates(args.csv), args)
    if not selected_crates:
        print("no crates selected", file=sys.stderr)
        return 2

    experiments = args.experiment or ["obligation-sequence"]
    try:
        experiment_configs = {
            experiment: select_configs(experiment, args.config) for experiment in experiments
        }
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2

    total = sum(len(configs) for configs in experiment_configs.values()) * len(selected_crates)
    args.backends = selected_backends(args)
    rows = []
    jobs = max(1, args.jobs)
    if jobs == 1:
        completed = 0
        for crate in selected_crates:
            crate_rows, completed = run_crate_generations(
                crate,
                experiments,
                experiment_configs,
                args,
                completed,
                total,
            )
            rows.extend(crate_rows)
            if rows and not args.dry_run:
                summarize(rows, args.out)
                summarize_slices(rows, args.out)
    else:
        with ThreadPoolExecutor(max_workers=jobs) as executor:
            futures = {
                executor.submit(
                    run_crate_generations,
                    crate,
                    experiments,
                    experiment_configs,
                    args,
                    (index - 1) * sum(len(configs) for configs in experiment_configs.values()),
                    total,
                ): crate
                for index, crate in enumerate(selected_crates, start=1)
            }
            for future in as_completed(futures):
                crate = futures[future]
                try:
                    crate_rows, _ = future.result()
                except Exception as error:
                    print(f"[crate-error] {crate_key(crate)}: {error}", file=sys.stderr, flush=True)
                    continue
                rows.extend(crate_rows)
                if rows and not args.dry_run:
                    existing = gather_existing_results(args.out)
                    summarize(existing, args.out)
                    summarize_slices(existing, args.out)

    if rows and not args.dry_run:
        final_rows = gather_existing_results(args.out) if jobs > 1 else rows
        summarize(final_rows, args.out)
        summarize_slices(final_rows, args.out)
    return 0


def run_crate_generations(crate, experiments, experiment_configs, args, completed, total):
    rows = []
    try:
        if args.dry_run:
            source_dir = source_dir_for(args.sources_dir, crate)
            source_preprocessing = []
        else:
            source_dir = ensure_source(args.sources_dir, crate, fetch=not args.skip_fetch)
            source_preprocessing = apply_source_preprocessing(source_dir, crate)
    except Exception as error:
        print(f"[source-error] {crate_key(crate)}: {error}", file=sys.stderr, flush=True)
        return rows, completed

    for experiment in experiments:
        for config in experiment_configs[experiment]:
            completed += 1
            print(
                f"[{completed}/{total}] {crate_key(crate)} {experiment}/{config['name']} "
                f"backend={','.join(args.backends)}",
                flush=True,
            )
            try:
                rows.extend(
                    run_generation(
                        crate,
                        source_dir,
                        source_preprocessing,
                        experiment,
                        config,
                        args.backends,
                        args,
                    )
                )
            except Exception as error:
                print(
                    f"[run-error] {crate_key(crate)} {config['name']} {','.join(args.backends)}: {error}",
                    file=sys.stderr,
                    flush=True,
                )
    return rows, completed


if __name__ == "__main__":
    raise SystemExit(main())
