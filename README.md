# cargo-4safer Reproduction Package

This package contains the `cargo 4safer` tool and the scripts used to reproduce
the paper experiments. `cargo 4safer` synthesizes verification suites for Rust
library crates by finding public safe APIs that directly or transitively reach
unsafe code, building a rustc-derived API dependency graph, and rendering
generated harnesses for Rust tests, Kani, and AFL fuzzing.

The default synthesis mode is budgeted obligation-driven planning. The
unbudgeted mode is exposed only for ablation through `--disable-budget-limit`
or `--unbudgeted`.

## Package Layout

```text
.
  Cargo.toml
  Cargo.lock
  rust-toolchain.toml
  src/
    cargo_phase.rs       cargo subcommand entry point and rustc-wrapper setup
    driver.rs            rustc callback entry point
    pipeline.rs          graph construction, unsafe analysis, and rendering flow
    rapx_graph.rs        rustc-side API dependency graph construction
    rapx_resolve.rs      reachable-type iteration and API monomorphization
    rapx_mono.rs         bounded generic argument solving
    unsafe_analysis.rs   MIR unsafe-effect propagation summaries
    synth/               obligation-driven target selection and planning
    backend/             tests, Kani, and fuzz harness renderers
  data/
    std_annotated_params.json
  misc/
    trophy_crates.csv
    popular_crates_with_unsafe.csv
  scripts/
    suite_synthesize.py  generate harness suites and synthesis metrics
    suite_execute.py     run generated tests, Kani, or fuzz harnesses
    result_analyze.py    build final experiment summary CSV files
    plot_sequence_length_distribution.py
    config/*.json        experiment profiles and output paths
  Dockerfile.experiments
```

Generated experiment artifacts are written under `tmp/paper-experiments/` by
default.

## Requirements

The core tool requires the pinned nightly compiler because it uses
`rustc_private` APIs.

```bash
rustup toolchain install nightly-2026-03-26 --component rustc-dev
cargo +nightly-2026-03-26 build --bin cargo-4safer --locked
```

For the full backend experiments, install or provide:

- Python 3.10 or newer.
- Docker, if using the supplied experiment container.
- AFL support through `cargo afl` for fuzz experiments.
- Kani, CBMC, `goto-cc`, and `goto-instrument` for Kani experiments.
- The Rust `miri` component for Miri execution.

The container helper expects a built Kani checkout at
`tmp/paper-experiments/tools/kani` by default. Override it with `KANI_HOST_DIR`
when needed.

## Using the Tool

Build or install the local checkout:

```bash
cargo +nightly-2026-03-26 install --path . --locked
```

Generate the default Rust test backend for a library crate:

```bash
cargo 4safer -C /path/to/crate -o /tmp/my-4safer-suite
```

Select a workspace member:

```bash
cargo 4safer -C /path/to/workspace -p my-lib
```

Run generated-crate validation after synthesis:

```bash
cargo 4safer -C /path/to/crate --check-harness
```

Render Kani or fuzz harnesses:

```bash
cargo 4safer -C /path/to/crate -b kani
cargo 4safer -C /path/to/crate -b fuzz
```

Forward target-crate feature flags:

```bash
cargo 4safer -C /path/to/crate --features extra
cargo 4safer -C /path/to/crate --all-features
cargo 4safer -C /path/to/crate -- --features extra
```

Important synthesis controls:

- `--max-mutators <N>`: maximum extra mutator rounds per target sequence.
- `--max-depth <CALLS>`: maximum call count per sequence; `0` means unlimited.
- `--max-sequences <COUNT>`: whole-suite sequence budget; `0` means unlimited.
- `--max-sequences-per-wrapper <COUNT>`: per unsafe-wrapper group budget.
- `--max-sequences-per-target <COUNT>`: per obligation-target budget.
- `--floor-sequences-per-target <COUNT>`: budget floor for targets with
  multiple obligated parameters.
- `--disable-ranking`: ranking ablation.
- `--disable-reuse-penalty`: mutator reuse penalty ablation.
- `--disable-depth-penalty`: depth penalty ablation.
- `--disable-budget-limit` or `--unbudgeted`: unbudgeted planning ablation.

## Generated Harness Layout

For the default `tests` backend, the output is a standalone crate:

```text
<harness-dir>/
  Cargo.toml
  Cargo.lock
  src/
    lib.rs
    test_<crate_name>0.rs
    test_<crate_name>1.rs
  foursafer_meta/
    <crate_name>_api_functions.json
    <crate_name>_api_sequences.json
    <crate_name>_target_summaries.json
```

When multiple backends are requested, each backend is rendered below a backend
subdirectory of the requested output root. The fuzz backend also emits
`fuzz_targets/` and `corpus/` directories.

Run generated test suites with:

```bash
cargo test --manifest-path /path/to/foursafer_test/Cargo.toml
```

Run generated Kani suites with:

```bash
cargo kani --manifest-path /path/to/foursafer_test/Cargo.toml
```

Build and run one generated AFL target with:

```bash
cargo afl build --manifest-path /path/to/foursafer_test/Cargo.toml --bin fuzz_target_0
cargo afl fuzz \
  -i /path/to/foursafer_test/corpus/fuzz_target_0 \
  -o /path/to/foursafer_test/afl-out/fuzz_target_0 \
  -V 300 \
  -- /path/to/foursafer_test/target/debug/fuzz_target_0
```

## Reproducing the Experiments

The reproduction workflow has three stages:

1. Synthesize suites and collect generation metrics.
2. Execute generated backend harnesses.
3. Aggregate backend results into final summary CSV files.

Experiment profiles live in `scripts/config/`. The main profiles are:

- `budgeted.json`: default budgeted obligation-driven synthesis.
- `no_ranking.json`: ranking ablation.

Each config defines the crate CSV, source checkout directory, generation output,
backend execution output, and final analysis output. Relative paths are resolved
from the repository root.

### Optional Container

Build the experiment container:

```bash
scripts/build_experiment_container.sh
```

Run any reproduction command inside the container:

```bash
scripts/run_in_experiment_container.sh bash -lc '<command>'
```

The helper mounts the repository at `/workspace/4safer` and stores container
home/cache state under `tmp/experiment-container-home`.

### Stage 1: Suite Synthesis

Run the main budgeted experiment over `misc/trophy_crates.csv`:

```bash
python3 scripts/suite_synthesize.py --experiment budgeted
```

Default synthesis output for the budgeted profile:

```text
tmp/paper-experiments/runs/budgeted/
  all_results.csv
  summary_by_config_backend.csv
  <backend>/<crate-version>/result.json
  multi-kani-tests-fuzz/<crate-version>/harness/
```

### Stage 2: Backend Execution

Execute the generated harnesses using the named runs from the experiment config:

```bash
python3 scripts/suite_execute.py --experiment-config scripts/config/budgeted.json --run tests-asan
# other options: tests-miri, fuzz-asan, kani
```

Run only one crate:

```bash
python3 scripts/suite_execute.py \
  --experiment-config scripts/config/budgeted.json \
  --run tests-asan \
  --crate bson-0.13.0
```

Refresh execution summaries from existing backend result files:

```bash
python3 scripts/suite_execute.py \
  --experiment-config scripts/config/budgeted.json \
  --aggregate-existing
```

Default execution output for the budgeted profile:

```text
tmp/paper-experiments/backend-execution/budgeted/
  summary_by_backend.csv
  unsoundness_cases.csv
  tests-asan/<crate-version>/result.json
  tests-miri/<crate-version>/result.json
  fuzz-asan/<crate-version>/result.json
  kani/<crate-version>/result.json
```

### Stage 3: Final Summaries

Build the final crate-level summaries:

```bash
python3 scripts/result_analyze.py --experiment-config scripts/config/budgeted.json
```

The default output is:

```text
tmp/paper-experiments/final_backend_summaries/budgeted/
  asan_summary.csv
  miri_summary.csv
  fuzz_summary.csv
  experiment_summary.csv
```
