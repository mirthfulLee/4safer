# cargo-4safer

## Purpose

`cargo 4safer` synthesizes obligation-driven verification suites for Rust
library crates. It keeps the user entry point as `cargo 4safer`, reuses the
RAPx-derived API dependency graph directly through a rustc wrapper callback, and
emits a generated harness crate plus sequence metadata for suite validation.

This repository is the standalone extraction of the former
`obligation_driven` synthesis flow. There is no user-selectable synthesis
strategy: budgeted obligation-driven planning is the default, and unbudgeted
planning is exposed only through `--disable-budget-limit` / `--unbudgeted` for
ablation.

## Project Structure

- `src/cargo_phase.rs`: `cargo 4safer` entry, cargo invocation, wrapper setup,
  and optional generated-harness validation.
- `src/driver.rs`: rustc callback entry that starts analysis and logs the
  selected backend and budget mode.
- `src/pipeline.rs`: rustc-direct orchestration layer for graph building, MIR
  unsafe analysis, budget-mode dispatch, and harness emission.
- `src/rapx_graph.rs`: RAPx-derived API dependency graph model and rustc-side
  graph construction.
- `src/rapx_resolve.rs`: reachable-type iteration and generic API
  monomorphization over rustc `Ty` candidates.
- `src/rapx_mono.rs`: bounded multi-generic solving over rustc `GenericArg`
  vectors, including trait-self seeding support and partial/final bound checks.
- `src/unsafe_analysis.rs`: MIR-based unsafe propagation summaries for public
  callables, including field-sensitive touched/unsafe field tracking and the
  current contract placeholder.
- `src/synth/strategy.rs`: budget-mode dispatch into the obligation-driven
  synthesizer.
- `src/synth/api.rs`: shared API descriptor construction and safety
  classification.
- `src/synth/obligation_driven/`: target selection, candidate ranking, budgeted
  and unbudgeted obligation-driven planning, and sequence synthesis.
- `src/backend/`: tests/Kani/fuzz harness crate rendering and metadata emission.
- `src/workspace.rs`: package/lib target discovery through `cargo metadata`.
- `tests/smoke.rs`: end-to-end coverage for CLI flows, default test-backend
  harness generation, manifest/workspace targeting, and budget ablation.
- `fixtures/`: intentionally small local crates used to cover focused workflow
  and synthesis scenarios.

## Formatting

- After any Rust code change, run `cargo fmt` before finishing the task.
- Keep pure formatting changes separate from unrelated functional changes.

## Problem Solving

- Do not add fallback paths, degradation handling, defensive band-aids, local
  stabilizations, or output-side post-processing just to make a failing case
  pass.
- When an issue is found, prefer a root-cause fix in the analysis or synthesis
  algorithm.
- Do not add targeted branches or fixture-specific special cases. Express needed
  behavior through shared capability checks or reusable algorithms that stay
  consistent across resolution, planning, and backend rendering.
- If a temporary diagnostic or narrow experiment is needed, remove it once the
  underlying cause is understood.

## Synthesis Flow

1. `cargo 4safer ...` parses CLI args and runs `cargo check --lib` with this
   binary as `RUSTC_WRAPPER`.
2. The wrapper enters the rustc callback only for the primary library target.
3. The callback runs MIR-based unsafe-surface analysis over local callable APIs.
4. The callback builds the RAPx-style API dependency graph from public safe APIs.
5. The graph layer resolves generic APIs into bounded mono instances before
   sequence planning.
6. The obligation-driven synthesizer maps graph edges into:
   - base sequences for supported obligation targets,
   - mutator-expanded successor sequences,
   - fallible producer adaptation for `Result<T, E>` and `Option<T>`.
7. The renderer writes:
   - a generated harness crate,
   - per-API metadata,
   - per-sequence metadata,
   - per-target planning summaries.
8. Smoke tests optionally run `cargo test` on the generated harness to prove the
   emitted suite is executable, not just serialized.

## Obligation-Driven Semantics

- Target selection includes both direct unsafe-block wrappers and
  MIR-propagated indirect unsafe targets.
- Classification is driven by MIR dataflow, not only by call reachability.
- Relationships are established only when actual read/write flow exists through
  assignments, temporaries, returns, or call arguments.
- Field-sensitive summaries are used to filter constructor and mutator choices
  so unrelated field-only APIs are not pulled into a target sequence.
- APIs that transitively invoke unsafe code without writing back into relevant
  object fields can still be indirect targets, but they should not act as broad
  mutators just because they take `&mut self`.
- Contract extraction for unsafe calls and pointer operations is currently a
  placeholder and should not be presented as complete.

## Budget Modes

- The default mode is budgeted obligation-driven synthesis.
- `--disable-budget-limit` selects the unbudgeted planner for ablation.
- `--disable-reuse-penalty` and `--disable-depth-penalty` are separate scoring
  ablations and should not be conflated with budget removal.
- Do not reintroduce a strategy CLI flag for the old direct-wrapper planner.

## Generic Resolution Notes

- The current direction is rustc-direct only. Do not reintroduce rustdoc-based
  generic parsing or `clean::Type` matching.
- `rapx_resolve` and `rapx_mono` intentionally diverge from upstream RAPx where
  needed for wrapper synthesis:
  - seed concrete `Self` types for provided trait methods,
  - keep bounded support for multiple generic parameters,
  - allow common wrapper-facing std bounds such as `Into<String>`,
    `Into<Vec<u8>>`, and `AsRef<[u8]>`.
- Treat current multi-generic support as bounded rather than complete:
  const generics, associated-type-heavy predicates, and broader blanket-impl
  reasoning are still incomplete.
- Prefer adding fixture-backed coverage before broadening the solver. The target
  behavior should stay tied to unsafe-effect wrapper synthesis, not generic API
  enumeration for its own sake.

## Fixture Strategy

- `basic-lib`: safe-only smoke coverage for default test-backend generation and
  empty obligation-target suites.
- `feature-lib`: feature-gated API visibility and harness dependency features.
- `workspace`: workspace root discovery, package selection, and ambiguous-target
  failure behavior.
- `fallible-lib`: `Result`/`Option` producers plus mutator expansion over the
  unwrapped value.
- `rich-lib`: richer synthesis shape with multiple producer APIs for one state
  type, two mutators, observer + consumer coverage, and assertions over emitted
  sequence structure.
- `unsafe-wrapper-lib`: public safe wrapper APIs around internal unsafe blocks.
- `unsafe-wrapper-indirect-field-lib`: MIR-propagated indirect wrapper targets
  and field-sensitive exclusion of unrelated mutators.
- `unsafe-wrapper-indirect-assign-lib`: propagation through temporaries and
  assignments, including indirect targets that write back to one field but not
  another.
- `unsafe-wrapper-multi-generic-lib`: bounded multi-generic free-function and
  trait-default wrapper targets; used to validate rustc-direct mono resolution
  for multiple type parameters and trait `Self` seeding.

When adding fixtures, prefer targeted crates that each prove one concrete
workflow or synthesis property. Keep assertions robust against incidental output
formatting, and prefer metadata-driven checks when sequence structure matters.

## Current Limitations

- For a downstream API that needs some type `T`, the planner still chooses one
  preferred producer path at a time instead of exhaustively enumerating all
  alternative producer combinations.
- Mutator expansion is bounded by `max_mutators_per_target`.
- MIR-based indirect propagation is intentionally bounded:
  field paths are depth-limited and the current contract extraction is only a
  placeholder.
- Generic resolution handles some multi-generic wrapper shapes, but complex
  associated-type predicates, const generics, and broad external blanket-impl
  reasoning remain out of scope.
- Only public safe library APIs are synthesized today; unsafe functions,
  closures, and more advanced generic handling remain out of scope.
