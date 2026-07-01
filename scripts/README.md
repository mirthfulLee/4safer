# Experiment Scripts

This directory keeps three experiment entry points:

- `suite_synthesize.py`: download/select crates, run `cargo 4safer`, and write generation metrics.
- `suite_execute.py`: execute generated `tests`, `kani`, or `fuzz` harnesses from synthesis output.
- `result_analyze.py`: combine generation and backend execution artifacts into final summary CSVs.

Container helper scripts are kept separate:

- `build_experiment_container.sh`: build the experiment container image.
- `run_in_experiment_container.sh`: run a command inside that container.

## Config Files

Experiment parameters live under `scripts/config/`.

- `budgeted.json`: budgeted obligation-driven synthesis experiment, backend execution profiles, and final analysis defaults.
- `no_ranking.json`: ranking ablation experiment; it matches `budgeted.json` except that `synthesize.cmd_args` also includes `--disable-ranking`.

The JSON files keep shared artifact roots in top-level `paths`, synthesis defaults in `synthesize`, and backend-specific execution profiles in `execute.runs`. The scripts derive experiment-specific artifacts with the top-level `experiment` name, for example `paths.generation_out/<experiment>/all_results.csv`, `paths.execution_out/<experiment>/...`, and `paths.analysis_out/<experiment>/...`. Backend execution defaults to one crate at a time; tests, fuzz targets, and Kani proof harnesses can still use per-crate internal parallelism through `test_jobs`, `fuzz_jobs`, and `kani_jobs`. CLI flags still override JSON defaults for one-off reruns.

Use `synthesize.cmd_args` for `cargo 4safer` synthesis arguments. The top-level `experiment` names the synthesis profile used in output paths and result rows.

## Main Trophy Workflow

Synthesize the main suites for `misc/trophy_crates.csv`:

```bash
python3 scripts/suite_synthesize.py \
  --experiment budgeted
```

Run the no-ranking ablation:

```bash
python3 scripts/suite_synthesize.py \
  --experiment no_ranking
```

Refresh synthesis summaries without rerunning synthesis:

```bash
python3 scripts/suite_synthesize.py --aggregate-existing
```

Execute generated backend harnesses using profiles from `budgeted.json`:

```bash
python3 scripts/suite_execute.py --run kani
python3 scripts/suite_execute.py --run tests-asan
python3 scripts/suite_execute.py --run tests-miri
python3 scripts/suite_execute.py --run fuzz-asan
```

Refresh backend execution summaries without rerunning backends:

```bash
python3 scripts/suite_execute.py --aggregate-existing
```

Write final ASan/Miri crate-level summaries:

```bash
python3 scripts/result_analyze.py
```

## Popular Unsafe Workflow

The former popular-crate shell workflow maps to explicit commands:

```bash
python3 scripts/suite_synthesize.py \
  --csv misc/popular_crates_with_unsafe.csv \
  --sources-dir ../../downloads/popular_crates \
  --out tmp/popular-unsafe-obligation-sequence/generation \
  --skip-fetch \
  --experiment budgeted \
  --backend tests \
  --backend kani \
  --jobs 16 \
  --timeout 1800

python3 scripts/suite_synthesize.py \
  --out tmp/popular-unsafe-obligation-sequence/generation \
  --aggregate-existing

python3 scripts/suite_execute.py \
  --generation-results tmp/popular-unsafe-obligation-sequence/generation/budgeted/all_results.csv \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --experiment budgeted \
  --backend tests \
  --tests-runner sanitizer \
  --sanitizer memory \
  --test-timeout 300 \
  --test-jobs 16

python3 scripts/suite_execute.py \
  --generation-results tmp/popular-unsafe-obligation-sequence/generation/budgeted/all_results.csv \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --experiment budgeted \
  --backend tests \
  --tests-runner miri \
  --test-timeout 300 \
  --test-jobs 16

python3 scripts/suite_execute.py \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --aggregate-existing
```

Use `scripts/run_in_experiment_container.sh <command...>` around any command that must run in the experiment container.
