use super::*;

struct TargetQueueState {
    target_api: usize,
    unsafe_wrapper: String,
    obligated_params: Vec<usize>,
    has_explicit_obligations: bool,
    base_sequences_before_cap: usize,
    base_materialization_failure: Option<&'static str>,
    seen_signatures: HashSet<u64>,
    queue: Vec<SequenceCandidate>,
    accepted_sequences: Vec<SequencePlan>,
    stats_by_round: HashMap<usize, MutatorRoundSummary>,
    target_sequence_cap: usize,
    accepted_mutator_counts: HashMap<MutatorIdentity, u32>,
    accepted_depth_counts: HashMap<usize, u32>,
    max_accepted_mutator_depth: usize,
    stagnation_depth_anchor: Option<usize>,
    next_stagnation_sequence_ratio: f32,
    stagnation_penalty_level: usize,
    reuse_penalty_enabled: bool,
    depth_penalty_enabled: bool,
    rejected_candidates_wrapper_sequence_cap: usize,
    rejected_candidates_target_sequence_cap: usize,
    stop_reason: Option<&'static str>,
}

struct MutatorInsertion<'a> {
    call_index: usize,
    obligation_param_index: usize,
    receiver_source: ValueSource,
    mutator_api: usize,
    mutated_param_index: usize,
    selected_obligation_fields: &'a FieldSet,
    plan: &'a CallPlan,
    prefix_distance: f32,
    target_distance: f32,
}

struct MaterializedTargetSuites {
    sequences: Vec<(Vec<CallStep>, f32, Option<usize>)>,
    failure_reason: Option<&'static str>,
}

fn sequence_cap_reached(cap: usize, accepted: usize) -> bool {
    cap != 0 && accepted >= cap
}

fn min_nonzero_cap(left: usize, right: usize) -> usize {
    match (left, right) {
        (0, 0) => 0,
        (0, value) | (value, 0) => value,
        (left, right) => left.min(right),
    }
}

fn pop_next_ranked_candidate(
    candidates: &mut Vec<SequenceCandidate>,
    states: &HashMap<usize, TargetQueueState>,
    tuning: &PlannerTuning,
) -> Option<(SequenceCandidate, CandidateEvaluation)> {
    let best_index = candidates
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            let left_evaluation = evaluate_candidate_for_states(left, states, tuning);
            let right_evaluation = evaluate_candidate_for_states(right, states, tuning);
            left_evaluation
                .queue_priority
                .partial_cmp(&right_evaluation.queue_priority)
                .unwrap()
                .then(left.sequence.steps.len().cmp(&right.sequence.steps.len()))
                .then(left.expansion_depth.cmp(&right.expansion_depth))
                .then(
                    symbolic_param_count(&left.sequence.steps)
                        .cmp(&symbolic_param_count(&right.sequence.steps)),
                )
                .then(
                    sequence_signature(&left.sequence.steps)
                        .cmp(&sequence_signature(&right.sequence.steps)),
                )
                .then(left.sequence.target_api.cmp(&right.sequence.target_api))
        })
        .map(|(index, _)| index)?;
    let candidate = candidates.remove(best_index);
    let evaluation = evaluate_candidate_for_states(&candidate, states, tuning);
    Some((candidate, evaluation))
}

fn evaluate_candidate_for_states(
    candidate: &SequenceCandidate,
    states: &HashMap<usize, TargetQueueState>,
    tuning: &PlannerTuning,
) -> CandidateEvaluation {
    let Some(state) = states.get(&candidate.sequence.target_api) else {
        return evaluate_candidate(candidate, 0.0, 0.0, tuning);
    };

    let reuse_penalty = if state.reuse_penalty_enabled {
        candidate
            .mutator_identity
            .and_then(|identity| state.accepted_mutator_counts.get(&identity).copied())
            .unwrap_or(0) as f32
    } else {
        0.0
    };
    let depth_penalty = if state.depth_penalty_enabled {
        depth_saturation_penalty(candidate, state, tuning)
            + stagnation_depth_penalty(candidate, state, tuning)
    } else {
        0.0
    };
    evaluate_candidate(candidate, reuse_penalty, depth_penalty, tuning)
}

fn depth_saturation_penalty(
    candidate: &SequenceCandidate,
    state: &TargetQueueState,
    tuning: &PlannerTuning,
) -> f32 {
    let accepted_at_depth = state
        .accepted_depth_counts
        .get(&candidate.expansion_depth)
        .copied()
        .unwrap_or(0) as f32;
    tuning.depth_saturation_weight * accepted_at_depth
}

fn stagnation_depth_penalty(
    candidate: &SequenceCandidate,
    state: &TargetQueueState,
    tuning: &PlannerTuning,
) -> f32 {
    let Some(depth_anchor) = state.stagnation_depth_anchor else {
        return 0.0;
    };
    if state.target_sequence_cap == 0 {
        return 0.0;
    }
    if state.stagnation_penalty_level == 0
        || state.max_accepted_mutator_depth != depth_anchor
        || candidate.expansion_depth > state.max_accepted_mutator_depth
    {
        return 0.0;
    }

    let depth_deficit = state.max_accepted_mutator_depth + 1 - candidate.expansion_depth;
    tuning.stagnation_depth_weight * state.stagnation_penalty_level as f32 * depth_deficit as f32
}

fn update_stagnation_phase(
    state: &mut TargetQueueState,
    previous_sequence_ratio: f32,
    sequence_ratio: f32,
    tuning: &PlannerTuning,
) {
    if state.target_sequence_cap == 0 {
        return;
    }

    while previous_sequence_ratio < state.next_stagnation_sequence_ratio
        && sequence_ratio >= state.next_stagnation_sequence_ratio
        && state.next_stagnation_sequence_ratio < 1.0
    {
        if state
            .stagnation_depth_anchor
            .is_some_and(|anchor| anchor == state.max_accepted_mutator_depth)
        {
            state.stagnation_penalty_level += 1;
        } else {
            state.stagnation_penalty_level = 1;
        }
        state.stagnation_depth_anchor = Some(state.max_accepted_mutator_depth);
        state.next_stagnation_sequence_ratio =
            next_stagnation_sequence_ratio(state.next_stagnation_sequence_ratio, tuning);
    }
}

fn next_stagnation_sequence_ratio(current: f32, tuning: &PlannerTuning) -> f32 {
    current + (1.0 - current) * tuning.stagnation_sequence_threshold
}

fn constructor_candidate_priority(api: &ApiDescriptor<'_>) -> (usize, usize, usize, usize) {
    (
        api.inputs.len(),
        usize::from(api.is_mono),
        mono_target_type_complexity(api),
        api.path.len(),
    )
}

fn generic_api_output_may_supply_input<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
    input: Ty<'tcx>,
) -> bool {
    let identity_args = ty::GenericArgs::identity_for_item(tcx, def_id);
    let fn_sig = crate::rapx_graph::fn_sig_with_generic_args(def_id, identity_args, tcx);
    let (value_output, _) = adapted_output(tcx, fn_sig.output());
    output_shape_may_supply_input(value_output, input)
}

fn output_shape_may_supply_input<'tcx>(output: Ty<'tcx>, input: Ty<'tcx>) -> bool {
    let output = output.peel_refs();
    let input = input.peel_refs();
    match input.kind() {
        TyKind::Adt(input_def, _) => match output.kind() {
            TyKind::Adt(output_def, _) => input_def.did() == output_def.did(),
            TyKind::Param(_) => true,
            _ => false,
        },
        TyKind::Tuple(input_tys) => match output.kind() {
            TyKind::Tuple(output_tys) => input_tys.len() == output_tys.len(),
            TyKind::Param(_) => true,
            _ => false,
        },
        TyKind::Slice(input_inner) | TyKind::Array(input_inner, _) => match output.kind() {
            TyKind::Slice(output_inner) | TyKind::Array(output_inner, _) => {
                output_shape_may_supply_input(*output_inner, *input_inner)
            }
            TyKind::Param(_) => true,
            _ => false,
        },
        TyKind::Param(_) => true,
        _ => output == input || matches!(output.kind(), TyKind::Param(_)),
    }
}

impl<'tcx, 'a> Planner<'tcx, 'a> {
    /// Synthesize all target suites associated with one unsafe-function family.
    pub(super) fn synthesize_target_group(
        &mut self,
        unsafe_wrapper: &str,
        target_group: &[usize],
        sequences: &mut Vec<SequencePlan>,
        target_summaries: &mut Vec<TargetSynthesisSummary>,
    ) {
        let group_start = Instant::now();
        info!(
            target: "foursafer::planner",
            "obligation-driven function `{}` has {} target api instances",
            unsafe_wrapper,
            target_group.len()
        );

        let wrapper_sequence_cap = self
            .remaining_sequence_capacity(sequences.len())
            .map_or(self.max_sequences_per_wrapper, |remaining| {
                min_nonzero_cap(self.max_sequences_per_wrapper, remaining)
            });
        let (group_sequences, group_summaries) = self
            .synthesize_wrapper_group_with_candidate_queue(
                unsafe_wrapper,
                target_group,
                wrapper_sequence_cap,
            );
        self.append_target_sequences(sequences, group_sequences);
        target_summaries.extend(group_summaries);

        info!(
            target: "foursafer::planner",
            "obligation-driven function `{}` finished in {:.2?}",
            unsafe_wrapper,
            group_start.elapsed()
        );
    }

    pub(super) fn synthesize_target_group_unlimited(
        &mut self,
        unsafe_wrapper: &str,
        target_group: &[usize],
        sequences: &mut Vec<SequencePlan>,
        target_summaries: &mut Vec<TargetSynthesisSummary>,
    ) {
        let group_start = Instant::now();
        info!(
            target: "foursafer::planner",
            "obligation-driven unlimited function `{}` has {} target api instances",
            unsafe_wrapper,
            target_group.len()
        );

        for &target_api in target_group {
            if self.sequence_limit_reached(sequences.len()) {
                info!(
                    target: "foursafer::planner",
                    "global suite sequence cap reached; skipping remaining targets in `{}`",
                    unsafe_wrapper
                );
                break;
            }
            let (target_sequences, target_summary) =
                self.synthesize_target_suite_unlimited(target_api, unsafe_wrapper);
            self.append_target_sequences(sequences, target_sequences);
            target_summaries.push(target_summary);
        }

        info!(
            target: "foursafer::planner",
            "obligation-driven unlimited function `{}` finished in {:.2?}",
            unsafe_wrapper,
            group_start.elapsed()
        );
    }

    fn synthesize_wrapper_group_with_candidate_queue(
        &mut self,
        unsafe_wrapper: &str,
        target_group: &[usize],
        wrapper_sequence_cap: usize,
    ) -> (Vec<SequencePlan>, Vec<TargetSynthesisSummary>) {
        let mut states = target_group
            .iter()
            .filter_map(|&target_api| {
                self.initialize_target_state(target_api, unsafe_wrapper)
                    .map(|state| (target_api, state))
            })
            .collect::<HashMap<_, _>>();

        let mut queue = states
            .values()
            .flat_map(|state| state.queue.iter().cloned())
            .collect::<Vec<_>>();
        for state in states.values_mut() {
            state.queue.clear();
        }

        let mut wrapper_sequences_accepted = 0usize;
        while let Some((mut candidate, evaluation)) =
            pop_next_ranked_candidate(&mut queue, &states, &self.tuning)
        {
            let target_api = candidate.sequence.target_api;

            if sequence_cap_reached(wrapper_sequence_cap, wrapper_sequences_accepted) {
                if let Some(state) = states.get_mut(&target_api) {
                    state.rejected_candidates_wrapper_sequence_cap += 1;
                }
                for queued in &queue {
                    if let Some(state) = states.get_mut(&queued.sequence.target_api) {
                        state.rejected_candidates_wrapper_sequence_cap += 1;
                        state
                            .stop_reason
                            .get_or_insert("wrapper_sequence_cap_reached");
                    }
                }
                for state in states.values_mut() {
                    state
                        .stop_reason
                        .get_or_insert("wrapper_sequence_cap_reached");
                }
                queue.clear();
                break;
            }

            let Some(state_view) = states.get(&target_api) else {
                continue;
            };
            if sequence_cap_reached(
                state_view.target_sequence_cap,
                state_view.accepted_sequences.len(),
            ) {
                let queued_for_capped_target = queue
                    .iter()
                    .filter(|queued| queued.sequence.target_api == target_api)
                    .count();
                let Some(state) = states.get_mut(&target_api) else {
                    continue;
                };
                state.rejected_candidates_target_sequence_cap += 1 + queued_for_capped_target;
                state
                    .stop_reason
                    .get_or_insert("target_sequence_cap_reached");
                queue.retain(|queued| queued.sequence.target_api != target_api);
                continue;
            }

            let Some(state) = states.get_mut(&target_api) else {
                continue;
            };
            let candidate_depth = candidate.expansion_depth;
            let current_index = state.accepted_sequences.len();
            let previous_sequence_ratio = if state.target_sequence_cap > 0 {
                state.accepted_sequences.len() as f32 / state.target_sequence_cap as f32
            } else {
                0.0
            };
            let predecessor = candidate.sequence.predecessor;
            candidate.sequence.ranking_value = evaluation.queue_priority;
            candidate.sequence.construction_cost = evaluation.construction_cost;
            candidate.sequence.prefix_distance = evaluation.prefix_distance;
            candidate.sequence.target_to_wrapper_distance = evaluation.target_to_wrapper_distance;
            candidate.sequence.total_distance = evaluation.total_distance;
            candidate.sequence.reuse_penalty = evaluation.reuse_penalty;
            candidate.sequence.queue_priority = evaluation.queue_priority;
            let sequence = candidate.sequence;
            if let Some(predecessor) = predecessor {
                state.accepted_sequences[predecessor]
                    .successor
                    .push(current_index);
            }
            state.accepted_sequences.push(sequence.clone());
            wrapper_sequences_accepted += 1;
            *state
                .accepted_depth_counts
                .entry(candidate_depth)
                .or_insert(0) += 1;
            state.max_accepted_mutator_depth =
                state.max_accepted_mutator_depth.max(candidate_depth);
            if let Some(identity) = candidate.mutator_identity {
                *state.accepted_mutator_counts.entry(identity).or_insert(0) += 1;
            }
            let sequence_ratio = if state.target_sequence_cap > 0 {
                state.accepted_sequences.len() as f32 / state.target_sequence_cap as f32
            } else {
                0.0
            };
            update_stagnation_phase(state, previous_sequence_ratio, sequence_ratio, &self.tuning);

            if sequence_cap_reached(state.target_sequence_cap, state.accepted_sequences.len()) {
                state
                    .stop_reason
                    .get_or_insert("target_sequence_cap_reached");
                let queued_for_capped_target = queue
                    .iter()
                    .filter(|queued| queued.sequence.target_api == target_api)
                    .count();
                state.rejected_candidates_target_sequence_cap += queued_for_capped_target;
                queue.retain(|queued| queued.sequence.target_api != target_api);
                continue;
            }
            if sequence_cap_reached(wrapper_sequence_cap, wrapper_sequences_accepted) {
                state
                    .stop_reason
                    .get_or_insert("wrapper_sequence_cap_reached");
                continue;
            }

            if candidate.expansion_depth >= self.max_mutators_per_target {
                continue;
            }

            let expansion = self.expand_with_mutators(&sequence);
            let round = candidate.expansion_depth;
            let stat = state
                .stats_by_round
                .entry(round)
                .or_insert(MutatorRoundSummary {
                    round,
                    parent_sequences_considered: 0,
                    raw_candidates: 0,
                    duplicate_candidates: 0,
                    depth_pruned_candidates: 0,
                    selected_candidates: 0,
                    retained_candidates: 0,
                    stop_reason: None,
                });
            stat.parent_sequences_considered += 1;
            stat.depth_pruned_candidates += expansion.depth_pruned_candidates;

            let mut new_candidates = expansion.candidates;
            stat.raw_candidates += new_candidates.len();
            new_candidates =
                rank_and_limit_mutation_candidates(new_candidates, round, &self.tuning);
            stat.selected_candidates += new_candidates.len();

            let mut retained = 0usize;
            for mut child in new_candidates {
                let signature = sequence_signature(&child.sequence.steps);
                if !state.seen_signatures.insert(signature) {
                    stat.duplicate_candidates += 1;
                    continue;
                }
                child.sequence.predecessor = Some(current_index);
                child.expansion_depth = candidate.expansion_depth + 1;
                queue.push(child);
                retained += 1;
            }
            stat.retained_candidates += retained;
        }

        let mut all_sequences = Vec::new();
        let mut summaries = Vec::new();
        for target_api in target_group {
            let Some(mut state) = states.remove(target_api) else {
                continue;
            };
            if state.accepted_sequences.is_empty() {
                state.stop_reason.get_or_insert("no_base_sequences");
            } else if state.stop_reason.is_none() {
                state.stop_reason = Some("queue_exhausted");
            }
            summaries.push(TargetSynthesisSummary {
                target_api: state.target_api,
                unsafe_wrapper: state.unsafe_wrapper,
                obligated_params: state.obligated_params,
                has_explicit_obligations: state.has_explicit_obligations,
                base_sequences_before_cap: state.base_sequences_before_cap,
                base_sequences_after_cap: state
                    .accepted_sequences
                    .iter()
                    .filter(|sequence| sequence.predecessor.is_none())
                    .count(),
                base_materialization_failure: state.base_materialization_failure,
                wrapper_sequence_cap,
                wrapper_sequences_accepted,
                target_sequence_cap: state.target_sequence_cap,
                target_sequences_accepted: state.accepted_sequences.len(),
                basic_sequence_cap: self.max_basic_sequences_per_target,
                rejected_candidates_wrapper_sequence_cap: state
                    .rejected_candidates_wrapper_sequence_cap,
                rejected_candidates_target_sequence_cap: state
                    .rejected_candidates_target_sequence_cap,
                mutator_rounds: finalize_round_summaries(state.stats_by_round, state.stop_reason),
                stop_reason: state.stop_reason.unwrap_or("queue_exhausted"),
            });
            let base_offset = all_sequences.len();
            for mut sequence in state.accepted_sequences {
                if let Some(predecessor) = sequence.predecessor {
                    sequence.predecessor = Some(base_offset + predecessor);
                }
                for successor in &mut sequence.successor {
                    *successor += base_offset;
                }
                all_sequences.push(sequence);
            }
        }
        (all_sequences, summaries)
    }

    fn synthesize_target_suite_unlimited(
        &mut self,
        target_api: usize,
        unsafe_wrapper: &str,
    ) -> (Vec<SequencePlan>, TargetSynthesisSummary) {
        let obligated_param_selection = select_obligated_params(&self.apis[target_api]);
        if obligated_param_selection.params.is_empty() && !self.apis[target_api].inputs.is_empty() {
            return (
                Vec::new(),
                TargetSynthesisSummary {
                    target_api,
                    unsafe_wrapper: unsafe_wrapper.to_owned(),
                    obligated_params: Vec::new(),
                    has_explicit_obligations: obligated_param_selection.has_explicit_obligations,
                    base_sequences_before_cap: 0,
                    base_sequences_after_cap: 0,
                    base_materialization_failure: Some("no_obligated_params"),
                    wrapper_sequence_cap: 0,
                    wrapper_sequences_accepted: 0,
                    target_sequence_cap: 0,
                    target_sequences_accepted: 0,
                    basic_sequence_cap: 0,
                    rejected_candidates_wrapper_sequence_cap: 0,
                    rejected_candidates_target_sequence_cap: 0,
                    mutator_rounds: Vec::new(),
                    stop_reason: "no_obligated_params",
                },
            );
        }

        let mut seen_signatures = HashSet::new();
        let (basic_candidates, base_sequences_before_cap, base_materialization_failure) = self
            .synthesize_base_target_sequences(
                target_api,
                unsafe_wrapper,
                &obligated_param_selection,
                &mut seen_signatures,
            );
        let (mut target_sequences, mutator_rounds, stop_reason) = self
            .generate_target_sequences_with_candidate_queue_unlimited(
                basic_candidates,
                &mut seen_signatures,
            );

        if obligated_param_selection.params.len() > 1
            && target_sequences
                .iter()
                .any(|sequence| sequence.predecessor.is_some())
            && target_sequences.len() > self.caps.multi_obligated_param_sequences_per_target
        {
            cap_sequence_plans_preserving_predecessors(
                &mut target_sequences,
                self.caps.multi_obligated_param_sequences_per_target,
            );
        }

        let stop_reason = if target_sequences.is_empty() {
            "no_base_sequences"
        } else {
            stop_reason
        };
        (
            target_sequences.clone(),
            TargetSynthesisSummary {
                target_api,
                unsafe_wrapper: unsafe_wrapper.to_owned(),
                obligated_params: obligated_param_selection.params,
                has_explicit_obligations: obligated_param_selection.has_explicit_obligations,
                base_sequences_before_cap,
                base_sequences_after_cap: target_sequences
                    .iter()
                    .filter(|sequence| sequence.predecessor.is_none())
                    .count(),
                base_materialization_failure: if target_sequences.is_empty() {
                    base_materialization_failure
                } else {
                    None
                },
                wrapper_sequence_cap: 0,
                wrapper_sequences_accepted: target_sequences.len(),
                target_sequence_cap: 0,
                target_sequences_accepted: target_sequences.len(),
                basic_sequence_cap: 0,
                rejected_candidates_wrapper_sequence_cap: 0,
                rejected_candidates_target_sequence_cap: 0,
                mutator_rounds,
                stop_reason,
            },
        )
    }

    fn generate_target_sequences_with_candidate_queue_unlimited(
        &mut self,
        mut queue: Vec<SequenceCandidate>,
        seen_signatures: &mut HashSet<u64>,
    ) -> (Vec<SequencePlan>, Vec<MutatorRoundSummary>, &'static str) {
        let mut target_sequences: Vec<SequencePlan> = Vec::new();
        let mut stats_by_round = HashMap::<usize, MutatorRoundSummary>::new();

        loop {
            if self.sequence_limit_reached(target_sequences.len()) {
                return (
                    target_sequences,
                    finalize_round_summaries(stats_by_round, Some("suite_sequence_cap_reached")),
                    "suite_sequence_cap_reached",
                );
            }
            if self.max_sequences != 0
                && !queue.is_empty()
                && target_sequences.len() + queue.len() >= self.max_sequences
            {
                self.drain_unlimited_queue_to_sequence_cap(&mut target_sequences, &mut queue);
                return (
                    target_sequences,
                    finalize_round_summaries(stats_by_round, Some("suite_sequence_cap_reached")),
                    "suite_sequence_cap_reached",
                );
            }

            let Some(candidate) = pop_next_unlimited_candidate(&mut queue, &self.tuning) else {
                break;
            };
            let current_index = target_sequences.len();

            let predecessor = candidate.sequence.predecessor;
            let sequence = candidate.sequence;
            if let Some(predecessor) = predecessor {
                target_sequences[predecessor].successor.push(current_index);
            }
            target_sequences.push(sequence.clone());

            if self.sequence_limit_reached(target_sequences.len()) {
                return (
                    target_sequences,
                    finalize_round_summaries(stats_by_round, Some("suite_sequence_cap_reached")),
                    "suite_sequence_cap_reached",
                );
            }

            if candidate.expansion_depth >= self.max_mutators_per_target {
                continue;
            }

            let expansion = self.expand_with_mutators(&sequence);
            let round = candidate.expansion_depth;
            let stat = stats_by_round.entry(round).or_insert(MutatorRoundSummary {
                round,
                parent_sequences_considered: 0,
                raw_candidates: 0,
                duplicate_candidates: 0,
                depth_pruned_candidates: 0,
                selected_candidates: 0,
                retained_candidates: 0,
                stop_reason: None,
            });
            stat.parent_sequences_considered += 1;
            stat.depth_pruned_candidates += expansion.depth_pruned_candidates;

            let mut new_candidates = expansion.candidates;
            stat.raw_candidates += new_candidates.len();
            if self.max_sequences != 0 {
                let remaining_candidate_capacity = self
                    .max_sequences
                    .saturating_sub(target_sequences.len() + queue.len());
                if remaining_candidate_capacity == 0 {
                    return (
                        target_sequences,
                        finalize_round_summaries(
                            stats_by_round,
                            Some("suite_sequence_cap_reached"),
                        ),
                        "suite_sequence_cap_reached",
                    );
                }
                new_candidates = rank_and_truncate_sequence_candidates(
                    new_candidates,
                    remaining_candidate_capacity,
                    &self.tuning,
                );
            } else if !self.relax_sequence_limit_filters {
                new_candidates =
                    rank_and_limit_mutation_candidates(new_candidates, round, &self.tuning);
            }
            stat.selected_candidates += new_candidates.len();

            let mut retained = 0usize;
            for mut child in new_candidates {
                let signature = sequence_signature(&child.sequence.steps);
                if !seen_signatures.insert(signature) {
                    stat.duplicate_candidates += 1;
                    continue;
                }
                child.sequence.predecessor = Some(current_index);
                child.expansion_depth = candidate.expansion_depth + 1;
                queue.push(child);
                retained += 1;
            }
            stat.retained_candidates += retained;
        }

        if self.max_mutators_per_target == 0 {
            return (
                target_sequences,
                finalize_round_summaries(stats_by_round, Some("mutator_limit_zero")),
                "mutator_limit_zero",
            );
        }
        let stop_reason = if target_sequences
            .iter()
            .any(|sequence| sequence.predecessor.is_some())
        {
            "no_new_mutator_candidates"
        } else {
            "basic_queue_exhausted"
        };
        (
            target_sequences,
            finalize_round_summaries(stats_by_round, Some(stop_reason)),
            stop_reason,
        )
    }

    fn drain_unlimited_queue_to_sequence_cap(
        &self,
        target_sequences: &mut Vec<SequencePlan>,
        queue: &mut Vec<SequenceCandidate>,
    ) {
        let Some(remaining) = self.remaining_sequence_capacity(target_sequences.len()) else {
            return;
        };
        if remaining == 0 {
            return;
        }

        rank_sequence_candidates(queue, &self.tuning);
        queue.truncate(remaining);
        for candidate in queue.drain(..) {
            let current_index = target_sequences.len();
            let predecessor = candidate.sequence.predecessor;
            let sequence = candidate.sequence;
            if let Some(predecessor) = predecessor {
                target_sequences[predecessor].successor.push(current_index);
            }
            target_sequences.push(sequence);
        }
    }

    fn initialize_target_state(
        &mut self,
        target_api: usize,
        unsafe_wrapper: &str,
    ) -> Option<TargetQueueState> {
        let obligated_param_selection = select_obligated_params(&self.apis[target_api]);
        if obligated_param_selection.params.is_empty() && !self.apis[target_api].inputs.is_empty() {
            return Some(TargetQueueState {
                target_api,
                unsafe_wrapper: unsafe_wrapper.to_owned(),
                obligated_params: Vec::new(),
                has_explicit_obligations: obligated_param_selection.has_explicit_obligations,
                base_sequences_before_cap: 0,
                base_materialization_failure: Some("no_obligated_params"),
                seen_signatures: HashSet::new(),
                queue: Vec::new(),
                accepted_sequences: Vec::new(),
                stats_by_round: HashMap::new(),
                target_sequence_cap: self.max_sequences_per_target,
                accepted_mutator_counts: HashMap::new(),
                accepted_depth_counts: HashMap::new(),
                max_accepted_mutator_depth: 0,
                stagnation_depth_anchor: None,
                next_stagnation_sequence_ratio: self.tuning.stagnation_sequence_threshold,
                stagnation_penalty_level: 0,
                reuse_penalty_enabled: self.reuse_penalty_enabled,
                depth_penalty_enabled: self.depth_penalty_enabled,
                rejected_candidates_wrapper_sequence_cap: 0,
                rejected_candidates_target_sequence_cap: 0,
                stop_reason: Some("no_obligated_params"),
            });
        }

        let mut seen_signatures = HashSet::new();
        let (basic_candidates, base_sequences_before_cap, base_materialization_failure) = self
            .synthesize_base_target_sequences(
                target_api,
                unsafe_wrapper,
                &obligated_param_selection,
                &mut seen_signatures,
            );

        Some(TargetQueueState {
            target_api,
            unsafe_wrapper: unsafe_wrapper.to_owned(),
            obligated_params: obligated_param_selection.params,
            has_explicit_obligations: obligated_param_selection.has_explicit_obligations,
            base_sequences_before_cap,
            base_materialization_failure,
            seen_signatures,
            queue: basic_candidates,
            accepted_sequences: Vec::new(),
            stats_by_round: HashMap::new(),
            target_sequence_cap: self.max_sequences_per_target,
            accepted_mutator_counts: HashMap::new(),
            accepted_depth_counts: HashMap::new(),
            max_accepted_mutator_depth: 0,
            stagnation_depth_anchor: None,
            next_stagnation_sequence_ratio: self.tuning.stagnation_sequence_threshold,
            stagnation_penalty_level: 0,
            reuse_penalty_enabled: self.reuse_penalty_enabled,
            depth_penalty_enabled: self.depth_penalty_enabled,
            rejected_candidates_wrapper_sequence_cap: 0,
            rejected_candidates_target_sequence_cap: 0,
            stop_reason: None,
        })
    }

    /// Materialize the base suites that reach the target without extra mutator
    /// expansion.
    fn synthesize_base_target_sequences(
        &mut self,
        target_api: usize,
        unsafe_wrapper: &str,
        obligated_param_selection: &ObligatedParamSelection,
        seen_signatures: &mut HashSet<u64>,
    ) -> (Vec<SequenceCandidate>, usize, Option<&'static str>) {
        let materialized = self.materialize_target_suites(
            target_api,
            &[basic_target_candidate(
                target_api,
                &obligated_param_selection.params,
                &self.apis,
            )],
            obligated_param_selection.has_explicit_obligations,
        );
        let basic_sequences = materialized.sequences;
        let base_sequences_before_cap = basic_sequences.len();
        debug!(
            target: "foursafer::planner",
            "target `{}` obligated_params={:?} basic_sequences={}",
            self.apis[target_api].path,
            obligated_param_selection.params,
            basic_sequences.len()
        );

        let mut target_sequences: Vec<SequenceCandidate> = Vec::new();
        for (steps, construction_cost, mutated_param) in basic_sequences {
            let signature = sequence_signature(&steps);
            if !seen_signatures.insert(signature) {
                continue;
            }
            target_sequences.push(self.basic_materialization_candidate(
                target_api,
                unsafe_wrapper,
                steps,
                construction_cost,
                mutated_param,
            ));
        }
        rank_sequence_candidates(&mut target_sequences, &self.tuning);
        if !self.relax_sequence_limit_filters && self.max_basic_sequences_per_target != 0 {
            target_sequences.truncate(self.max_basic_sequences_per_target);
        }
        if let Some(limit) = self.sequence_generation_limit() {
            target_sequences.truncate(limit);
        }
        let failure_reason = if target_sequences.is_empty() {
            materialized.failure_reason
        } else {
            None
        };
        (target_sequences, base_sequences_before_cap, failure_reason)
    }

    fn append_target_sequences(
        &self,
        sequences: &mut Vec<SequencePlan>,
        mut target_sequences: Vec<SequencePlan>,
    ) {
        let Some(remaining) = self.remaining_sequence_capacity(sequences.len()) else {
            self.offset_and_extend_target_sequences(sequences, target_sequences);
            return;
        };
        if remaining == 0 {
            return;
        }
        let uncapped_len = target_sequences.len();
        cap_sequence_plans_preserving_predecessors(&mut target_sequences, remaining);
        if target_sequences.len() < uncapped_len {
            info!(
                target: "foursafer::planner",
                "global suite sequence cap retained {} of {} pending target sequences (remaining={})",
                target_sequences.len(),
                uncapped_len,
                remaining
            );
        }
        self.offset_and_extend_target_sequences(sequences, target_sequences);
    }

    fn offset_and_extend_target_sequences(
        &self,
        sequences: &mut Vec<SequencePlan>,
        mut target_sequences: Vec<SequencePlan>,
    ) {
        let base_offset = sequences.len();
        for sequence in &mut target_sequences {
            if let Some(predecessor) = sequence.predecessor {
                sequence.predecessor = Some(base_offset + predecessor);
            }
            for successor in &mut sequence.successor {
                *successor += base_offset;
            }
        }
        sequences.extend(target_sequences);
    }

    fn max_depth_allows_len(&self, len: usize) -> bool {
        self.max_depth == 0 || len <= self.max_depth
    }

    fn expand_with_mutators(&mut self, base: &SequencePlan) -> MutatorExpansionResult {
        let obligations = self.sequence_obligations(base);
        let candidate_limit = self.sequence_generation_limit();
        let mut candidates = Vec::new();
        let mut depth_pruned_candidates = 0usize;
        let insertion_limit = earliest_mutator_insertion_index(base);

        'calls: for call_index in (0..=insertion_limit).rev() {
            let call = &base.steps[call_index];
            let inputs = self.apis[call.api_index].inputs.clone();

            for (param_index, arg) in call.args.iter().enumerate() {
                let Some(input) = inputs.get(param_index) else {
                    continue;
                };
                let obligation = obligations
                    .get(call_index)
                    .and_then(|params| params.get(param_index))
                    .cloned()
                    .unwrap_or_default();
                if obligation.is_empty() {
                    continue;
                }

                let Some(mutator_candidates) =
                    self.mutators.get(&obligated_state_ty(*input)).cloned()
                else {
                    continue;
                };
                let mut mutator_candidates = mutator_candidates
                    .into_iter()
                    .filter(|(mutator_api, mutated_param_index)| {
                        field_sets_overlap(
                            Some(&call_provided_fields(
                                &self.apis[*mutator_api],
                                *mutated_param_index,
                            )),
                            &obligation,
                        )
                    })
                    .collect::<Vec<_>>();
                let call_distance = distance_to_selected_obligation_site(base, call_index);
                let target_distance = average_distance_for_required_fields(
                    &self.apis[call.api_index],
                    param_index,
                    &obligation,
                )
                .unwrap_or(call_distance as f32);
                mutator_candidates.sort_by(|left, right| {
                    let left_provided = call_provided_fields(&self.apis[left.0], left.1);
                    let right_provided = call_provided_fields(&self.apis[right.0], right.1);
                    mutator_candidate_rank_key(
                        base,
                        left.0,
                        &left_provided,
                        &obligation,
                        call_distance,
                        target_distance,
                        &self.tuning,
                    )
                    .cmp(&mutator_candidate_rank_key(
                        base,
                        right.0,
                        &right_provided,
                        &obligation,
                        call_distance,
                        target_distance,
                        &self.tuning,
                    ))
                    .then(
                        left_provided.len().cmp(&right_provided.len()).then(
                            self.apis[left.0]
                                .inputs
                                .len()
                                .cmp(&self.apis[right.0].inputs.len()),
                        ),
                    )
                });
                if !self.relax_sequence_limit_filters {
                    mutator_candidates.truncate(self.tuning.max_mutator_candidates_per_param);
                } else if let Some(limit) = candidate_limit {
                    mutator_candidates.truncate(limit);
                }
                debug!(
                    target: "foursafer::planner",
                    "mutator screening: target=`{}` call=`{}` param={} obligation={:?} receiver_source={:?} matching_candidates={}",
                    self.apis[base.target_api].path,
                    self.apis[call.api_index].path,
                    param_index,
                    obligation,
                    arg.source,
                    mutator_candidates.len()
                );

                for (mutator_api, mutated_param_index) in mutator_candidates {
                    let provided =
                        call_provided_fields(&self.apis[mutator_api], mutated_param_index);
                    debug!(
                        target: "foursafer::planner",
                        "mutator candidate accepted: target=`{}` mutator=`{}` mutator_param={} provided={:?} obligation={:?}",
                        self.apis[base.target_api].path,
                        self.apis[mutator_api].path,
                        mutated_param_index,
                        provided,
                        obligation
                    );
                    let Some(plans) = self.build_call_plans(mutator_api, mutated_param_index)
                    else {
                        continue;
                    };
                    for plan in &plans {
                        let insertion = MutatorInsertion {
                            call_index,
                            obligation_param_index: param_index,
                            receiver_source: arg.source.clone(),
                            mutator_api,
                            mutated_param_index,
                            selected_obligation_fields: &obligation,
                            plan,
                            prefix_distance: call_distance.saturating_add(1) as f32,
                            target_distance,
                        };
                        let Some(candidate) = self.insert_mutator_before_call(base, insertion)
                        else {
                            depth_pruned_candidates += 1;
                            continue;
                        };
                        debug!(
                            target: "foursafer::planner",
                            "mutator inserted: target=`{}` mutator=`{}` before_call=`{}` obligation={:?} plan_prefix_steps={}",
                            self.apis[base.target_api].path,
                            self.apis[mutator_api].path,
                            self.apis[call.api_index].path,
                            obligation,
                            plan.prefix.len()
                        );
                        candidates.push(candidate);
                        if candidate_limit.is_some_and(|limit| candidates.len() >= limit) {
                            break 'calls;
                        }
                    }
                }
            }
        }

        MutatorExpansionResult {
            candidates,
            depth_pruned_candidates,
        }
    }

    fn sequence_obligations(&self, sequence: &SequencePlan) -> Vec<Vec<FieldSet>> {
        let mut source_obligations: HashMap<ValueSource, FieldSet> = HashMap::new();
        let mut obligations = sequence
            .steps
            .iter()
            .map(|step| vec![FieldSet::new(); self.apis[step.api_index].inputs.len()])
            .collect::<Vec<_>>();

        for step_index in (0..sequence.steps.len()).rev() {
            let step = &sequence.steps[step_index];
            let api = &self.apis[step.api_index];

            if step_index == sequence.target_step {
                for (param_index, arg) in step.args.iter().enumerate() {
                    let needed = initial_target_obligations(api, param_index);
                    if needed.is_empty() {
                        continue;
                    }
                    obligations[step_index][param_index].extend(needed.clone());
                    extend_source_obligation(&mut source_obligations, &arg.source, needed);
                }
            }

            let output_source = ValueSource::Step(step_index);
            let output_needed = source_obligations
                .remove(&output_source)
                .unwrap_or_default();
            if !output_needed.is_empty() {
                for (param_index, arg) in step.args.iter().enumerate() {
                    let required = output_required_fields(api, param_index, &output_needed);
                    if required.is_empty() {
                        continue;
                    }
                    obligations[step_index][param_index].extend(required.clone());
                    extend_source_obligation(&mut source_obligations, &arg.source, required);
                }
            }

            for (param_index, arg) in step.args.iter().enumerate() {
                let TyKind::Ref(_, _, ty::Mutability::Mut) = api.inputs[param_index].kind() else {
                    continue;
                };
                let current_needed = source_obligations
                    .get(&arg.source)
                    .cloned()
                    .unwrap_or_default();
                if current_needed.is_empty() {
                    continue;
                }

                let provided = call_provided_fields(api, param_index);
                let mut before = required_fields_before_call(
                    &current_needed,
                    &provided,
                    ALLOW_REQUIRED_PROVIDED_OVERLAP,
                );
                before.extend(call_required_fields(api, param_index));
                obligations[step_index][param_index].extend(before.clone());
                source_obligations.insert(arg.source.clone(), before);

                if field_sets_overlap(Some(&provided), &current_needed) {
                    for (other_index, other_arg) in step.args.iter().enumerate() {
                        if other_index == param_index {
                            continue;
                        }
                        let required =
                            effect_required_fields(api, param_index, other_index, &current_needed);
                        if required.is_empty() {
                            continue;
                        }
                        obligations[step_index][other_index].extend(required.clone());
                        extend_source_obligation(
                            &mut source_obligations,
                            &other_arg.source,
                            required,
                        );
                    }
                }
            }
        }

        obligations
    }

    fn insert_mutator_before_call(
        &self,
        base: &SequencePlan,
        insertion: MutatorInsertion<'_>,
    ) -> Option<SequenceCandidate> {
        let call_index = insertion.call_index;
        let plan = insertion.plan;
        let mut new_steps = base.steps[..call_index].to_vec();
        let step_offset = new_steps.len();
        let param_offset = insertion_param_offset(&base.steps);
        let mut extra_steps = plan.prefix.clone();
        shift_steps(&mut extra_steps, step_offset, param_offset);
        new_steps.extend(extra_steps);

        let mut args = Vec::with_capacity(plan.args.len());
        for (param_index, arg) in plan.args.iter().enumerate() {
            if param_index == insertion.mutated_param_index {
                args.push(ArgPlan {
                    source: insertion.receiver_source.clone(),
                    borrow: BorrowMode::Mutable,
                });
                continue;
            }

            let mut arg = arg.clone()?;
            shift_arg_refs(&mut arg, step_offset, param_offset);
            args.push(arg);
        }

        new_steps.push(CallStep {
            api_index: insertion.mutator_api,
            args,
        });
        let latest_call = new_steps.len() - 1;
        let inserted_count = latest_call + 1 - call_index;

        for step in &base.steps[call_index..] {
            let mut shifted = step.clone();
            shift_step_refs_at_or_after(&mut shifted, call_index, inserted_count);
            new_steps.push(shifted);
        }

        if !self.max_depth_allows_len(new_steps.len()) {
            return None;
        }

        let construction_cost = 1.0 + plan.prefix.len() as f32;
        let target_to_wrapper_distance = insertion.target_distance;
        let total_distance = self.tuning.prefix_distance_weight * insertion.prefix_distance
            + self.tuning.target_distance_weight * target_to_wrapper_distance;
        let queue_priority =
            self.tuning.construction_cost_weight * construction_cost + total_distance;

        Some(SequenceCandidate {
            construction_cost,
            prefix_distance: insertion.prefix_distance,
            target_to_wrapper_distance,
            mutator_identity: Some(MutatorIdentity {
                api_index: insertion.mutator_api,
                mutated_param_index: insertion.mutated_param_index,
            }),
            expansion_depth: 1,
            sequence: SequencePlan {
                target_api: base.target_api,
                unsafe_wrapper: base.unsafe_wrapper.clone(),
                target_step: if base.target_step >= call_index {
                    base.target_step + inserted_count
                } else {
                    base.target_step
                },
                predecessor: None,
                successor: Vec::new(),
                latest_call,
                mutated_param: Some(insertion.mutated_param_index),
                selected_obligation_call: Some(call_index + inserted_count),
                selected_obligation_param: Some(insertion.obligation_param_index),
                selected_obligation_fields: insertion.selected_obligation_fields.clone(),
                ranking_value: queue_priority,
                construction_cost,
                prefix_distance: insertion.prefix_distance,
                target_to_wrapper_distance,
                total_distance,
                reuse_penalty: 0.0,
                queue_priority,
                steps: new_steps,
            },
        })
    }

    fn basic_materialization_candidate(
        &self,
        target_api: usize,
        unsafe_wrapper: &str,
        steps: Vec<CallStep>,
        construction_cost: f32,
        mutated_param: Option<usize>,
    ) -> SequenceCandidate {
        let target_distance = average_target_distance(&self.apis[target_api]).unwrap_or(0.0);
        let prefix_distance = steps.len().saturating_sub(1) as f32;
        let target_to_wrapper_distance = target_distance;
        let total_distance = self.tuning.prefix_distance_weight * prefix_distance
            + self.tuning.target_distance_weight * target_to_wrapper_distance;
        let queue_priority =
            self.tuning.construction_cost_weight * construction_cost + total_distance;

        SequenceCandidate {
            construction_cost,
            prefix_distance,
            target_to_wrapper_distance,
            mutator_identity: None,
            expansion_depth: 0,
            sequence: SequencePlan {
                target_api,
                unsafe_wrapper: unsafe_wrapper.to_owned(),
                target_step: steps.len().saturating_sub(1),
                predecessor: None,
                successor: Vec::new(),
                latest_call: steps.len().saturating_sub(1),
                mutated_param,
                selected_obligation_call: None,
                selected_obligation_param: None,
                selected_obligation_fields: FieldSet::new(),
                ranking_value: queue_priority,
                construction_cost,
                prefix_distance,
                target_to_wrapper_distance,
                total_distance,
                reuse_penalty: 0.0,
                queue_priority,
                steps,
            },
        }
    }

    fn materialize_obligated_param_candidate(
        &mut self,
        target_api: usize,
        candidate: &ObligatedParamCandidatePath,
    ) -> Vec<ObligatedParamSteps> {
        let target = &self.apis[target_api];
        let obligated_param = candidate.obligated_param;
        let obligated_input = target.inputs[obligated_param];
        let constructor_candidates = self.constructor_candidates_for_obligated_param(
            obligated_input,
            &candidate.remaining_needed,
        );

        let sequence_limit = self.sequence_generation_limit();
        let mut base_variants = Vec::new();
        'constructors: for constructor in constructor_candidates {
            let constructor_steps =
                self.basic_sequences_ended_with_constructor(constructor, &mut Vec::new());
            for base_steps in constructor_steps {
                base_variants.push(ObligatedParamSteps {
                    obligated_param,
                    obligated_source: ValueSource::Step(base_steps.len().saturating_sub(1)),
                    steps: base_steps,
                    mutator_count: 0,
                    materialization_cost: 0.0,
                });
                if sequence_limit.is_some_and(|limit| base_variants.len() >= limit) {
                    break 'constructors;
                }
            }
        }
        let has_materialized_constructor_path = !base_variants.is_empty();
        if (supports_direct_symbolic_input(self.tcx, obligated_input)
            || is_pyo3_python_token_input(self.tcx, obligated_input))
            && !prefers_constructor_materialization_for_direct_target(
                &self.apis[target_api],
                obligated_input,
                has_materialized_constructor_path,
            )
        {
            base_variants.push(ObligatedParamSteps {
                obligated_param,
                steps: Vec::new(),
                obligated_source: ValueSource::Param(0),
                mutator_count: 0,
                materialization_cost: 0.0,
            });
        }
        let mut obligated_steps = dedupe_obligated_param_steps(base_variants);
        if let Some(limit) = sequence_limit {
            rank_and_truncate_obligated_param_steps(&mut obligated_steps, limit);
        } else if !self.relax_sequence_limit_filters {
            rank_and_truncate_obligated_param_steps(
                &mut obligated_steps,
                self.tuning.max_materialized_variants_per_obligated_param,
            );
        }
        obligated_steps
    }

    fn constructor_candidates_for_obligated_param(
        &mut self,
        obligated_input: Ty<'tcx>,
        required_fields: &FieldSet,
    ) -> Vec<usize> {
        let mut candidates =
            self.filtered_constructor_candidates_for_input(obligated_input, Some(required_fields));
        if candidates.is_empty() {
            self.ensure_generic_constructors_for_input(obligated_input);
            candidates = self
                .filtered_constructor_candidates_for_input(obligated_input, Some(required_fields));
        }
        self.rank_and_truncate_constructor_candidates(&mut candidates);
        candidates
    }

    fn basic_sequences_ended_with_constructor(
        &mut self,
        api_index: usize,
        api_stack: &mut Vec<usize>,
    ) -> Vec<Vec<CallStep>> {
        if let Some(cached) = self.basic_cache.get(&api_index) {
            return cached.clone();
        }
        if api_stack.contains(&api_index) {
            return Vec::new();
        }

        if self.apis[api_index].inputs.is_empty() {
            if let Some(steps) = append_api_call_simple(self.tcx, &self.apis, api_index, &[]) {
                self.basic_cache.insert(api_index, vec![steps.clone()]);
                return vec![steps];
            }
            self.basic_cache.insert(api_index, Vec::new());
            return Vec::new();
        }

        api_stack.push(api_index);
        let mut prefixes = vec![Vec::new()];
        for (param_index, input) in self.apis[api_index].inputs.clone().into_iter().enumerate() {
            if prefers_direct_symbolic_input(self.tcx, input) {
                continue;
            }
            let mut next_prefixes = Vec::new();

            let sequence_limit = self.sequence_generation_limit();
            let mut constructor_sequences = Vec::new();
            for constructor in self.constructor_candidates_for_input(api_index, param_index, input)
            {
                constructor_sequences
                    .extend(self.basic_sequences_ended_with_constructor(constructor, api_stack));
                if let Some(limit) = sequence_limit {
                    constructor_sequences = dedupe_step_sequences(constructor_sequences);
                    rank_and_truncate_step_sequences(&mut constructor_sequences, limit);
                    if constructor_sequences.len() >= limit {
                        break;
                    }
                } else if !self.relax_sequence_limit_filters
                    && constructor_sequences.len() >= self.tuning.max_basic_constructor_sequences
                {
                    break;
                }
            }

            if constructor_sequences.is_empty() {
                api_stack.pop();
                self.basic_cache.insert(api_index, Vec::new());
                return Vec::new();
            }

            'prefixes: for prefix in &prefixes {
                for constructor_steps in &constructor_sequences {
                    next_prefixes.push(merge_steps(prefix, constructor_steps));
                    if sequence_limit.is_some_and(|limit| next_prefixes.len() >= limit) {
                        break 'prefixes;
                    }
                }
            }

            prefixes = dedupe_step_sequences(next_prefixes);
            if let Some(limit) = sequence_limit {
                rank_and_truncate_step_sequences(&mut prefixes, limit);
            }
        }

        api_stack.pop();
        let sequences = prefixes
            .into_iter()
            .filter_map(|prefix| {
                append_api_call_with_prefix(self.tcx, &self.apis, api_index, &prefix)
            })
            .collect::<Vec<_>>();
        let mut sequences = dedupe_step_sequences(sequences);
        if let Some(limit) = self.sequence_generation_limit() {
            rank_and_truncate_step_sequences(&mut sequences, limit);
        } else if !self.relax_sequence_limit_filters {
            rank_and_truncate_step_sequences(
                &mut sequences,
                self.tuning.max_basic_constructor_sequences,
            );
        }
        self.basic_cache.insert(api_index, sequences.clone());
        sequences
    }

    fn build_call_plans(
        &mut self,
        api_index: usize,
        receiver_param: usize,
    ) -> Option<Vec<CallPlan>> {
        let cache_key = (api_index, receiver_param);
        if let Some(cached) = self.call_plan_cache.get(&cache_key) {
            return cached.clone();
        }

        let inputs = self.apis[api_index].inputs.clone();
        let mut partials = vec![PartialCallPlan {
            prefix: Vec::new(),
            args: vec![None; inputs.len()],
            next_param_index: 0,
        }];

        for (param_index, input) in inputs.iter().enumerate() {
            if param_index == receiver_param {
                continue;
            }

            let borrow = borrow_mode_for_input(*input);
            if prefers_direct_symbolic_input(self.tcx, *input) {
                for partial in &mut partials {
                    partial.args[param_index] = Some(ArgPlan {
                        source: ValueSource::Param(partial.next_param_index),
                        borrow,
                    });
                    partial.next_param_index += 1;
                }
                continue;
            }

            let constructor_sequences =
                self.collect_constructor_sequences_for_input(api_index, param_index, *input);
            if constructor_sequences.is_empty() {
                self.call_plan_cache.insert(cache_key, None);
                return None;
            }

            partials = extend_partials_with_constructor_sequences(
                partials,
                param_index,
                borrow,
                &constructor_sequences,
            );
            if let Some(limit) = self.sequence_generation_limit() {
                rank_and_truncate_partial_call_plans(&mut partials, limit);
            }
        }

        let mut deduped = dedupe_call_plans(partials);
        if let Some(limit) = self.sequence_generation_limit() {
            rank_and_truncate_call_plans(&mut deduped, limit);
        } else if !self.relax_sequence_limit_filters {
            rank_and_truncate_call_plans(&mut deduped, self.tuning.max_call_plan_candidates);
        }

        self.call_plan_cache
            .insert(cache_key, Some(deduped.clone()));
        Some(deduped)
    }

    fn build_target_plans(
        &mut self,
        api_index: usize,
        obligated_params: &[usize],
    ) -> Option<Vec<CallPlan>> {
        let inputs = self.apis[api_index].inputs.clone();
        let obligated = obligated_params.iter().copied().collect::<HashSet<_>>();
        let mut partials = vec![PartialCallPlan {
            prefix: Vec::new(),
            args: vec![None; inputs.len()],
            next_param_index: 0,
        }];

        for (param_index, input) in inputs.iter().enumerate() {
            if obligated.contains(&param_index) {
                continue;
            }

            let borrow = borrow_mode_for_input(*input);
            if prefers_direct_symbolic_input(self.tcx, *input) {
                for partial in &mut partials {
                    partial.args[param_index] = Some(ArgPlan {
                        source: ValueSource::Param(partial.next_param_index),
                        borrow,
                    });
                    partial.next_param_index += 1;
                }
                continue;
            }

            let constructor_sequences =
                self.collect_constructor_sequences_for_input(api_index, param_index, *input);
            if constructor_sequences.is_empty() {
                return None;
            }

            partials = extend_partials_with_constructor_sequences(
                partials,
                param_index,
                borrow,
                &constructor_sequences,
            );
            if let Some(limit) = self.sequence_generation_limit() {
                rank_and_truncate_partial_call_plans(&mut partials, limit);
            }
        }

        let mut deduped = dedupe_call_plans(partials);
        if let Some(limit) = self.sequence_generation_limit() {
            rank_and_truncate_call_plans(&mut deduped, limit);
        } else if !self.relax_sequence_limit_filters {
            rank_and_truncate_call_plans(&mut deduped, self.tuning.max_target_plan_candidates);
        }
        Some(deduped)
    }

    fn collect_constructor_sequences_for_input(
        &mut self,
        consumer_api: usize,
        param_index: usize,
        input: Ty<'tcx>,
    ) -> Vec<Vec<CallStep>> {
        let cache_key = (consumer_api, param_index);
        if let Some(cached) = self.constructor_sequence_cache.get(&cache_key) {
            return cached.clone();
        }

        let mut constructor_sequences = Vec::new();
        let sequence_limit = self.sequence_generation_limit();
        for constructor in self.constructor_candidates_for_input(consumer_api, param_index, input) {
            constructor_sequences
                .extend(self.basic_sequences_ended_with_constructor(constructor, &mut Vec::new()));
            if let Some(limit) = sequence_limit {
                constructor_sequences = dedupe_step_sequences(constructor_sequences);
                rank_and_truncate_step_sequences(&mut constructor_sequences, limit);
                if constructor_sequences.len() >= limit {
                    break;
                }
            }
        }
        self.constructor_sequence_cache
            .insert(cache_key, constructor_sequences.clone());
        constructor_sequences
    }

    fn constructor_candidates_for_input(
        &mut self,
        consumer_api: usize,
        param_index: usize,
        input: Ty<'tcx>,
    ) -> Vec<usize> {
        let required_fields =
            target_required_fields_for_param(&self.apis[consumer_api], param_index);
        let mut candidates =
            self.filtered_constructor_candidates_for_input(input, Some(&required_fields));
        if candidates.is_empty() {
            self.ensure_generic_constructors_for_input(input);
            candidates =
                self.filtered_constructor_candidates_for_input(input, Some(&required_fields));
        }
        self.rank_and_truncate_constructor_candidates(&mut candidates);
        candidates
    }

    fn rank_and_truncate_constructor_candidates(&self, candidates: &mut Vec<usize>) {
        candidates.sort_by(|left, right| {
            constructor_candidate_priority(&self.apis[*left])
                .cmp(&constructor_candidate_priority(&self.apis[*right]))
                .then_with(|| self.apis[*left].path.cmp(&self.apis[*right].path))
                .then_with(|| {
                    self.apis[*left]
                        .instantiated_path
                        .cmp(&self.apis[*right].instantiated_path)
                })
                .then(left.cmp(right))
        });
        candidates.dedup();
        if !self.relax_sequence_limit_filters {
            candidates.truncate(self.tuning.max_basic_constructor_sequences);
        }
    }

    fn filtered_constructor_candidates_for_input(
        &self,
        input: Ty<'tcx>,
        required_fields: Option<&FieldSet>,
    ) -> Vec<usize> {
        let candidates = if input_requires_constructor_scan(input) {
            self.constructors
                .values()
                .flat_map(|bucket| bucket.iter().copied())
                .collect::<Vec<_>>()
        } else {
            let key = normalize_value_ty(input);
            self.constructors.get(&key).cloned().unwrap_or_default()
        };
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter(|api_index| seen.insert(*api_index))
            .filter(|api_index| {
                can_supply_to_input(self.tcx, self.apis[*api_index].value_output, input)
            })
            .filter(|api_index| {
                required_fields.is_none_or(|fields| {
                    fields.is_empty()
                        || self.apis[*api_index]
                            .field_facts
                            .output_overlaps_relevant(fields)
                        || self.is_constructor_descriptor(*api_index)
                })
            })
            .collect()
    }

    fn materialize_target_suites(
        &mut self,
        target_api: usize,
        candidates: &[MultiObligatedParamCandidatePath],
        has_explicit_root_obligations: bool,
    ) -> MaterializedTargetSuites {
        if candidates.is_empty() {
            return MaterializedTargetSuites {
                sequences: Vec::new(),
                failure_reason: Some("no_candidate_paths"),
            };
        }

        let obligated_params = candidates[0]
            .obligated_params
            .iter()
            .map(|candidate| candidate.obligated_param)
            .collect::<Vec<_>>();
        let is_indirect_target =
            matches!(self.apis[target_api].api_safety, ApiSafety::IndirectUnsafe);
        let should_cap_multi_obligated_param_sequences = is_indirect_target
            && has_explicit_root_obligations
            && obligated_params.len() > 1
            && candidates
                .iter()
                .any(|candidate| candidate.mutator_count > 0);
        let Some(mut target_plans) = self.build_target_plans(target_api, &obligated_params) else {
            return MaterializedTargetSuites {
                sequences: Vec::new(),
                failure_reason: Some("target_plan_unmaterializable"),
            };
        };
        let sequence_limit = self.sequence_generation_limit();
        if let Some(limit) = sequence_limit {
            rank_and_truncate_call_plans(&mut target_plans, limit);
        }

        let mut sequences = Vec::new();
        let mut saw_candidate_with_unmaterialized_obligation = false;
        let mut saw_depth_pruned_combined_prefix = false;
        for candidate in candidates {
            let obligated_param_variants = candidate
                .obligated_params
                .iter()
                .map(|candidate| {
                    (
                        candidate.obligated_param,
                        self.materialize_obligated_param_candidate(target_api, candidate),
                    )
                })
                .collect::<Vec<_>>();
            if obligated_param_variants
                .iter()
                .any(|(_, variants)| variants.is_empty())
            {
                saw_candidate_with_unmaterialized_obligation = true;
                continue;
            }

            let should_cap_combined = is_indirect_target
                && has_explicit_root_obligations
                && obligated_param_variants.len() > 1
                && obligated_param_variants.iter().any(|(_, variants)| {
                    variants
                        .iter()
                        .any(|obligated_steps| obligated_steps.mutator_count > 0)
                });

            let mut combined = vec![CombinedObligatedParamSteps {
                steps: Vec::new(),
                obligated_sources: HashMap::new(),
                mutator_count: 0,
                materialization_cost: 0.0,
            }];

            for (_obligated_param, variants) in &obligated_param_variants {
                let mut next = Vec::new();
                'combine: for prefix in &combined {
                    for obligated_steps in variants {
                        let candidate =
                            merge_combined_with_obligated_param(prefix, obligated_steps);
                        if candidate.mutator_count > self.max_mutators_per_target {
                            continue;
                        }
                        next.push(candidate);
                        if sequence_limit.is_some_and(|limit| next.len() >= limit) {
                            break 'combine;
                        }
                    }
                }
                combined = dedupe_combined_sequences(next);
                if let Some(limit) = sequence_limit {
                    rank_and_truncate_combined_sequences(&mut combined, limit);
                } else if !self.relax_sequence_limit_filters {
                    rank_and_truncate_combined_sequences(
                        &mut combined,
                        self.tuning.max_combined_prefixes,
                    );
                }
            }

            let before_depth_filter = combined.len();
            combined.retain(|prefix| {
                combined_prefix_has_depth_feasible_target_plan(
                    prefix,
                    &target_plans,
                    self.max_depth,
                )
            });
            if before_depth_filter > 0 && combined.is_empty() {
                saw_depth_pruned_combined_prefix = true;
            }
            if !self.relax_sequence_limit_filters && should_cap_combined {
                rank_and_truncate_combined_sequences(&mut combined, self.tuning.top_k_candidates);
            }

            'emit_sequences: for combined_prefix in combined {
                for plan in &target_plans {
                    if let Some(steps) = append_multi_obligated_param_call(
                        combined_prefix.steps.clone(),
                        &combined_prefix.obligated_sources,
                        &self.apis[target_api],
                        target_api,
                        plan,
                        self.max_depth,
                    ) {
                        sequences.push((
                            steps.clone(),
                            steps.len() as f32 + combined_prefix.materialization_cost,
                            obligated_params.first().copied(),
                            combined_prefix.mutator_count,
                        ));
                        if sequence_limit.is_some_and(|limit| sequences.len() >= limit) {
                            break 'emit_sequences;
                        }
                    }
                }
            }
            if sequence_limit.is_some_and(|limit| sequences.len() >= limit) {
                break;
            }
        }
        let has_mutator_materialization = sequences.iter().any(|sequence| sequence.3 > 0);
        if has_mutator_materialization {
            sequences.sort_by(|left, right| {
                left.3
                    .cmp(&right.3)
                    .then_with(|| left.1.partial_cmp(&right.1).unwrap())
                    .then(left.0.len().cmp(&right.0.len()))
                    .then(symbolic_param_count(&left.0).cmp(&symbolic_param_count(&right.0)))
                    .then(sequence_signature(&left.0).cmp(&sequence_signature(&right.0)))
            });
            if let Some(limit) = sequence_limit {
                sequences.truncate(limit);
            } else if !self.relax_sequence_limit_filters
                && should_cap_multi_obligated_param_sequences
            {
                sequences.truncate(self.caps.multi_obligated_param_sequences_per_target);
            }
        } else {
            sequences.sort_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap()
                    .then(left.0.len().cmp(&right.0.len()))
                    .then(symbolic_param_count(&left.0).cmp(&symbolic_param_count(&right.0)))
                    .then(sequence_signature(&left.0).cmp(&sequence_signature(&right.0)))
            });
        }
        if let Some(limit) = sequence_limit {
            sequences.truncate(limit);
        } else if !self.relax_sequence_limit_filters
            && is_indirect_target
            && has_explicit_root_obligations
            && obligated_params.len() > 1
        {
            sequences.truncate(self.caps.multi_obligated_param_sequences_per_target);
        }
        let sequences = sequences
            .into_iter()
            .map(
                |(steps, construction_cost, mutated_param, _mutator_count)| {
                    (steps, construction_cost, mutated_param)
                },
            )
            .collect::<Vec<_>>();
        let failure_reason = if sequences.is_empty() {
            if saw_candidate_with_unmaterialized_obligation {
                Some("obligated_param_unmaterializable")
            } else if saw_depth_pruned_combined_prefix {
                Some("combined_prefix_depth_pruned")
            } else {
                Some("target_call_append_failed")
            }
        } else {
            None
        };
        MaterializedTargetSuites {
            sequences,
            failure_reason,
        }
    }

    fn ensure_generic_constructors_for_input(&mut self, input: Ty<'tcx>) {
        let cache_key = self.tcx.erase_and_anonymize_regions(input).to_string();
        if self.generic_constructor_cache.contains_key(&cache_key) {
            return;
        }

        let mut added = Vec::new();
        let mut seen_added = HashSet::new();
        let mut rounds = 0usize;
        let mut matched = 0usize;

        let match_limit = self
            .tuning
            .max_basic_constructor_sequences
            .max(self.tuning.max_target_plan_candidates);

        'scan: loop {
            rounds += 1;
            let available_tys = self.available_constructor_seed_tys();
            let mut round_added = 0usize;
            let mut round_matched = 0usize;
            for fn_did in self.graph.all_apis() {
                if !self
                    .tcx
                    .generics_of(*fn_did)
                    .requires_monomorphization(self.tcx)
                {
                    continue;
                }
                if !generic_api_output_may_supply_input(self.tcx, *fn_did, input) {
                    continue;
                }

                for mono in rapx_mono::resolve_mono_apis(*fn_did, &available_tys, self.tcx) {
                    let args = self.tcx.mk_args(&mono.value);
                    if self.find_api_descriptor_index(*fn_did, args).is_some() {
                        continue;
                    }

                    let fn_sig =
                        crate::rapx_graph::fn_sig_with_generic_args(*fn_did, args, self.tcx);
                    let (value_output, output_adapter) = adapted_output(self.tcx, fn_sig.output());
                    if is_unit_ty(value_output) {
                        continue;
                    }

                    let index =
                        self.push_api_descriptor(*fn_did, args, value_output, output_adapter);
                    if !self.apis[index].supported {
                        continue;
                    }
                    if !self.is_constructor_descriptor(index) {
                        continue;
                    }

                    if !seen_added.insert(index) {
                        continue;
                    }

                    self.constructors
                        .entry(self.apis[index].value_output_key)
                        .or_default()
                        .push(index);
                    if can_supply_to_input(self.tcx, value_output, input) {
                        round_matched += 1;
                    }
                    round_added += 1;
                    added.push(index);

                    if !self.relax_sequence_limit_filters && matched + round_matched >= match_limit
                    {
                        matched += round_matched;
                        break 'scan;
                    }
                }
            }

            matched += round_matched;
            if round_added == 0 {
                break;
            }
        }

        debug!(
            target: "foursafer::planner",
            "generic constructor scan for `{}` matched {} mono outputs and added {} constructors in {} rounds",
            cache_key,
            matched,
            added.len(),
            rounds
        );

        self.generic_constructor_cache.insert(cache_key, added);
    }

    fn available_constructor_seed_tys(&self) -> HashSet<TyWrapper<'tcx>> {
        let mut tys = HashSet::new();
        for api in &self.apis {
            tys.insert(api.value_output_key);
            extend_available_ty_variants(&mut tys, api.value_output, self.tcx);
        }
        for ty in crate::std_adapters::common_symbolic_seed_value_tys(self.tcx) {
            extend_available_ty_variants(&mut tys, ty, self.tcx);
        }
        tys
    }

    fn find_api_descriptor_index(
        &self,
        def_id: rustc_hir::def_id::DefId,
        args: ty::GenericArgsRef<'tcx>,
    ) -> Option<usize> {
        let instantiated = self.tcx.def_path_str_with_args(def_id, args);
        self.apis
            .iter()
            .find(|api| api.instantiated_path == instantiated)
            .map(|api| api.index)
    }

    fn push_api_descriptor(
        &mut self,
        def_id: rustc_hir::def_id::DefId,
        args: ty::GenericArgsRef<'tcx>,
        value_output: Ty<'tcx>,
        output_adapter: OutputAdapter,
    ) -> usize {
        let path = render_api_path(self.tcx, self.graph, def_id, args);
        let instantiated_path = self.tcx.def_path_str_with_args(def_id, args);
        let identity_args = ty::GenericArgs::identity_for_item(self.tcx, def_id);
        let is_mono = self
            .tcx
            .generics_of(def_id)
            .requires_monomorphization(self.tcx)
            && args != identity_args;
        let concrete_args = render_concrete_args(self.tcx, self.graph, args, identity_args);
        let fn_sig = crate::rapx_graph::fn_sig_with_generic_args(def_id, args, self.tcx);
        let api_safety = classify_api_safety(def_id, self.tcx, self.field_facts_index);
        let supported =
            is_supported_callable(self.tcx, self.graph, def_id, args, &path, api_safety);
        let index = self.apis.len();
        self.apis.push(ApiDescriptor {
            index,
            target_family: path.clone(),
            path,
            instantiated_path,
            concrete_args,
            is_mono,
            inputs: fn_sig.inputs().to_vec(),
            value_output,
            value_output_key: TyWrapper::from(value_output),
            output_adapter,
            api_safety,
            field_facts: self.field_facts_index.field_facts_for(def_id),
            supported,
        });
        index
    }

    fn is_constructor_descriptor(&self, api_index: usize) -> bool {
        let api = &self.apis[api_index];
        !is_unit_ty(api.value_output)
            && !api
                .inputs
                .iter()
                .any(|input| normalize_value_ty(*input) == api.value_output_key)
    }
}

fn rank_and_truncate_partial_call_plans(partials: &mut Vec<PartialCallPlan>, limit: usize) {
    if partials.len() <= limit {
        return;
    }
    partials.sort_by(|left, right| {
        left.prefix
            .len()
            .cmp(&right.prefix.len())
            .then(symbolic_param_count(&left.prefix).cmp(&symbolic_param_count(&right.prefix)))
            .then(left.args.len().cmp(&right.args.len()))
            .then(partial_call_plan_signature(left).cmp(&partial_call_plan_signature(right)))
    });
    partials.truncate(limit);
}

fn partial_call_plan_signature(partial: &PartialCallPlan) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    partial.hash(&mut hasher);
    hasher.finish()
}

fn dedupe_call_plans(partials: Vec<PartialCallPlan>) -> Vec<CallPlan> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for partial in partials {
        let plan = CallPlan {
            prefix: partial.prefix,
            args: partial.args,
        };
        if seen.insert(plan.clone()) {
            deduped.push(plan);
        }
    }
    deduped
}

fn finalize_round_summaries(
    mut stats_by_round: HashMap<usize, MutatorRoundSummary>,
    terminal_reason: Option<&'static str>,
) -> Vec<MutatorRoundSummary> {
    let mut rounds = stats_by_round
        .drain()
        .map(|(_, summary)| summary)
        .collect::<Vec<_>>();
    rounds.sort_by_key(|summary| summary.round);
    if let Some(reason) = terminal_reason
        && let Some(last) = rounds.last_mut()
    {
        last.stop_reason = Some(reason);
    }
    rounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{CallStep, SequencePlan};
    use crate::unsafe_analysis::FieldSet;

    fn test_sequence(expansion_depth: usize) -> SequenceCandidate {
        SequenceCandidate {
            sequence: SequencePlan {
                steps: vec![CallStep {
                    api_index: 0,
                    args: Vec::new(),
                }],
                target_api: 0,
                unsafe_wrapper: String::new(),
                target_step: 0,
                predecessor: None,
                successor: Vec::new(),
                latest_call: 0,
                mutated_param: None,
                selected_obligation_call: None,
                selected_obligation_param: None,
                selected_obligation_fields: FieldSet::new(),
                ranking_value: 0.0,
                construction_cost: 1.0,
                prefix_distance: 0.0,
                target_to_wrapper_distance: 0.0,
                total_distance: 0.0,
                reuse_penalty: 0.0,
                queue_priority: 0.0,
            },
            construction_cost: 1.0,
            prefix_distance: 0.0,
            target_to_wrapper_distance: 0.0,
            mutator_identity: None,
            expansion_depth,
        }
    }

    fn test_state(target_sequence_cap: usize) -> TargetQueueState {
        TargetQueueState {
            target_api: 0,
            unsafe_wrapper: String::new(),
            obligated_params: Vec::new(),
            has_explicit_obligations: false,
            base_sequences_before_cap: 0,
            base_materialization_failure: None,
            seen_signatures: HashSet::new(),
            queue: Vec::new(),
            accepted_sequences: Vec::new(),
            stats_by_round: HashMap::new(),
            target_sequence_cap,
            accepted_mutator_counts: HashMap::new(),
            accepted_depth_counts: HashMap::new(),
            max_accepted_mutator_depth: 0,
            stagnation_depth_anchor: None,
            next_stagnation_sequence_ratio: PlannerTuning::default().stagnation_sequence_threshold,
            stagnation_penalty_level: 0,
            reuse_penalty_enabled: true,
            depth_penalty_enabled: true,
            rejected_candidates_wrapper_sequence_cap: 0,
            rejected_candidates_target_sequence_cap: 0,
            stop_reason: None,
        }
    }

    #[test]
    fn sequence_progress_activates_stagnation_depth_penalty() {
        let tuning = PlannerTuning::default();
        let mut state = test_state(4);
        state.max_accepted_mutator_depth = 1;

        update_stagnation_phase(&mut state, 0.25, 0.5, &tuning);

        assert_eq!(state.stagnation_penalty_level, 1);
        assert_eq!(state.stagnation_depth_anchor, Some(1));
        assert!(stagnation_depth_penalty(&test_sequence(0), &state, &tuning) > 0.0);
        assert_eq!(
            stagnation_depth_penalty(&test_sequence(2), &state, &tuning),
            0.0
        );
    }
}
