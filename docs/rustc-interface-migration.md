# Rustc-Direct Status

Date: 2026-03-19

## Repository Status

- `~/projects/varies`: historical reference for the original wrapper-oriented
  sequence generator and Kani harness shape.
- `cargo-4safer`: standalone obligation-driven synthesis repository. The active
  entry path is `src/cargo_phase.rs` -> `src/driver.rs` -> `src/pipeline.rs`,
  with RAPx-derived graphing in `src/rapx_graph.rs`, generic resolution in
  `src/rapx_resolve.rs` / `src/rapx_mono.rs`, obligation-driven synthesis in
  `src/synth/obligation_driven`, and backend rendering in `src/backend`.
- `~/projects/RAPx`: reference architecture for direct rustc integration.
  The callback split is in `rapx/src/lib.rs` (`Callbacks::config`, optional
  preprocessing, then `after_analysis` dispatch) and the API dependency logic
  lives under `rapx/src/analysis/core/api_dependency`.

## Current Shape

1. **Driver architecture**
   The repository uses a rustc wrapper callback for the primary library target.
2. **Graph / monomorphization**
   The current solver resolves bounded multi-generic wrapper targets, but it
   still relies on bounded candidate expansion and partial trait-bound
   reasoning.
3. **Obligation-driven planning**
   Target classification is MIR dataflow based and includes direct unsafe-block
   targets plus propagated indirect unsafe targets.
4. **Sequence planning**
   The planner still chooses bounded producer paths and mutator expansions
   instead of exhaustively enumerating all producer combinations.
5. **Rendering**
   The default backend is ordinary Rust tests. Kani and fuzz rendering remain
   available for experiments.

## Main Risks

- Nightly `rustc_private` drift between `cargo-4safer`, RAPx, and the selected
  toolchain.
- Regressing the metadata contract consumed by experiment scripts and
  `varies_kani`.
- Overfitting generic resolution to fixture-specific std-wrapper cases.
- Using heuristic trait-bound checks where rustc obligation evaluation is still
  brittle on partially inferred arguments.

## Execution Checklist

- [x] Extract analysis orchestration behind a dedicated rustc-direct pipeline
      module.
- [x] Finish the first bounded multi-generic implementation slice in
      `rapx_mono` / `rapx_resolve`.
- [x] Add smoke coverage for multi-generic free functions and trait-default
      wrapper targets.
- [x] Extract the obligation-driven strategy into the standalone `cargo-4safer`
      project.
- [x] Make `tests` the default backend and expose budget ablation through
      `--disable-budget-limit`.
- [ ] Restore more of the intended Kani rendering shape and input pipeline.
- [ ] Reproduce and fix remaining THIR- or inference-related ICEs, then decide
      how to treat stale fixture crash artifacts.
