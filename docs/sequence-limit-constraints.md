# Sequence Limit Constraints

This document summarizes the current sequence-limited obligation-driven
synthesizer. The important semantic change is that candidate cost is no longer a
quota that gets consumed. Cost is only an ordering signal. Generation is bounded
by explicit sequence counts.

## User-Facing Limits

The planner receives these count-based limits from the CLI:

| Option | Default | Meaning |
|---|---:|---|
| `--max-sequences` | `0` | Maximum sequences in the whole generated suite. `0` means no global cap. |
| `--max-sequences-per-wrapper` | `1024` | Maximum accepted sequences for one unsafe-wrapper group. `0` means no wrapper cap. |
| `--max-sequences-per-target` | `128` | Maximum accepted sequences for one obligation target. `0` means no target cap. |
| `--max-basic-sequences-per-target` | `32` | Maximum basic, non-mutator-expanded sequences initially queued for one target. `0` means no basic-sequence cap. |
| `--disable-sequence-limits` | `false` | Uses the sequence-unlimited planner for wrapper/target/basic limit ablation. The global `--max-sequences` cap can still stop generation. |

The corresponding internal configuration fields are:

```text
max_sequences
max_sequences_per_wrapper
max_sequences_per_target
max_basic_sequences_per_target
```

Two internal environment variables remain for bounded combinatorial expansion:

| Environment Variable | Default | Meaning |
|---|---:|---|
| `FOURSAFER_OD_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET` | `16` | Maximum retained sequences for an indirect target whose materialization combines multiple obligated parameters. |
| `FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES` | `128` | Maximum retained call-plan variants for constructing non-receiver arguments of a mutator or helper call. |

## Candidate Cost And Ordering

Each candidate sequence carries several ranking components:

```text
construction_cost
prefix_distance
target_to_wrapper_distance
reuse_penalty
expansion_depth
ordering_penalty
```

The base distance score is:

```text
total_distance =
    prefix_distance_weight * prefix_distance
  + target_distance_weight * target_to_wrapper_distance
```

The queue priority used by the limited planner is:

```text
queue_priority =
    construction_cost_weight * construction_cost
  + total_distance
  + reuse_penalty_weight * reuse_penalty
  + depth_penalty_weight * expansion_depth
  + ordering_penalty
```

Lower `queue_priority` wins. Ties are broken by shorter sequence length, lower
expansion depth, fewer symbolic parameters, sequence signature, and target API
index.

### Construction Cost

For a basic target materialization, `construction_cost` is produced by the
materialization path that builds the target call and its required inputs. For a
mutator insertion candidate:

```text
construction_cost = 1 + len(mutator_prefix)
```

where `mutator_prefix` is the constructor/prefix call chain needed before the
inserted mutator call.

### Reuse Penalty

Reuse penalty is target-local. For a mutator candidate identified by:

```text
mutator_identity = (mutator_api, mutated_param_index)
```

the reuse penalty is the number of already accepted sequences for the same
target that used the same identity:

```text
reuse_penalty = accepted_mutator_counts[mutator_identity]
```

It is `0` for basic candidates, candidates without a mutator identity, or when
`--disable-reuse-penalty` is set.

### Depth Penalties

Depth penalties are ordering-only terms. They do not reject candidates by
themselves.

The saturation penalty discourages repeatedly accepting candidates at the same
mutator expansion depth:

```text
depth_saturation_penalty =
    depth_saturation_weight * accepted_depth_counts[expansion_depth]
```

The stagnation penalty is activated by target sequence progress. For a target
with cap `target_sequence_cap`, define:

```text
sequence_ratio = accepted_sequences_for_target / target_sequence_cap
```

When `sequence_ratio` crosses `next_stagnation_sequence_ratio`, the planner
records the current maximum accepted mutator depth as `stagnation_depth_anchor`.
If later progress crosses another threshold without increasing that maximum
depth, the penalty level increases.

Thresholds advance geometrically toward `1.0`:

```text
next = current + (1 - current) * stagnation_sequence_threshold
```

With the default threshold `0.5`, the sequence-ratio checkpoints are:

```text
0.5, 0.75, 0.875, ...
```

The stagnation penalty applies only to candidates that do not exceed the current
maximum accepted depth:

```text
if candidate_depth <= max_accepted_mutator_depth
   and max_accepted_mutator_depth == stagnation_depth_anchor:

    depth_deficit = max_accepted_mutator_depth + 1 - candidate_depth

    stagnation_depth_penalty =
        stagnation_depth_weight * stagnation_penalty_level * depth_deficit
```

The final `ordering_penalty` term is:

```text
ordering_penalty =
    depth_saturation_penalty + stagnation_depth_penalty
```

## Limited Planner Flow

In normal mode, the planner synthesizes one unsafe-wrapper group at a time. Each
group contains all obligation targets associated with the same unsafe wrapper.

Before a wrapper group starts, its effective cap is clipped by remaining global
capacity:

```text
remaining_global =
    if max_sequences == 0:
        unlimited
    else:
        max_sequences - already_emitted_suite_sequences

wrapper_cap =
    min_nonzero(max_sequences_per_wrapper, remaining_global)
```

where:

```text
min_nonzero(0, 0) = 0
min_nonzero(0, x) = x
min_nonzero(x, 0) = x
min_nonzero(x, y) = min(x, y)
```

For each target, basic sequences are materialized, deduplicated, ranked, and
then capped:

```text
basic_queue = materialize_basic_target_sequences(target)
rank(basic_queue)

if max_basic_sequences_per_target != 0:
    truncate(basic_queue, max_basic_sequences_per_target)

if max_sequences != 0:
    truncate(basic_queue, max_sequences)
```

The wrapper group then uses a shared candidate queue across its targets:

```text
while queue is not empty:
    candidate = pop_lowest_queue_priority(queue)
    target = candidate.target

    if wrapper_cap != 0 and wrapper_accepted >= wrapper_cap:
        reject remaining candidates for wrapper cap
        stop wrapper group

    if target_cap != 0 and accepted[target] >= target_cap:
        reject remaining candidates for that target
        continue

    accept(candidate)
    wrapper_accepted += 1
    accepted[target] += 1
    update_reuse_depth_and_stagnation_state(target)

    if accepted[target] reached target_cap:
        drop queued candidates for that target
        continue

    if wrapper_accepted reached wrapper_cap:
        stop adding children
        continue

    if candidate.expansion_depth < max_mutators_per_target:
        children = expand_with_mutators(candidate)
        children = rank(children)
        enqueue_deduplicated(children)
```

A target stops with one of these main reasons:

```text
no_obligated_params
no_base_sequences
target_sequence_cap_reached
wrapper_sequence_cap_reached
queue_exhausted
```

The emitted metadata records both accepted sequence counts and cap-triggered
rejections:

```text
wrapper_sequence_cap
wrapper_sequences_accepted
target_sequence_cap
target_sequences_accepted
basic_sequence_cap
rejected_candidates_wrapper_sequence_cap
rejected_candidates_target_sequence_cap
```

## Global Suite Cap

The global suite cap is enforced in three places:

1. Before starting another wrapper group.
2. When appending a target or wrapper group's sequences to the suite.
3. During final suite capping and sorting.

The append/final cap preserves predecessor chains. If a selected sequence has a
predecessor, the predecessor chain must also fit in the remaining capacity.
Otherwise that selected sequence is skipped.

Conceptually:

```text
rank all sequences by:
    ranking_value,
    basic-before-expanded,
    sequence length,
    symbolic parameter count,
    sequence signature,
    target API

for sequence in ranked:
    chain = missing_predecessor_chain(sequence)
    if selected_count + len(chain) <= limit:
        select(chain)
```

After capping, predecessor and successor indices are remapped to the retained
sequence list.

## Sequence-Unlimited Mode

`--disable-sequence-limits` selects the sequence-unlimited planner. In this
mode, wrapper, target, and basic sequence caps are not used as stopping
conditions. The planner also relaxes several internal truncation filters used by
the limited mode.

However, `--max-sequences` is still honored if it is nonzero. This provides a
global hard cap for ablation runs that should avoid wrapper/target balancing but
still keep total output size bounded.

The sequence-unlimited candidate loop is target-local:

```text
while queue is not empty:
    if max_sequences != 0 and emitted >= max_sequences:
        stop with suite_sequence_cap_reached

    if max_sequences != 0 and emitted + queued >= max_sequences:
        drain best queued candidates until the global cap is filled
        stop with suite_sequence_cap_reached

    candidate = pop_lowest_base_candidate_priority(queue)
    accept(candidate)

    if candidate.expansion_depth < max_mutators_per_target:
        children = expand_with_mutators(candidate)
        if max_sequences != 0:
            children = rank_and_truncate(children, remaining_global_capacity)
        enqueue_deduplicated(children)
```

Unlimited-mode base candidate priority is:

```text
base_candidate_priority =
    construction_cost_weight * construction_cost
  + prefix_distance_weight * prefix_distance
  + target_distance_weight * target_to_wrapper_distance
```

It intentionally omits target-local reuse and depth progress penalties because
those penalties are part of the limited planner's balancing policy.

## Practical Interpretation

The count limits now have direct meanings:

- `--max-sequences` controls the generated suite size.
- `--max-sequences-per-wrapper` prevents one unsafe wrapper family from
  dominating the suite.
- `--max-sequences-per-target` prevents one target API from dominating its
  wrapper group.
- `--max-basic-sequences-per-target` controls how many initial producer/target
  materializations can seed later mutator expansion.
The cost fields answer a different question: among currently legal candidates,
which one should be accepted first? They do not determine whether a target still
has enough "budget" left. That decision is now made only by sequence counts.
