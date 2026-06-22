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

- `obligation_sequence.json`: main sequence-limited synthesis experiment, backend execution profiles, and final analysis defaults.
- `ablation_budget.json`: budget/penalty ablation synthesis experiment.

The JSON files intentionally include both synthesis parameters and run parameters such as `timeout`, `jobs`, output paths, and backend execution limits. CLI flags still override JSON defaults for one-off reruns.

## Main Trophy Workflow

Synthesize the main suites for `misc/trophy_crates.csv`:

```bash
python3 scripts/suite_synthesize.py \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0
```

Run the budget/penalty ablations:

```bash
for config in \
  od_sequence_full \
  od_no_reuse_penalty \
  od_no_depth_penalty \
  od_no_reuse_no_depth_penalty \
  od_sequence_unlimited
do
  python3 scripts/suite_synthesize.py \
    --experiment ablation \
    --config "$config"
done
```

Refresh synthesis summaries without rerunning synthesis:

```bash
python3 scripts/suite_synthesize.py --aggregate-existing
```

Execute generated backend harnesses using profiles from `obligation_sequence.json`:

```bash
python3 scripts/suite_execute.py --run kani
python3 scripts/suite_execute.py --run tests-asan
python3 scripts/suite_execute.py --run tests-msan
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
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --backend kani \
  --jobs 16 \
  --timeout 1800

python3 scripts/suite_synthesize.py \
  --out tmp/popular-unsafe-obligation-sequence/generation \
  --aggregate-existing

python3 scripts/suite_execute.py \
  --generation-results tmp/popular-unsafe-obligation-sequence/generation/all_results.csv \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --tests-runner sanitizer \
  --sanitizer memory \
  --jobs 4 \
  --timeout 1800 \
  --test-timeout 300 \
  --test-jobs 1 \
  --max-tests-per-crate 5

python3 scripts/suite_execute.py \
  --generation-results tmp/popular-unsafe-obligation-sequence/generation/all_results.csv \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --tests-runner miri \
  --jobs 4 \
  --timeout 1800 \
  --test-timeout 300 \
  --test-jobs 1 \
  --max-tests-per-crate 5

python3 scripts/suite_execute.py \
  --out tmp/popular-unsafe-obligation-sequence/backend-execution \
  --aggregate-existing
```

Use `scripts/run_in_experiment_container.sh <command...>` around any command that must run in the experiment container.
