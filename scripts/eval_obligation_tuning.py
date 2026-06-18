#!/usr/bin/env python3
import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]
SOURCES = REPO / "tmp/selected-crates-eval/sources"
OUT = REPO / "tmp/selected-crates-eval/tuning-runs"
BIN = REPO / "target/debug/cargo-4safer"


CONFIGS = {
    "final": [
        {"name": "final_default", "max_mutators": 4, "max_depth": 0},
    ],
    "cli": [
        {"name": "base_m2_d0", "max_mutators": 2, "max_depth": 0},
        {"name": "m0_d0", "max_mutators": 0, "max_depth": 0},
        {"name": "m1_d0", "max_mutators": 1, "max_depth": 0},
        {"name": "m3_d0", "max_mutators": 3, "max_depth": 0},
        {"name": "m4_d0", "max_mutators": 4, "max_depth": 0},
        {"name": "m2_d4", "max_mutators": 2, "max_depth": 4},
        {"name": "m2_d6", "max_mutators": 2, "max_depth": 6},
        {"name": "m2_d8", "max_mutators": 2, "max_depth": 8},
        {"name": "m2_d0_no_reuse", "max_mutators": 2, "max_depth": 0, "disable_reuse": True},
        {"name": "m2_d0_no_depth_penalty", "max_mutators": 2, "max_depth": 0, "disable_depth": True},
        {
            "name": "m2_d0_no_penalties",
            "max_mutators": 2,
            "max_depth": 0,
            "disable_reuse": True,
            "disable_depth": True,
        },
    ],
    "sequence_limits": [
        {"name": "sequence_low", "max_mutators": 2, "max_depth": 0, "max_sequences_per_target": 16, "max_basic_sequences_per_target": 8},
        {"name": "sequence_high", "max_mutators": 2, "max_depth": 0, "max_sequences_per_target": 128, "max_basic_sequences_per_target": 32},
        {"name": "candidates_low", "max_mutators": 2, "max_depth": 0, "env": {"FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "4", "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "4", "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "8"}},
        {"name": "candidates_high", "max_mutators": 2, "max_depth": 0, "env": {"FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16", "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16", "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32"}},
        {"name": "reuse_weight_high", "max_mutators": 2, "max_depth": 0, "env": {"FOURSAFER_OD_REUSE_PENALTY_WEIGHT": "2.0"}},
        {"name": "distance_weight_high", "max_mutators": 2, "max_depth": 0, "env": {"FOURSAFER_OD_PREFIX_DISTANCE_WEIGHT": "1.5", "FOURSAFER_OD_TARGET_DISTANCE_WEIGHT": "1.5"}},
        {"name": "depth_pressure_high", "max_mutators": 2, "max_depth": 0, "env": {"FOURSAFER_OD_DEPTH_SATURATION_WEIGHT": "0.5", "FOURSAFER_OD_STAGNATION_DEPTH_WEIGHT": "1.0"}},
    ],
    "combo": [
        {
            "name": "sequence_high_candidates_high",
            "max_mutators": 2,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
            },
        },
        {
            "name": "sequence_high_reuse_weight_high",
            "max_mutators": 2,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_REUSE_PENALTY_WEIGHT": "2.0",
            },
        },
        {
            "name": "sequence_high_candidates_high_reuse_weight_high",
            "max_mutators": 2,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_REUSE_PENALTY_WEIGHT": "2.0",
            },
        },
        {
            "name": "sequence_high_candidates_high_m3",
            "max_mutators": 3,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_r3_8",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_MUTATION_ROUND3_LIMIT": "8",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_r3_4",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_MUTATION_ROUND3_LIMIT": "4",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_depth_cost_05",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_DEPTH_PENALTY_WEIGHT": "0.5",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_depth_cost_025",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_DEPTH_PENALTY_WEIGHT": "0.25",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_depth_cost_1",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_DEPTH_PENALTY_WEIGHT": "1.0",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_r3_4_depth_cost_05",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_MUTATION_ROUND3_LIMIT": "4",
                "FOURSAFER_OD_DEPTH_PENALTY_WEIGHT": "0.5",
            },
        },
        {
            "name": "sequence_high_candidates_high_m4_r3_4_depth_cost_025",
            "max_mutators": 4,
            "max_depth": 0,
            "env": {
                "FOURSAFER_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM": "16",
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES": "16",
                "FOURSAFER_OD_MAX_TARGET_PLAN_CANDIDATES": "32",
                "FOURSAFER_OD_MUTATION_ROUND3_LIMIT": "4",
                "FOURSAFER_OD_DEPTH_PENALTY_WEIGHT": "0.25",
            },
        },
    ],
}


def crate_dirs():
    return sorted(path for path in SOURCES.iterdir() if (path / "Cargo.toml").exists())


def crate_key(crate_dir):
    name = crate_dir.name
    return re.sub(r"-\d.*$", "", name)


def run_one(crate_dir, config, timeout, force):
    run_dir = OUT / crate_key(crate_dir) / config["name"]
    result_path = run_dir / "result.json"
    if result_path.exists() and not force:
        return json.loads(result_path.read_text())

    if force and run_dir.exists():
        shutil.rmtree(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)

    harness = run_dir / "harness"
    target_dir = run_dir / "target"
    stdout_path = run_dir / "stdout.log"
    stderr_path = run_dir / "stderr.log"

    cmd = [
        str(BIN),
        "4safer",
        "-C",
        str(crate_dir),
        "-o",
        str(harness),
        "--max-mutators",
        str(config["max_mutators"]),
        "--max-depth",
        str(config["max_depth"]),
        "--timeout",
        str(timeout),
        "--log-level",
        "info",
    ]
    if config.get("disable_reuse"):
        cmd.append("--disable-reuse-penalty")
    if config.get("disable_depth"):
        cmd.append("--disable-depth-penalty")
    if config.get("max_sequences_per_wrapper") is not None:
        cmd.extend(["--max-sequences-per-wrapper", str(config["max_sequences_per_wrapper"])])
    if config.get("max_sequences_per_target") is not None:
        cmd.extend(["--max-sequences-per-target", str(config["max_sequences_per_target"])])
    if config.get("max_basic_sequences_per_target") is not None:
        cmd.extend([
            "--max-basic-sequences-per-target",
            str(config["max_basic_sequences_per_target"]),
        ])

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env.update(config.get("env", {}))

    started = time.monotonic()
    status = "ok"
    exit_code = 0
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            completed = subprocess.run(
                cmd,
                cwd=REPO,
                env=env,
                stdout=stdout,
                stderr=stderr,
                timeout=timeout + 300,
            )
        exit_code = completed.returncode
        if exit_code != 0:
            status = "fail"
    except subprocess.TimeoutExpired:
        status = "outer-timeout"
        exit_code = -999

    elapsed = round(time.monotonic() - started, 3)
    result = {
        "crate": crate_key(crate_dir),
        "source": str(crate_dir),
        "config": config["name"],
        "max_mutators": config["max_mutators"],
        "max_depth": config["max_depth"],
        "disable_reuse": bool(config.get("disable_reuse")),
        "disable_depth": bool(config.get("disable_depth")),
        "env": config.get("env", {}),
        "status": status,
        "exit_code": exit_code,
        "elapsed_sec": elapsed,
    }
    result.update(read_metrics(harness))
    result["stderr_tail"] = tail(stderr_path)
    result_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def tail(path, limit=4000):
    if not path.exists():
        return ""
    data = path.read_bytes()[-limit:]
    return data.decode("utf-8", "replace")


def load_json(path):
    return json.loads(path.read_text())


def find_meta(harness):
    meta_dir = harness / "foursafer_meta"
    if not meta_dir.exists():
        return None, None, None
    seqs = sorted(meta_dir.glob("*_api_sequences.json"))
    apis = sorted(meta_dir.glob("*_api_functions.json"))
    summaries = sorted(meta_dir.glob("*_target_summaries.json"))
    if not seqs:
        return None, None, None
    return seqs[0], apis[0] if apis else None, summaries[0] if summaries else None


def read_metrics(harness):
    seq_path, api_path, summary_path = find_meta(harness)
    empty = {
        "meta_present": False,
        "sequence_count": 0,
        "basic_sequence_count": 0,
        "expanded_sequence_count": 0,
        "unique_targets": 0,
        "avg_length": None,
        "max_length": None,
        "depth_counts": {},
        "shape_unique_ratio": None,
        "self_repeat_ratio": None,
        "target_repeat_ratio": None,
        "top_target_share": None,
        "evenness": None,
        "target_sequences_accepted": None,
        "target_sequence_cap_total": None,
        "sequence_cap_rejects": None,
    }
    if seq_path is None:
        return empty

    sequences = load_json(seq_path)
    apis = load_json(api_path) if api_path else []
    summaries = load_json(summary_path) if summary_path else []
    api_paths = {api.get("index"): api.get("path", str(api.get("index"))) for api in apis}
    depths = sequence_depths(sequences)
    target_counts = Counter(seq["target"] for seq in sequences)
    chains = [[api_paths.get(index, str(index)) for index in seq.get("functions", [])] for seq in sequences]
    shape_chains = [tuple(shape_path(part) for part in chain) + (("basic" if seq.get("is_basic") else "expanded"),) for chain, seq in zip(chains, sequences)]

    total = len(sequences)
    target_total = sum(target_counts.values())
    top_target_share = max(target_counts.values()) / target_total if target_total else 0
    return {
        "meta_present": True,
        "sequence_count": total,
        "basic_sequence_count": sum(1 for seq in sequences if seq.get("is_basic")),
        "expanded_sequence_count": sum(1 for seq in sequences if not seq.get("is_basic")),
        "unique_targets": len(target_counts),
        "avg_length": round(sum(seq.get("length", 0) for seq in sequences) / total, 3) if total else 0,
        "max_length": max((seq.get("length", 0) for seq in sequences), default=0),
        "depth_counts": dict(sorted(Counter(depths).items())),
        "shape_unique_ratio": round(len(set(shape_chains)) / total, 3) if total else 1,
        "self_repeat_ratio": round(sum(1 for chain in chains if len(set(chain)) < len(chain)) / total, 3) if total else 0,
        "target_repeat_ratio": round(sum(1 for seq in sequences if seq.get("target") in seq.get("functions", [])[:-1]) / total, 3) if total else 0,
        "top_target_share": round(top_target_share, 3),
        "evenness": round(evenness(target_counts), 3),
        "target_sequences_accepted": round(sum(s.get("target_sequences_accepted", 0) for s in summaries), 3),
        "target_sequence_cap_total": sum(s.get("target_sequence_cap", 0) for s in summaries),
        "sequence_cap_rejects": sum(
            s.get("rejected_candidates_target_sequence_cap", 0)
            + s.get("rejected_candidates_wrapper_sequence_cap", 0)
            for s in summaries
        ),
    }


def sequence_depths(sequences):
    depths = []
    for index, seq in enumerate(sequences):
        if seq.get("is_basic"):
            depths.append(0)
            continue
        parent = seq.get("predecessor")
        if isinstance(parent, int) and 0 <= parent < index:
            depths.append(depths[parent] + 1)
        else:
            depths.append(1)
    return depths


def shape_path(path):
    path = re.sub(r"::\{.*?\}", "", path)
    path = re.sub(r"<[^<>]*>", "<_>", path)
    path = re.sub(r"::h[0-9a-f]{8,}", "", path)
    return path


def evenness(counter):
    if not counter:
        return 0
    total = sum(counter.values())
    if len(counter) == 1:
        return 1
    entropy = 0.0
    for value in counter.values():
        p = value / total
        entropy -= p * math.log(p)
    return entropy / math.log(len(counter))


def summarize(rows):
    OUT.mkdir(parents=True, exist_ok=True)
    summary_path = OUT / "summary.json"
    summary_path.write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n")

    by_config = []
    for config in sorted(set(row["config"] for row in rows)):
        group = [row for row in rows if row["config"] == config]
        ok_meta = [row for row in group if row.get("meta_present")]
        by_config.append({
            "config": config,
            "runs": len(group),
            "ok": sum(1 for row in group if row["status"] == "ok"),
            "fail": sum(1 for row in group if row["status"] == "fail"),
            "outer_timeout": sum(1 for row in group if row["status"] == "outer-timeout"),
            "meta_runs": len(ok_meta),
            "median_elapsed_sec": median([row["elapsed_sec"] for row in group]),
            "total_sequences": sum(row.get("sequence_count", 0) for row in ok_meta),
            "median_sequences": median([row.get("sequence_count", 0) for row in ok_meta]),
            "median_shape_unique_ratio": median([row.get("shape_unique_ratio") for row in ok_meta]),
            "median_self_repeat_ratio": median([row.get("self_repeat_ratio") for row in ok_meta]),
            "median_target_repeat_ratio": median([row.get("target_repeat_ratio") for row in ok_meta]),
            "depth_counts": merge_depth_counts(ok_meta),
            "score": score_config(group),
        })
    (OUT / "config_summary.json").write_text(json.dumps(by_config, indent=2, sort_keys=True) + "\n")
    write_markdown(by_config)


def median(values):
    values = sorted(value for value in values if value is not None)
    if not values:
        return None
    mid = len(values) // 2
    if len(values) % 2:
        return values[mid]
    return round((values[mid - 1] + values[mid]) / 2, 3)


def merge_depth_counts(rows):
    merged = Counter()
    for row in rows:
        merged.update({int(k): v for k, v in row.get("depth_counts", {}).items()})
    return dict(sorted(merged.items()))


def score_config(group):
    meta = [row for row in group if row.get("meta_present")]
    if not meta:
        return -1000
    timeout_penalty = 20 * sum(1 for row in group if row["status"] == "outer-timeout")
    fail_penalty = 5 * sum(1 for row in group if row["status"] == "fail")
    seq = math.log1p(sum(row.get("sequence_count", 0) for row in meta))
    targets = math.log1p(sum(row.get("unique_targets", 0) for row in meta))
    shape = sum(row.get("shape_unique_ratio") or 0 for row in meta) / len(meta)
    repeats = sum((row.get("self_repeat_ratio") or 0) + (row.get("target_repeat_ratio") or 0) for row in meta) / len(meta)
    depth_counts = merge_depth_counts(meta)
    deep = sum(count for depth, count in depth_counts.items() if int(depth) >= 2)
    depth_bonus = math.log1p(deep)
    return round(seq + targets + 4 * shape + depth_bonus - 2 * repeats - timeout_penalty - fail_penalty, 3)


def write_markdown(rows):
    lines = ["# Obligation Tuning Summary", ""]
    lines.append("| config | runs | ok/fail/timeout | meta | median sec | total seq | median seq | shape unique | repeats | depth counts | score |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|")
    for row in sorted(rows, key=lambda item: item["score"], reverse=True):
        repeats = f"{row['median_self_repeat_ratio']}/{row['median_target_repeat_ratio']}"
        status = f"{row['ok']}/{row['fail']}/{row['outer_timeout']}"
        lines.append(
            f"| {row['config']} | {row['runs']} | {status} | {row['meta_runs']} | "
            f"{row['median_elapsed_sec']} | {row['total_sequences']} | {row['median_sequences']} | "
            f"{row['median_shape_unique_ratio']} | {repeats} | `{row['depth_counts']}` | {row['score']} |"
        )
    (OUT / "summary.md").write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--suite", choices=sorted(CONFIGS), action="append", default=[])
    parser.add_argument("--crate", action="append", default=[])
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--aggregate-existing", action="store_true")
    args = parser.parse_args()

    if not BIN.exists():
        print(f"missing {BIN}; run cargo build first", file=sys.stderr)
        return 2

    if args.aggregate_existing:
        rows = [
            json.loads(path.read_text())
            for path in sorted(OUT.glob("*/*/result.json"))
        ]
        summarize(rows)
        return 0

    suites = args.suite or ["cli"]
    configs = [config for suite in suites for config in CONFIGS[suite]]
    selected_crates = crate_dirs()
    if args.crate:
        wanted = set(args.crate)
        selected_crates = [path for path in selected_crates if crate_key(path) in wanted or path.name in wanted]
    if args.limit:
        selected_crates = selected_crates[: args.limit]

    rows = []
    total = len(selected_crates) * len(configs)
    count = 0
    for crate_dir in selected_crates:
        for config in configs:
            count += 1
            print(f"[{count}/{total}] {crate_key(crate_dir)} {config['name']}", flush=True)
            rows.append(run_one(crate_dir, config, args.timeout, args.force))
            summarize(rows)

    summarize(rows)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
