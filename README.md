# cargo-4safer

`cargo 4safer` synthesizes obligation-driven verification suites for Rust
library crates. The project is the standalone extraction of the
`obligation_driven` synthesis flow: target discovery is driven by MIR unsafe
effect propagation, sequence planning is fixed to the obligation-driven planner,
and generated metadata is written next to the harness crate for validation.

## Status

- The user-facing entry point is `cargo 4safer`.
- The implementation runs `cargo check --lib` with this binary as
  `RUSTC_WRAPPER`, then enters an in-process rustc callback for the primary
  library target.
- MIR unsafe-surface analysis classifies direct and propagated unsafe-effect
  public APIs before planning.
- The RAPx-derived API dependency graph is built directly through rustc and
  resolved into bounded generic mono instances.
- Suite synthesis always uses the obligation-driven planner.
- The default generated crate is `foursafer_test`.
- The default backend is `tests`; `kani` and `fuzz` can still be selected with
  `--backend`.
- Ranking ablation is controlled by `--disable-ranking`.
- Metadata is written under `foursafer_meta`.

Contract extraction for unsafe calls and pointer operations remains a
placeholder in the MIR analysis layer. The current planner should not be
presented as full precondition recovery.

## Usage

Run `cargo 4safer -h` to see the full flag list.

## Install

`cargo-4safer` uses `rustc_private`, so install it with the pinned nightly and
the matching `rustc-dev` component:

```bash
rustup toolchain install nightly-2026-03-26 --component rustc-dev
cargo +nightly-2026-03-26 install \
  --git git@github.com:mirthfulLee/4safer.git \
  --branch master \
  --locked
```

For a local checkout:

```bash
cargo +nightly-2026-03-26 install --path . --locked
```

Generate the default Rust test backend for a crate directory:

```bash
cargo 4safer -C /path/to/crate
```

Generate from a manifest path:

```bash
cargo 4safer -m /path/to/crate/Cargo.toml
```

Select a workspace member explicitly:

```bash
cargo 4safer -C /path/to/workspace -p my-lib
```

Write the generated harness crate somewhere else:

```bash
cargo 4safer -C /path/to/crate -o /tmp/my-4safer-suite
```

Build-check the generated harness after synthesis:

```bash
cargo 4safer -C /path/to/crate --check-harness
```

Run without heuristic sequence ranking for ablation:

```bash
cargo 4safer -C /path/to/crate --disable-ranking
```

Render a non-default backend:

```bash
cargo 4safer -C /path/to/crate -b kani
cargo 4safer -C /path/to/crate -b fuzz
```

Cargo feature flags are forwarded to analysis and to the generated harness
dependency:

```bash
cargo 4safer -C /path/to/crate --features extra
cargo 4safer -C /path/to/crate --all-features
```

Extra `cargo check` flags can be passed after `--`:

```bash
cargo 4safer -C /path/to/crate -- --features extra
```

Common options:

- `-C, --dir <DIR>`: crate directory.
- `-m, --manifest-path <PATH>`: crate manifest path.
- `-p, --package <SPEC>`: workspace package.
- `-o, --out <PATH>`: generated harness output directory.
- `-b, --backend <BACKEND>`: `tests`, `kani`, or `fuzz`; repeat or use comma
  separated values for multiple backends.
- `-c, --check-harness`: run a quick generated-crate validation step.
- `-d, --max-depth <CALLS>`: max call count per sequence; `0` means unlimited.
- `-M, --max-mutators <N>`: maximum extra mutator rounds per target sequence.
- `--max-sequences <COUNT>`: max generated sequences in the whole suite; `0`
  means unlimited.
- `--max-sequences-per-wrapper <COUNT>`: max generated sequences per unsafe
  wrapper group.
- `--max-sequences-per-target <COUNT>`: max generated sequences per obligation
  target.
- `--floor-sequences-per-target <COUNT>`: sequence budget for
  one target with multiple obligated parameters; default `16`.
- `--disable-ranking`: accept generated sequences in construction order while
  keeping the same sequence budgets.
- `--r-flow <VALUE>`: flow decay ratio in sequence ranking; default `0.5`.
- `--w-c <VALUE>`: construction-cost weight in sequence ranking; default `0.1`.
- `--w-p <VALUE>`: diversity-penalty weight in sequence ranking; default `0.2`.
- `--disable-reuse-penalty`: disable mutator reuse penalty for ablation.
- `--disable-depth-penalty`: disable depth saturation and stagnation penalties
  for ablation.

## Generated Layout

For the default `tests` backend, the output is a standalone harness crate:

```text
<harness-dir>/
  Cargo.toml
  Cargo.lock
  src/
    lib.rs
    test_<crate_name>0.rs
    test_<crate_name>1.rs
    ...
  foursafer_meta/
    <crate_name>_api_functions.json
    <crate_name>_api_sequences.json
    <crate_name>_target_summaries.json
```

The optional `fuzz` backend also writes AFL targets and seed corpora:

```text
  .cargo/config.toml
  fuzz_targets/
    fuzz_target_0.rs
    ...
  corpus/
    fuzz_target_0/
      seed
```

When multiple backends are requested, each backend is rendered under a
subdirectory of the requested output directory.

## Running Generated Suites

The default backend emits ordinary Rust tests:

```bash
cargo test --manifest-path /path/to/foursafer_test/Cargo.toml
```

Kani backend output can be checked with:

```bash
cargo kani --manifest-path /path/to/foursafer_test/Cargo.toml
```

The fuzz backend emits AFL binary targets:

```bash
cargo afl build --manifest-path /path/to/foursafer_test/Cargo.toml --bin fuzz_target_0
cargo afl fuzz \
  -i /path/to/foursafer_test/corpus/fuzz_target_0 \
  -o /path/to/foursafer_test/afl-out/fuzz_target_0 \
  -V 300 \
  -- /path/to/foursafer_test/target/debug/fuzz_target_0
```

## Design Notes

- Target selection includes direct unsafe-block wrappers and MIR-propagated
  indirect unsafe targets.
- Relationships are established from MIR dataflow through assignments,
  temporaries, returns, and call arguments, not plain call reachability.
- Field-sensitive summaries filter constructor and mutator choices so unrelated
  field-only APIs are not pulled into a target sequence.
- APIs that transitively invoke unsafe code without writing back into relevant
  object fields can still be targets, but they should not become broad mutators
  merely because they take `&mut self`.
- Generic resolution is rustc-direct only. Do not reintroduce rustdoc
  `clean::Type` matching.

## Toolchain

The repo is pinned to `nightly-2026-03-26` because the graph builder relies on
`rustc_private` plus `rustc-dev`.

Child `cargo metadata`, `cargo check`, lockfile generation, and
`--check-harness` validation inherit the active `RUSTUP_TOOLCHAIN` so the target
crate is analyzed with the same compiler selection that launched `cargo 4safer`.
