# Obligation-Driven Distance and Budget Design

Date: 2026-04-22  
Updated: 2026-04-25

## Goal

Improve `obligation-driven` sequence construction by preferring actions that are
closer to the unsafe wrapper whose obligation needs to be checked, while
controlling suite growth through a budgeted generation process.

This design is scoped to the `obligation-driven` strategy only. The
`unsafe-wrapper` strategy remains a baseline and is not changed by this design.

## Terminology

Use `unsafe_wrapper` for the wrapper-family anchor used by obligation-driven
planning.

Do not use `unsafe_function` for this concept. That name is ambiguous because it
can be confused with a real Rust `unsafe fn`. Reserve `unsafe_fn` terminology for
language-level unsafe functions or the `ApiSafety::UnsafeFn` classification.

In this document:

- `unsafe_wrapper` means the selected safe public wrapper/family that owns or
  reaches the unsafe code being verified.
- `target API` means the public API instance for which a target-focused sequence
  is synthesized.
- `basic action` means materializing a base sequence whose prefix consists of
  constructor-style calls and ends at the target API.
- `expansion action` means extending an accepted parent sequence, currently by
  inserting an obligation-preserving mutator.

## High-Level Direction

The planner should replace ad hoc local growth with an explicit model:

1. define distance-to-wrapper signals for obligation-relevant fields,
2. preserve those distance components in action metadata,
3. rank candidate actions with one weighted score/cost,
4. generate suites under target budgets derived from an unsafe-wrapper total budget,
5. keep search-space bounds separate from suite-shape policy.

The core intent is not "more sequences", but "more useful sequences":

- closer to the unsafe wrapper that owns the obligation,
- more field- and obligation-relevant,
- less dominated by repeated low-information mutator expansions,
- balanced between basic constructor diversity and mutator exploration.

## Confirmed Principles

### A. Unsafe wrapper is the reference point

Distance is defined relative to the selected unsafe wrapper, not primarily
relative to the target API itself.

The target API remains the sequence-construction proxy, but the optimization goal
is to get closer to the unsafe wrapper whose soundness obligation is being
checked.

### B. One unsafe wrapper at a time

Distances and budgets are computed within one selected `unsafe_wrapper` synthesis
family at a time.

Do not mix multiple unsafe wrappers or multiple unsafe sinks in one distance or
budget space.

### C. Field paths are the basic unit

Distance is defined for `field path`s, not only for whole parameters or objects.

Object-level or target-level summaries are derived by aggregating field-path
signals. The first-class reasoning unit remains field-sensitive.

### D. Distance has two components

Action distance is split into two components:

- `prefix_distance`
  - how far the selected prefix/insertion site is from the current target
    obligation chain,
- `target_to_wrapper_distance`
  - how far the current target obligation is from the selected unsafe wrapper.

The combined distance is weighted:

```text
total_distance =
    w_prefix * prefix_distance
  + w_target * target_to_wrapper_distance
```

Initial weights:

- `w_prefix = 1`
- `w_target = 1`

The weights are explicit because the two distances have different meanings and
may need independent tuning.

### E. `target_to_wrapper_distance` is obligation-driven

`target_to_wrapper_distance` should be derived from existing obligation and field
propagation facts when the analysis supports it cleanly.

The intended first implementation can use the currently available
obligation-input distance facts and aggregate them for the fields relevant to the
action being ranked. This is consistent with the direction of using the unsafe
wrapper as the reference point, rather than treating distance as merely local to
the target API.

### F. `target_to_wrapper_distance` is field-sensitive

The conceptual distance is per:

```text
(target API, obligated field path)
```

When a whole target or whole action needs one value, aggregate explicitly and
keep the aggregation rule visible. Do not hide the field-sensitive origin of the
value.

### G. `prefix_distance` is obligation-chain depth

`prefix_distance` should represent how far a prefix-producing or mutating action
is from the selected obligation site in the synthesized chain.

For the first implementation, it is acceptable to use the current sequence
obligation-site distance approximation, including the existing `+1` convention.
This preserves current behavior while making the component explicit.

Examples:

- if the path is `constructor <- target`, then `prefix_distance` is roughly `1`,
- if the path is `mutator <- constructor <- target`, then `prefix_distance` is
  roughly `2`.

The important requirement for the next implementation step is not to perfect the
exact off-by-one convention, but to record the component separately from
`target_to_wrapper_distance`.

### H. Distances are preserved as metadata

Actions should preserve the raw distance components as metadata:

- `prefix_distance`
- `target_to_wrapper_distance`
- `total_distance`

Do not only store a single merged distance. Keeping the components observable is
needed for debugging, metadata validation, and later weight tuning.

### I. Ranking uses one weighted score/cost

Do not rank actions lexicographically by `incremental_cost`, then distance, then
reuse penalty.

Use one weighted score/cost:

```text
action_score =
    w_delta * delta_cost
  + w_prefix * prefix_distance
  + w_target * target_to_wrapper_distance
  + w_reuse * reuse_penalty
```

Initial weights:

- `w_delta = 1`
- `w_prefix = 1`
- `w_target = 1`
- `w_reuse = 1`

Lower score is better.

Tie-breakers may still be used for deterministic ordering, but only after the
weighted score is compared.

### J. Budget consumes the same score/cost used for ranking

The score used to prioritize actions should also be the budget cost consumed when
an action is accepted.

This avoids drifting semantics where the queue orders actions by one formula but
budget accounting charges a different formula.

### K. Wrapper-level total budget, target-level admission

Generation has two budget concepts:

- one total budget assigned to an `unsafe_wrapper` family,
- derived target budgets under that wrapper.

After a target budget is assigned, runtime admission only needs to enforce:

- the target budget,
- the target's derived basic budget cap.

A separate wrapper-level runtime cap is unnecessary once the wrapper total
budget has been fully allocated into target budgets.

Therefore implementations should avoid stale or misleading unused
`wrapper_budget` runtime checks.

### L. Targets receive a share of the wrapper-level total budget

For one `unsafe_wrapper`, first compute a true wrapper-level total budget. That
budget grows with target count, but sublinearly:

```text
wrapper_budget_total = wrapper_budget_base + wrapper_budget_scale * sqrt(target_count)
```

Initial values:

- `guaranteed = max(basic_sequences_per_target, 4)`
- `wrapper_budget_total = wrapper_budget_base + wrapper_budget_scale * sqrt(target_count)`
- `wrapper_budget_base = 96`
- `wrapper_budget_scale = 32`

Then allocate this total wrapper budget into target budgets:

1. reserve the guaranteed floor for every target,
2. compute the remaining `extra_pool`,
3. distribute `extra_pool` by target priority.

The current target-priority share is:

```text
share(t) ∝ 1 / (1 + d_target(t))
```

where `d_target(t)` is the aggregated target-to-wrapper distance.

This means:

- wrapper budget grows with target count, but sublinearly,
- every target receives a guaranteed floor,
- closer targets receive a larger share of the remaining wrapper budget.

This design preserves a true wrapper-level total budget:

- `sum(target_budget_i) = wrapper_budget_total`

The wrapper-level decay, if used, should apply to wrapper-family totals across
different wrapper families rather than to target instances within one wrapper
family.
wrapper families rather than to target instances within one wrapper family.

The ranking used for target sharing should still prefer targets that are more
useful to explore first, including:

- smaller aggregated target-to-wrapper distance,
- direct `UnsafeBlock` targets before indirect ones,
- lower type complexity for mono instances,
- stable path-based tie-breaking.

### M. Basic actions also consume budget

Basic sequence materialization is not free.

Although a basic sequence prefix only contains constructor-style calls, constructor
combinations can grow exponentially. Basic actions must therefore participate in
budget accounting so the planner can trade off:

- richer base constructor coverage,
- versus deeper or broader mutator expansion.

### N. Basic actions have a budget fraction cap

To prevent basic materialization from exhausting a target's whole budget, each
target has a basic budget cap:

```text
basic_budget_cap(target) = target_budget(target) * basic_budget_fraction
```

Initial value:

- `basic_budget_fraction = 0.4`

Once accepted basic actions for a target consume this cap, additional basic
actions for that target should not be admitted. The remaining target budget stays
available for expansion actions.

### O. Basic and expansion costs share a framework

Basic and expansion actions use the same weighted score framework, but their
components can differ.

For a basic action:

- `delta_cost` should reflect the materialized basic sequence size or equivalent
  constructor-prefix cost,
- `prefix_distance` can use the current basic-prefix distance approximation,
- `target_to_wrapper_distance` uses the target-level or field-relevant target
  distance,
- `reuse_penalty = 0` in the first implementation.

For an expansion action:

- `delta_cost` should be incremental relative to the parent sequence,
- `prefix_distance` uses the selected insertion/obligation-site distance,
- `target_to_wrapper_distance` uses the selected obligated fields,
- `reuse_penalty` is target-level and dynamically computed from accepted history.

### P. Expansion cost is incremental

The `delta_cost` component for an expansion action should depend on what the new
action adds relative to its parent sequence, not on the full inherited sequence
history.

The exact formula remains tunable, but the invariant is:

- use incremental cost,
- do not repeatedly charge for inherited parent structure.

### Q. Reuse penalty is target-level and dynamic

The reuse penalty should be computed from accepted history for the current
target, not only from the parent sequence.

Recommended mutator identity:

```text
(mutator_api, mutated_param_index)
```

At selection time, the queue should compute:

```text
reuse_penalty = accepted_count_for_mutator_identity_under_target
```

This penalty must be dynamic. It should be recomputed when selecting the best
queue action, because accepting one action changes the accepted history for later
actions in the same target.

The current `Vec`-based queue with linear minimum selection is compatible with
this requirement and is preferable to a heap while scores are state-dependent.

### R. Basic and expansion actions share the queue

Basic actions and expansion actions should both be queue actions.

The queue starts with basic actions. When a basic action is accepted, it can seed
expansion actions from that accepted base sequence. When an expansion action is
accepted, it can seed further expansion actions until depth and budget constraints
are reached.

Admission checks:

- all accepted actions consume target budget by their weighted score,
- basic actions additionally consume the target's basic budget cap,
- expansion actions use target-level dynamic reuse penalty at selection time.

### S. Budget is the suite-shape policy; search bounds are still allowed

Final suite admission should be controlled by budget, not by fixed sequence-count
caps.

However, intermediate search-space bounds are still necessary to prevent
candidate explosion before the budgeted queue can make decisions.

Therefore:

- remove or weaken final admission caps such as fixed expanded-sequence counts,
- keep search-space bounds such as candidate-plan limits and combined-prefix
  limits,
- name and document remaining caps as search bounds, not suite policy.

## Recommended Generation Flow

For each unsafe wrapper group:

1. enumerate target APIs for the wrapper,
2. compute target-to-wrapper distance summaries,
3. allocate the wrapper budget into target budgets,
4. compute each target's `basic_budget_cap`,
5. materialize basic actions and seed the shared queue,
6. repeatedly select the currently lowest weighted-score action,
7. reject or skip actions that exceed their target or basic budget cap,
8. accept the action and charge its current score to budget,
9. update accepted-history state, including mutator reuse counts,
10. generate follow-up expansion actions from the accepted sequence,
11. stop when the queue is exhausted or all remaining actions exceed budget.

The queue is shared across targets within one unsafe wrapper group, but runtime
budget admission is target-level because the wrapper budget has already been
allocated.

## Recommended Data Model

The exact Rust types can be tuned during implementation, but the model should
include these conceptual records.

### Distance breakdown

```text
DistanceBreakdown {
  prefix_distance,
  target_to_wrapper_distance,
  total_distance,
}
```

`total_distance` is computed with explicit weights.

### Action score breakdown

```text
ActionScore {
  delta_cost,
  prefix_distance,
  target_to_wrapper_distance,
  reuse_penalty,
  weighted_score,
}
```

`weighted_score` is used for both queue ordering and budget consumption.

### Queue action

```text
QueueAction {
  action_kind,              // basic or expansion
  target_api,
  parent_sequence,
  child_sequence,
  distance_breakdown,
  delta_cost,
  mutator_identity,         // only for expansion actions
}
```

`reuse_penalty` and `weighted_score` may be computed dynamically rather than
stored permanently, because they depend on accepted target history.

### Target budget state

```text
TargetBudgetState {
  target_budget,
  target_budget_consumed,
  basic_budget_cap,
  basic_budget_consumed,
  accepted_mutator_counts,
}
```

`accepted_mutator_counts` is keyed by `(mutator_api, mutated_param_index)`.

## Recommended Scoring Rules

### Basic action score

For a basic action:

```text
basic_score =
    w_delta * basic_delta_cost
  + w_prefix * prefix_distance
  + w_target * target_to_wrapper_distance
```

`w_reuse * reuse_penalty` is omitted because `reuse_penalty = 0` for basic
actions in the first implementation.

### Expansion action score

For an expansion action:

```text
expansion_score =
    w_delta * expansion_delta_cost
  + w_prefix * prefix_distance
  + w_target * target_to_wrapper_distance
  + w_reuse * target_reuse_penalty
```

where:

- `expansion_delta_cost` is incremental relative to the parent sequence,
- `target_reuse_penalty` is dynamically computed from accepted target history,
- `target_to_wrapper_distance` is based on the selected obligated fields when
  available.

## Recommended Budget Rules

Let one unsafe wrapper have conceptual total budget `B_wrapper`.

### Target allocation

1. assign each target a guaranteed minimum `b_min`,
2. compute per-target aggregated distance `d_target(t)`,
3. distribute remaining wrapper budget using inverse-distance share:

```text
share(t) ∝ 1 / (1 + d_target(t))
```

4. final target budget:

```text
B_target(t) = b_min + extra_share(t)
```

5. do not separately enforce `B_wrapper` at runtime if:

```text
sum(B_target(t)) == B_wrapper
```

### Consumption

Each accepted action consumes the corresponding target budget by the action's
current weighted score.

Basic actions also consume the target's basic budget cap. Expansion actions do
not consume the basic budget cap.

## Legacy Caps and Search Bounds

Budget should replace final suite-shape caps.

Recommended changes:

- remove `expanded_sequences_per_target` as an admission stop condition,
- stop treating `basic_sequences_per_target` as a final suite-size policy,
- retain bounded candidate-generation limits such as action-plan, target-plan,
  combined-prefix, and mutator-candidate search bounds,
- retain especially defensive complex-shape bounds temporarily if they protect
  candidate generation rather than final admission.

Remaining bounds should be named and documented as search bounds. They should not
be presented as the policy that decides final suite size.

## Metadata and Debugging

Distance-guided generation is hard to validate unless metadata explains why
actions were admitted or skipped.

Recommended metadata additions:

- per-action distance breakdown,
- per-action weighted score breakdown,
- per-target budget summary,
- per-target basic budget summary,
- per-target accepted mutator reuse counts or reuse summary,
- stop reasons distinguishing budget exhaustion, basic-budget exhaustion, search
  exhaustion, and depth pruning.

This should extend current stop-reason metadata rather than replace it.

## Expected Behavior Changes

Relative to the current planner, this design should:

- make distance components observable and tunable,
- let distance, delta cost, and reuse jointly determine queue priority,
- charge both basic and expansion actions against budget,
- prevent basic materialization from exhausting a target budget,
- reduce repeated use of the same mutator under one target,
- keep target-level budgets consistent with wrapper-level allocation,
- make suite size explainable through budget consumption rather than fixed
  sequence-count caps.

## Residual Open Items

These are intentionally left open for later tuning:

- exact formulas for `basic_delta_cost` and `expansion_delta_cost`,
- exact aggregation rule for per-field target distance into `d_target(t)`,
- whether basic actions later need constructor/producer reuse penalties,
- whether reuse penalties should later include wrapper-global counts,
- whether sequence-local mutator repetition should become a separate weighted
  component,
- when distance should influence target ordering beyond budget allocation.

## Immediate Implementation Notes

The first implementation should prioritize these changes in order:

1. rename wrapper-family metadata from `unsafe_function` to `unsafe_wrapper`,
2. add explicit distance and score breakdown metadata,
3. replace lexicographic action ordering with weighted score ordering,
4. make target-level reuse penalty dynamic at queue selection time,
5. charge both basic and expansion actions to target budget,
6. add `basic_budget_fraction = 0.4`,
7. clean stale wrapper-budget runtime variables,
8. remove final admission sequence-count caps once budget accounting is in place.
