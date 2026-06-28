use super::*;

pub(super) fn earliest_mutator_insertion_index(sequence: &SequencePlan) -> usize {
    if sequence.latest_call < sequence.target_step {
        sequence.latest_call
    } else {
        sequence.steps.len().saturating_sub(1)
    }
}

pub(super) fn basic_target_candidate(
    target_api: usize,
    unsafe_flow_params: &[usize],
    apis: &[ApiDescriptor<'_>],
) -> MultiUnsafeFlowParamCandidatePath {
    MultiUnsafeFlowParamCandidatePath {
        unsafe_flow_params: unsafe_flow_params
            .iter()
            .map(|param_index| UnsafeFlowParamCandidatePath {
                unsafe_flow_param: *param_index,
                remaining_needed: initial_target_unsafe_flow_entries(
                    &apis[target_api],
                    *param_index,
                ),
            })
            .collect(),
    }
}

pub(super) fn extend_source_unsafe_flow_entries(
    pending_entries: &mut HashMap<ValueSource, FieldSet>,
    source: &ValueSource,
    required: FieldSet,
) {
    pending_entries
        .entry(source.clone())
        .or_default()
        .extend(required);
}

pub(super) fn target_required_fields(
    target: &ApiDescriptor<'_>,
    receiver_param: usize,
) -> FieldSet {
    target_required_fields_for_param(target, receiver_param)
}

pub(super) fn initial_target_unsafe_flow_entries(
    target: &ApiDescriptor<'_>,
    receiver_param: usize,
) -> FieldSet {
    let preconditions = call_required_fields(target, receiver_param);
    if !preconditions.is_empty() {
        return preconditions;
    }
    target_required_fields(target, receiver_param)
}

pub(super) fn target_required_fields_for_param(
    api: &ApiDescriptor<'_>,
    param_index: usize,
) -> FieldSet {
    let contract_fields = api
        .field_facts
        .contract_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default();
    if !contract_fields.is_empty() {
        return contract_fields;
    }
    let unsafe_relevance_fields = api
        .field_facts
        .unsafe_relevance_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default();
    if !unsafe_relevance_fields.is_empty() {
        return unsafe_relevance_fields;
    }
    api.field_facts
        .mutated_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn call_required_fields(api: &ApiDescriptor<'_>, param_index: usize) -> FieldSet {
    let contract_fields = api
        .field_facts
        .contract_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default();
    if !contract_fields.is_empty() {
        return contract_fields;
    }
    let unsafe_relevance_fields = api
        .field_facts
        .unsafe_relevance_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default();
    if !unsafe_relevance_fields.is_empty() {
        return unsafe_relevance_fields;
    }
    if api_has_contract_summary(api) {
        return FieldSet::new();
    }
    api.field_facts
        .unsafe_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn output_required_fields(
    api: &ApiDescriptor<'_>,
    param_index: usize,
    output_needed: &FieldSet,
) -> FieldSet {
    if output_needed.is_empty() || !api.field_facts.output_overlaps_relevant(output_needed) {
        return FieldSet::new();
    }

    let contract_fields = api
        .field_facts
        .contract_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default();
    if !contract_fields.is_empty() {
        return contract_fields;
    }

    api.field_facts
        .unsafe_relevance_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn effect_required_fields(
    api: &ApiDescriptor<'_>,
    receiver_param: usize,
    param_index: usize,
    needed: &FieldSet,
) -> FieldSet {
    if needed.is_empty()
        || !field_sets_overlap(Some(&call_provided_fields(api, receiver_param)), needed)
    {
        return FieldSet::new();
    }
    call_required_fields(api, param_index)
}

pub(super) fn adapted_output<'tcx>(
    tcx: TyCtxt<'tcx>,
    output: Ty<'tcx>,
) -> (Ty<'tcx>, OutputAdapter) {
    let TyKind::Adt(def, args) = output.kind() else {
        return (output, OutputAdapter::Plain);
    };

    if tcx.is_diagnostic_item(rustc_span::sym::Option, def.did())
        && let Some(inner) = args.types().next()
    {
        return (inner, OutputAdapter::Option);
    }

    if tcx.is_diagnostic_item(rustc_span::sym::Result, def.did())
        && let Some(ok_ty) = args.types().next()
    {
        return (ok_ty, OutputAdapter::Result);
    }

    (output, OutputAdapter::Plain)
}

pub(super) fn api_has_contract_summary(api: &ApiDescriptor<'_>) -> bool {
    !api.field_facts.contract_output.is_empty()
        || api
            .field_facts
            .contract_inputs
            .iter()
            .any(|fields| !fields.is_empty())
}

pub(super) fn call_provided_fields(api: &ApiDescriptor<'_>, param_index: usize) -> FieldSet {
    api.field_facts
        .mutated_inputs
        .get(param_index)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn average_distance_for_required_fields(
    api: &ApiDescriptor<'_>,
    param_index: usize,
    required: &FieldSet,
) -> Option<f32> {
    let map = api
        .field_facts
        .unsafe_relevance_input_distances
        .get(param_index)?;
    let mut total = 0u32;
    let mut count = 0u32;

    for field in required {
        if let Some(distance) = distance_for_field(map, field) {
            total += distance;
            count += 1;
        }
    }

    (count > 0).then_some(total as f32 / count as f32)
}

pub(super) fn average_target_distance(api: &ApiDescriptor<'_>) -> Option<f32> {
    let mut total = 0u32;
    let mut count = 0u32;

    for (required_fields, map) in api
        .field_facts
        .unsafe_relevance_inputs
        .iter()
        .zip(api.field_facts.unsafe_relevance_input_distances.iter())
    {
        for field in required_fields {
            if let Some(distance) = distance_for_field(map, field) {
                total += distance;
                count += 1;
            }
        }
    }

    (count > 0).then_some(total as f32 / count as f32)
}

pub(super) fn distance_for_field(map: &FieldDistanceMap, field: &FieldPath) -> Option<u32> {
    map.get(field).copied().or_else(|| {
        map.iter()
            .filter(|(candidate, _)| candidate.starts_with(field) || field.starts_with(candidate))
            .map(|(_, distance)| *distance)
            .min()
    })
}

pub(super) fn required_fields_before_call(
    current_needed: &FieldSet,
    provided: &FieldSet,
    allow_required_provided_overlap: bool,
) -> FieldSet {
    if allow_required_provided_overlap && field_sets_overlap(Some(provided), current_needed) {
        current_needed.clone()
    } else {
        subtract_covered_fields(current_needed, provided)
    }
}

pub(super) fn unsafe_flow_state_ty(input: Ty<'_>) -> TyWrapper<'_> {
    match input.kind() {
        TyKind::Ref(_, inner, _) | TyKind::RawPtr(inner, _) => TyWrapper::from(*inner),
        _ => TyWrapper::from(input),
    }
}

pub(super) fn subtract_covered_fields(needed: &FieldSet, provided: &FieldSet) -> FieldSet {
    needed
        .iter()
        .filter(|field| !provided.iter().any(|provider| field.starts_with(provider)))
        .cloned()
        .collect()
}

pub(super) fn append_api_call_simple<'tcx>(
    tcx: TyCtxt<'tcx>,
    apis: &[ApiDescriptor<'tcx>],
    api_index: usize,
    prefix: &[CallStep],
) -> Option<Vec<CallStep>> {
    let api = &apis[api_index];
    let mut steps = prefix.to_vec();
    let mut args = Vec::with_capacity(api.inputs.len());
    let mut next_param_index = symbolic_param_count(prefix);

    for input in &api.inputs {
        let borrow = borrow_mode_for_input(*input);
        if supports_direct_symbolic_input(tcx, *input) {
            args.push(ArgPlan {
                source: ValueSource::Param(next_param_index),
                borrow,
            });
            next_param_index += 1;
            continue;
        }
        return None;
    }

    steps.push(CallStep { api_index, args });
    Some(steps)
}

pub(super) fn append_api_call_with_prefix<'tcx>(
    tcx: TyCtxt<'tcx>,
    apis: &[ApiDescriptor<'tcx>],
    api_index: usize,
    prefix: &[CallStep],
) -> Option<Vec<CallStep>> {
    let api = &apis[api_index];
    let mut steps = prefix.to_vec();
    let mut args = Vec::with_capacity(api.inputs.len());
    let mut moved_steps = moved_steps_in_prefix(prefix);
    let mut local_mutable_borrows = HashSet::new();
    let mut local_shared_borrows = HashSet::new();
    let mut next_param_index = symbolic_param_count(prefix);

    for input in &api.inputs {
        let borrow = borrow_mode_for_input(*input);

        let mut satisfied = None;
        for source_step in 0..prefix.len() {
            if moved_steps.contains(&source_step) {
                continue;
            }
            if !can_supply_to_input(
                tcx,
                apis[prefix[source_step].api_index].value_output,
                *input,
            ) {
                continue;
            }
            if !borrow_is_available(
                borrow,
                source_step,
                &local_mutable_borrows,
                &local_shared_borrows,
            ) {
                continue;
            }

            match borrow {
                BorrowMode::Move => {
                    moved_steps.insert(source_step);
                }
                BorrowMode::Shared | BorrowMode::RawConst => {
                    local_shared_borrows.insert(source_step);
                }
                BorrowMode::Mutable | BorrowMode::RawMutable => {
                    local_mutable_borrows.insert(source_step);
                }
            }
            satisfied = Some(source_step);
            break;
        }

        if let Some(source_step) = satisfied {
            args.push(ArgPlan {
                source: ValueSource::Step(source_step),
                borrow,
            });
            continue;
        }

        if supports_direct_symbolic_input(tcx, *input) {
            args.push(ArgPlan {
                source: ValueSource::Param(next_param_index),
                borrow,
            });
            next_param_index += 1;
            continue;
        }

        return None;
    }

    steps.push(CallStep { api_index, args });
    Some(steps)
}

fn moved_steps_in_prefix(prefix: &[CallStep]) -> HashSet<usize> {
    let mut moved = HashSet::new();
    for step in prefix {
        for arg in &step.args {
            if matches!(arg.borrow, BorrowMode::Move)
                && let ValueSource::Step(source_step) = arg.source
            {
                moved.insert(source_step);
            }
        }
    }
    moved
}

pub(super) fn shift_steps(steps: &mut [CallStep], step_offset: usize, param_offset: usize) {
    for step in steps {
        for arg in &mut step.args {
            shift_arg_refs(arg, step_offset, param_offset);
        }
    }
}

pub(super) fn shift_arg_refs(arg: &mut ArgPlan, step_offset: usize, param_offset: usize) {
    match &mut arg.source {
        ValueSource::Param(index) => *index += param_offset,
        ValueSource::Step(index) => *index += step_offset,
    }
}

pub(super) fn normalize_value_ty(ty: Ty<'_>) -> TyWrapper<'_> {
    match ty.kind() {
        TyKind::Ref(_, inner, _) | TyKind::RawPtr(inner, _) => TyWrapper::from(*inner),
        _ => TyWrapper::from(ty),
    }
}

pub(super) fn borrow_mode_for_input(ty: Ty<'_>) -> BorrowMode {
    match ty.kind() {
        TyKind::Ref(_, _, ty::Mutability::Mut) => BorrowMode::Mutable,
        TyKind::Ref(_, _, ty::Mutability::Not) => BorrowMode::Shared,
        TyKind::RawPtr(_, ty::Mutability::Mut) => BorrowMode::RawMutable,
        TyKind::RawPtr(_, ty::Mutability::Not) => BorrowMode::RawConst,
        _ => BorrowMode::Move,
    }
}

pub(super) fn prefers_direct_symbolic_input<'tcx>(tcx: TyCtxt<'tcx>, input: Ty<'tcx>) -> bool {
    is_pyo3_python_token_input(tcx, input)
        || (supports_direct_symbolic_input(tcx, input) && !is_local_user_adt(seed_value_ty(input)))
}

pub(super) fn prefers_constructor_materialization_for_direct_target(
    api: &ApiDescriptor<'_>,
    input: Ty<'_>,
    has_constructor_candidates: bool,
) -> bool {
    has_constructor_candidates
        && matches!(api.api_safety, ApiSafety::UnsafeBlock)
        && is_local_user_adt(seed_value_ty(input))
}

pub(super) fn is_local_user_adt(value_ty: Ty<'_>) -> bool {
    let TyKind::Adt(def, _) = value_ty.kind() else {
        return false;
    };
    def.did().is_local() && (def.is_struct() || def.is_enum())
}

pub(super) fn is_unit_ty(ty: Ty<'_>) -> bool {
    matches!(ty.kind(), TyKind::Tuple(types) if types.is_empty())
}

pub(super) fn field_sets_overlap(left: Option<&FieldSet>, right: &FieldSet) -> bool {
    let Some(left) = left else {
        return false;
    };
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left.iter()
        .any(|lhs| right.iter().any(|rhs| paths_overlap(lhs, rhs)))
}

pub(super) fn paths_overlap(left: &FieldPath, right: &FieldPath) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

pub(super) fn dedupe_step_sequences(sequences: Vec<Vec<CallStep>>) -> Vec<Vec<CallStep>> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for steps in sequences {
        let signature = sequence_signature(&steps);
        if seen.insert(signature) {
            deduped.push(steps);
        }
    }
    deduped
}

pub(super) fn dedupe_unsafe_flow_param_steps(
    sequences: Vec<UnsafeFlowParamSteps>,
) -> Vec<UnsafeFlowParamSteps> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for unsafe_flow_steps in sequences {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        unsafe_flow_steps.steps.hash(&mut hasher);
        unsafe_flow_steps.unsafe_flow_param.hash(&mut hasher);
        unsafe_flow_steps.unsafe_flow_source.hash(&mut hasher);
        let signature = hasher.finish();
        if seen.insert(signature) {
            deduped.push(unsafe_flow_steps);
        }
    }
    deduped
}

pub(super) fn merge_combined_with_unsafe_flow_param(
    left: &CombinedUnsafeFlowParamSteps,
    right: &UnsafeFlowParamSteps,
) -> CombinedUnsafeFlowParamSteps {
    let step_offset = left.steps.len();
    let param_offset = combined_symbolic_param_count(&left.steps, &left.unsafe_flow_sources);
    let mut right_steps = right.steps.clone();
    shift_steps(&mut right_steps, step_offset, param_offset);

    let mut steps = left.steps.clone();
    steps.extend(right_steps);

    let mut unsafe_flow_sources = left.unsafe_flow_sources.clone();
    unsafe_flow_sources.insert(
        right.unsafe_flow_param,
        shift_value_source(&right.unsafe_flow_source, step_offset, param_offset),
    );

    CombinedUnsafeFlowParamSteps {
        steps,
        unsafe_flow_sources,
        mutator_count: left.mutator_count + right.mutator_count,
        materialization_cost: left.materialization_cost + right.materialization_cost,
    }
}

pub(super) fn dedupe_combined_sequences(
    sequences: Vec<CombinedUnsafeFlowParamSteps>,
) -> Vec<CombinedUnsafeFlowParamSteps> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for combined in sequences {
        let signature = combined_sequence_signature(&combined);
        if seen.insert(signature) {
            deduped.push(combined);
        }
    }
    deduped
}

pub(super) fn rank_and_truncate_combined_sequences(
    sequences: &mut Vec<CombinedUnsafeFlowParamSteps>,
    limit: usize,
) {
    sequences.sort_by(|left, right| {
        left.mutator_count
            .cmp(&right.mutator_count)
            .then_with(|| {
                left.materialization_cost
                    .partial_cmp(&right.materialization_cost)
                    .unwrap()
            })
            .then(left.steps.len().cmp(&right.steps.len()))
            .then(symbolic_param_count(&left.steps).cmp(&symbolic_param_count(&right.steps)))
            .then(combined_sequence_signature(left).cmp(&combined_sequence_signature(right)))
    });
    sequences.truncate(limit);
}

pub(super) fn rank_and_truncate_step_sequences(sequences: &mut Vec<Vec<CallStep>>, limit: usize) {
    if sequences.len() <= limit {
        return;
    }
    sequences.sort_by(|left, right| {
        left.len()
            .cmp(&right.len())
            .then(symbolic_param_count(left).cmp(&symbolic_param_count(right)))
            .then(sequence_signature(left).cmp(&sequence_signature(right)))
    });
    sequences.truncate(limit);
}

pub(super) fn rank_and_truncate_call_plans(plans: &mut Vec<CallPlan>, limit: usize) {
    if plans.len() <= limit {
        return;
    }
    plans.sort_by(|left, right| {
        left.prefix
            .len()
            .cmp(&right.prefix.len())
            .then(symbolic_param_count(&left.prefix).cmp(&symbolic_param_count(&right.prefix)))
            .then(left.args.len().cmp(&right.args.len()))
            .then(call_plan_signature(left).cmp(&call_plan_signature(right)))
    });
    plans.truncate(limit);
}

pub(super) fn rank_and_truncate_unsafe_flow_param_steps(
    sequences: &mut Vec<UnsafeFlowParamSteps>,
    limit: usize,
) {
    if sequences.len() <= limit {
        return;
    }
    sequences.sort_by(|left, right| {
        left.steps
            .len()
            .cmp(&right.steps.len())
            .then(left.mutator_count.cmp(&right.mutator_count))
            .then(symbolic_param_count(&left.steps).cmp(&symbolic_param_count(&right.steps)))
            .then(sequence_signature(&left.steps).cmp(&sequence_signature(&right.steps)))
            .then(left.unsafe_flow_param.cmp(&right.unsafe_flow_param))
    });
    sequences.truncate(limit);
}

pub(super) fn rank_and_limit_mutation_candidates(
    mut sequences: Vec<SequenceCandidate>,
    round: usize,
    tuning: &PlannerTuning,
) -> Vec<SequenceCandidate> {
    let size_limit = mutation_round_limit(round, tuning);
    if sequences.len() <= size_limit {
        return sequences;
    }

    sequences.sort_by(|left, right| compare_sequence_candidates(left, right, tuning));
    sequences.truncate(size_limit);
    sequences
}

pub(super) fn rank_sequence_candidates(
    candidates: &mut [SequenceCandidate],
    tuning: &PlannerTuning,
) {
    candidates.sort_by(|left, right| compare_sequence_candidates(left, right, tuning));
}

fn compare_sequence_candidates(
    left: &SequenceCandidate,
    right: &SequenceCandidate,
    tuning: &PlannerTuning,
) -> std::cmp::Ordering {
    base_candidate_priority(left, tuning)
        .partial_cmp(&base_candidate_priority(right, tuning))
        .unwrap()
        .then(
            right
                .sequence
                .ranking_value
                .partial_cmp(&left.sequence.ranking_value)
                .unwrap(),
        )
        .then(left.expansion_depth.cmp(&right.expansion_depth))
        .then(left.sequence.steps.len().cmp(&right.sequence.steps.len()))
        .then(
            symbolic_param_count(&left.sequence.steps)
                .cmp(&symbolic_param_count(&right.sequence.steps)),
        )
        .then(
            sequence_signature(&left.sequence.steps)
                .cmp(&sequence_signature(&right.sequence.steps)),
        )
        .then(left.sequence.target_api.cmp(&right.sequence.target_api))
}

pub(super) fn base_candidate_priority(
    candidate: &SequenceCandidate,
    tuning: &PlannerTuning,
) -> f32 {
    tuning.cost_weight * candidate.construction_cost - candidate_flow_score(candidate, tuning)
}

pub(super) fn evaluate_candidate(
    candidate: &SequenceCandidate,
    diversity_penalty: f32,
    ordering_penalty: f32,
    tuning: &PlannerTuning,
) -> CandidateEvaluation {
    let total_distance = candidate.prefix_distance + candidate.target_to_wrapper_distance;
    let flow_score = candidate_flow_score(candidate, tuning);
    let total_diversity_penalty = diversity_penalty + ordering_penalty;
    let priority_score = flow_score
        - tuning.cost_weight * candidate.construction_cost
        - tuning.diversity_penalty_weight * total_diversity_penalty;
    CandidateEvaluation {
        construction_cost: candidate.construction_cost,
        prefix_distance: candidate.prefix_distance,
        target_to_wrapper_distance: candidate.target_to_wrapper_distance,
        total_distance,
        priority_score,
        reuse_penalty: total_diversity_penalty,
        queue_priority: -priority_score,
    }
}

pub(super) fn candidate_flow_score(candidate: &SequenceCandidate, tuning: &PlannerTuning) -> f32 {
    flow_score_for_parts(
        candidate.sequence.selected_unsafe_flow_fields.len(),
        candidate.prefix_distance,
        candidate.target_to_wrapper_distance,
        tuning,
    )
}

pub(super) fn flow_score_for_parts(
    propagated_fields: usize,
    prefix_distance: f32,
    target_distance: f32,
    tuning: &PlannerTuning,
) -> f32 {
    if propagated_fields == 0 {
        return 0.0;
    }
    let distance = prefix_distance + target_distance;
    propagated_fields as f32 * tuning.r_flow.powf(distance.max(0.0))
}

pub(super) fn distance_to_selected_unsafe_flow_site(
    sequence: &SequencePlan,
    call_index: usize,
) -> u32 {
    distances_to_target(&sequence.steps, sequence.target_step)
        .get(call_index)
        .copied()
        .unwrap_or(u32::MAX)
}

pub(super) fn unsafe_flow_overlap_count(provided: &FieldSet, pending_entries: &FieldSet) -> usize {
    provided
        .iter()
        .filter(|field| {
            pending_entries
                .iter()
                .any(|needed| paths_overlap(field, needed))
        })
        .count()
}

pub(super) fn mutator_candidate_rank_key(
    _base: &SequencePlan,
    mutator_api: usize,
    provided: &FieldSet,
    pending_entries: &FieldSet,
    call_distance: u32,
    target_distance: f32,
    tuning: &PlannerTuning,
) -> (i32, i32, usize, usize) {
    let prefix_distance = call_distance.saturating_add(1) as f32;
    let overlap = unsafe_flow_overlap_count(provided, pending_entries) as i32;
    let flow_score = flow_score_for_parts(
        overlap.max(0) as usize,
        prefix_distance,
        target_distance,
        tuning,
    );
    let queue_priority = tuning.cost_weight - flow_score;
    let provided_width = provided.len();

    (
        (queue_priority * 1000.0).round() as i32,
        -overlap,
        provided_width,
        mutator_api,
    )
}

pub(super) fn distances_to_target(steps: &[CallStep], target_step: usize) -> Vec<u32> {
    let mut distances = vec![u32::MAX; steps.len()];
    if target_step >= steps.len() {
        return distances;
    }

    let mut queue = VecDeque::from([(target_step, 0u32)]);
    while let Some((step_index, distance)) = queue.pop_front() {
        if distances[step_index] <= distance {
            continue;
        }
        distances[step_index] = distance;

        for arg in &steps[step_index].args {
            if let ValueSource::Step(source_index) = arg.source {
                if distances[source_index] != u32::MAX {
                    continue;
                }
                queue.push_back((source_index, distance + 1));
            }
        }
    }

    for distance in &mut distances {
        if *distance == u32::MAX {
            *distance = 0;
        }
    }
    distances
}

pub(super) fn combined_prefix_has_depth_feasible_target_plan(
    prefix: &CombinedUnsafeFlowParamSteps,
    target_plans: &[CallPlan],
    max_depth: usize,
) -> bool {
    target_plans.iter().any(|plan| {
        let final_len = prefix.steps.len() + plan.prefix.len() + 1;
        max_depth == 0 || final_len <= max_depth
    })
}

pub(super) fn merge_steps(left: &[CallStep], right: &[CallStep]) -> Vec<CallStep> {
    let mut merged = left.to_vec();
    let mut shifted = right.to_vec();
    shift_steps(&mut shifted, merged.len(), symbolic_param_count(left));
    merged.extend(shifted);
    merged
}

pub(super) fn extend_partials_with_constructor_sequences(
    partials: Vec<PartialCallPlan>,
    param_index: usize,
    borrow: BorrowMode,
    constructor_sequences: &[Vec<CallStep>],
) -> Vec<PartialCallPlan> {
    let mut next_partials = Vec::new();
    for partial in partials {
        for constructor_steps in constructor_sequences {
            let mut prefix = partial.prefix.clone();
            let offset = prefix.len();
            let param_offset = partial.next_param_index;
            let mut shifted = constructor_steps.clone();
            shift_steps(&mut shifted, offset, param_offset);
            prefix.extend(shifted);

            let mut args = partial.args.clone();
            args[param_index] = Some(ArgPlan {
                source: ValueSource::Step(prefix.len() - 1),
                borrow,
            });

            next_partials.push(PartialCallPlan {
                prefix,
                args,
                next_param_index: partial.next_param_index
                    + symbolic_param_count(constructor_steps),
            });
        }
    }
    next_partials
}

pub(super) fn append_multi_unsafe_flow_param_call<'tcx>(
    mut steps: Vec<CallStep>,
    unsafe_flow_sources: &HashMap<usize, ValueSource>,
    api: &ApiDescriptor<'tcx>,
    api_index: usize,
    plan: &CallPlan,
    max_depth: usize,
) -> Option<Vec<CallStep>> {
    let step_offset = steps.len();
    let mut max_param = symbolic_param_count(&steps);
    for source in unsafe_flow_sources.values() {
        if let ValueSource::Param(index) = source {
            max_param = max_param.max(index + 1);
        }
    }
    let mut extra_steps = plan.prefix.clone();
    shift_steps(&mut extra_steps, step_offset, max_param);
    steps.extend(extra_steps);

    let mut args = Vec::with_capacity(api.inputs.len());
    for (param_index, input) in api.inputs.iter().enumerate() {
        if let Some(unsafe_flow_source) = unsafe_flow_sources.get(&param_index) {
            args.push(ArgPlan {
                source: unsafe_flow_source.clone(),
                borrow: borrow_mode_for_input(*input),
            });
            continue;
        }

        let mut arg = plan.args.get(param_index)?.clone()?;
        shift_arg_refs(&mut arg, step_offset, max_param);
        args.push(arg);
    }

    steps.push(CallStep { api_index, args });
    if max_depth != 0 && steps.len() > max_depth {
        return None;
    }
    Some(steps)
}

pub(super) fn insertion_param_offset(steps: &[CallStep]) -> usize {
    symbolic_param_count(steps)
}

pub(super) fn borrow_is_available(
    borrow: BorrowMode,
    source_step: usize,
    mutable_borrows: &HashSet<usize>,
    shared_borrows: &HashSet<usize>,
) -> bool {
    match borrow {
        BorrowMode::Move => {
            !mutable_borrows.contains(&source_step) && !shared_borrows.contains(&source_step)
        }
        BorrowMode::Shared | BorrowMode::RawConst => !mutable_borrows.contains(&source_step),
        BorrowMode::Mutable | BorrowMode::RawMutable => {
            !mutable_borrows.contains(&source_step) && !shared_borrows.contains(&source_step)
        }
    }
}

pub(super) fn shift_value_source(
    source: &ValueSource,
    step_offset: usize,
    param_offset: usize,
) -> ValueSource {
    match source {
        ValueSource::Param(index) => ValueSource::Param(index + param_offset),
        ValueSource::Step(index) => ValueSource::Step(index + step_offset),
    }
}

pub(super) fn combined_symbolic_param_count(
    steps: &[CallStep],
    unsafe_flow_sources: &HashMap<usize, ValueSource>,
) -> usize {
    let mut count = symbolic_param_count(steps);
    for source in unsafe_flow_sources.values() {
        if let ValueSource::Param(index) = source {
            count = count.max(index + 1);
        }
    }
    count
}

pub(super) fn sequence_signature(steps: &[CallStep]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    steps.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn call_plan_signature(plan: &CallPlan) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    plan.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn combined_sequence_signature(combined: &CombinedUnsafeFlowParamSteps) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    combined.steps.hash(&mut hasher);
    let mut unsafe_flow_entries = combined
        .unsafe_flow_sources
        .iter()
        .map(|(param, source)| (*param, source.clone()))
        .collect::<Vec<_>>();
    unsafe_flow_entries.sort_by_key(|(param, _)| *param);
    unsafe_flow_entries.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn symbolic_param_count(steps: &[CallStep]) -> usize {
    steps
        .iter()
        .flat_map(|step| step.args.iter())
        .filter_map(|arg| match arg.source {
            ValueSource::Param(index) => Some(index + 1),
            ValueSource::Step(_) => None,
        })
        .max()
        .unwrap_or(0)
}

pub(super) fn shift_step_refs_at_or_after(step: &mut CallStep, insertion_at: usize, delta: usize) {
    for arg in &mut step.args {
        if let ValueSource::Step(index) = &mut arg.source
            && *index >= insertion_at
        {
            *index += delta;
        }
    }
}

pub(super) fn mutation_round_limit(round: usize, tuning: &PlannerTuning) -> usize {
    match round {
        0 => tuning.mutation_round0_limit,
        1 => tuning.mutation_round1_limit,
        2 => tuning.mutation_round2_limit,
        3 => tuning.mutation_round3_limit,
        _ => tuning.top_k_candidates,
    }
}

pub(super) fn cap_sequence_plans_preserving_predecessors(
    sequences: &mut Vec<SequencePlan>,
    limit: usize,
) -> HashMap<usize, usize> {
    if sequences.len() <= limit {
        return (0..sequences.len()).map(|index| (index, index)).collect();
    }

    let mut ranked = (0..sequences.len()).collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        sequences[*right]
            .ranking_value
            .partial_cmp(&sequences[*left].ranking_value)
            .unwrap()
            .then(
                usize::from(sequences[*left].predecessor.is_some())
                    .cmp(&usize::from(sequences[*right].predecessor.is_some())),
            )
            .then(
                sequences[*left]
                    .steps
                    .len()
                    .cmp(&sequences[*right].steps.len()),
            )
            .then(
                symbolic_param_count(&sequences[*left].steps)
                    .cmp(&symbolic_param_count(&sequences[*right].steps)),
            )
            .then(
                sequence_signature(&sequences[*left].steps)
                    .cmp(&sequence_signature(&sequences[*right].steps)),
            )
            .then(
                sequences[*left]
                    .target_api
                    .cmp(&sequences[*right].target_api),
            )
    });

    let mut selected_set = HashSet::new();
    let mut selected = Vec::new();
    for index in ranked {
        let mut chain = Vec::new();
        let mut cursor = Some(index);
        while let Some(current) = cursor {
            if selected_set.contains(&current) {
                break;
            }
            chain.push(current);
            cursor = sequences[current].predecessor;
        }
        chain.reverse();
        let missing = chain
            .into_iter()
            .filter(|current| !selected_set.contains(current))
            .collect::<Vec<_>>();
        if selected_set.len() + missing.len() > limit {
            continue;
        }
        for current in missing {
            if selected_set.insert(current) {
                selected.push(current);
            }
        }
        if selected_set.len() >= limit {
            break;
        }
    }

    selected.sort_unstable();

    let index_map = selected
        .iter()
        .enumerate()
        .map(|(new_index, old_index)| (*old_index, new_index))
        .collect::<HashMap<_, _>>();
    let mut capped = selected
        .into_iter()
        .map(|old_index| sequences[old_index].clone())
        .collect::<Vec<_>>();

    for sequence in &mut capped {
        sequence.predecessor = sequence
            .predecessor
            .and_then(|old| index_map.get(&old).copied());
        sequence.successor = sequence
            .successor
            .iter()
            .filter_map(|old| index_map.get(old).copied())
            .collect();
    }

    *sequences = capped;
    index_map
}

pub(super) fn truncate_sequence_plans_preserving_order(
    sequences: &mut Vec<SequencePlan>,
    limit: usize,
) {
    if sequences.len() <= limit {
        return;
    }
    sequences.truncate(limit);
    for sequence in sequences {
        if sequence
            .predecessor
            .is_some_and(|predecessor| predecessor >= limit)
        {
            sequence.predecessor = None;
        }
        sequence.successor.retain(|successor| *successor < limit);
    }
}

pub(super) fn sort_sequence_plans_preserving_links(sequences: &mut Vec<SequencePlan>) {
    let mut ranked = (0..sequences.len()).collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        sequences[*right]
            .ranking_value
            .partial_cmp(&sequences[*left].ranking_value)
            .unwrap()
            .then(
                sequences[*left]
                    .steps
                    .len()
                    .cmp(&sequences[*right].steps.len()),
            )
            .then(
                symbolic_param_count(&sequences[*left].steps)
                    .cmp(&symbolic_param_count(&sequences[*right].steps)),
            )
            .then(
                sequence_signature(&sequences[*left].steps)
                    .cmp(&sequence_signature(&sequences[*right].steps)),
            )
            .then(
                sequences[*left]
                    .target_api
                    .cmp(&sequences[*right].target_api),
            )
    });

    let index_map = ranked
        .iter()
        .enumerate()
        .map(|(new_index, old_index)| (*old_index, new_index))
        .collect::<HashMap<_, _>>();
    let mut reordered = ranked
        .into_iter()
        .map(|old_index| sequences[old_index].clone())
        .collect::<Vec<_>>();

    for sequence in &mut reordered {
        sequence.predecessor = sequence
            .predecessor
            .and_then(|old| index_map.get(&old).copied());
        sequence.successor = sequence
            .successor
            .iter()
            .filter_map(|old| index_map.get(old).copied())
            .collect();
    }

    *sequences = reordered;
}

pub(super) fn extend_available_ty_variants<'tcx>(
    available: &mut HashSet<TyWrapper<'tcx>>,
    ty: Ty<'tcx>,
    tcx: TyCtxt<'tcx>,
) {
    available.insert(TyWrapper::from(ty));
    if matches!(ty.kind(), TyKind::Ref(..) | TyKind::RawPtr(..)) {
        return;
    }
    available.insert(TyWrapper::from(Ty::new_ref(
        tcx,
        tcx.lifetimes.re_erased,
        ty,
        ty::Mutability::Not,
    )));
    available.insert(TyWrapper::from(Ty::new_ref(
        tcx,
        tcx.lifetimes.re_erased,
        ty,
        ty::Mutability::Mut,
    )));
}

#[cfg(test)]
mod tests {
    use super::{
        earliest_mutator_insertion_index, required_fields_before_call,
        truncate_sequence_plans_preserving_order,
    };
    use crate::synth::{CallStep, SequencePlan};
    use crate::unsafe_analysis::FieldSet;

    #[test]
    fn indirect_mutator_expansion_stops_at_existing_prefix_head() {
        let sequence = SequencePlan {
            steps: vec![
                CallStep {
                    api_index: 0,
                    args: Vec::new(),
                },
                CallStep {
                    api_index: 1,
                    args: Vec::new(),
                },
                CallStep {
                    api_index: 2,
                    args: Vec::new(),
                },
                CallStep {
                    api_index: 3,
                    args: Vec::new(),
                },
            ],
            target_api: 3,
            unsafe_wrapper: String::new(),
            target_step: 3,
            predecessor: Some(0),
            successor: Vec::new(),
            latest_call: 1,
            mutated_param: Some(0),
            selected_unsafe_flow_call: None,
            selected_unsafe_flow_param: None,
            selected_unsafe_flow_fields: FieldSet::new(),
            ranking_value: 4.0,
            construction_cost: 4.0,
            prefix_distance: 0.0,
            target_to_wrapper_distance: 0.0,
            total_distance: 0.0,
            reuse_penalty: 0.0,
            queue_priority: 4.0,
        };

        assert_eq!(earliest_mutator_insertion_index(&sequence), 1);
    }

    #[test]
    fn indirect_mutator_expansion_uses_target_when_no_prefix_exists() {
        let sequence = SequencePlan {
            steps: vec![
                CallStep {
                    api_index: 0,
                    args: Vec::new(),
                },
                CallStep {
                    api_index: 1,
                    args: Vec::new(),
                },
            ],
            target_api: 1,
            unsafe_wrapper: String::new(),
            target_step: 1,
            predecessor: None,
            successor: Vec::new(),
            latest_call: 1,
            mutated_param: None,
            selected_unsafe_flow_call: None,
            selected_unsafe_flow_param: None,
            selected_unsafe_flow_fields: FieldSet::new(),
            ranking_value: 2.0,
            construction_cost: 2.0,
            prefix_distance: 0.0,
            target_to_wrapper_distance: 0.0,
            total_distance: 0.0,
            reuse_penalty: 0.0,
            queue_priority: 2.0,
        };

        assert_eq!(earliest_mutator_insertion_index(&sequence), 1);
    }

    #[test]
    fn required_fields_keep_overlap_when_required_and_provided_overlap_is_enabled() {
        let current_needed = FieldSet::from([vec![0]]);
        let provided = FieldSet::from([vec![0]]);
        assert_eq!(
            required_fields_before_call(&current_needed, &provided, true),
            current_needed,
        );
    }

    #[test]
    fn required_fields_drop_covered_fields_when_overlap_is_disabled() {
        let current_needed = FieldSet::from([vec![0]]);
        let provided = FieldSet::from([vec![0]]);

        assert!(required_fields_before_call(&current_needed, &provided, false).is_empty());
    }

    #[test]
    fn order_truncation_drops_links_to_removed_sequences() {
        let mut sequences = vec![
            test_sequence(0, None),
            test_sequence(1, Some(2)),
            test_sequence(2, None),
        ];
        sequences[0].successor = vec![1, 2];
        sequences[1].successor = vec![2];

        truncate_sequence_plans_preserving_order(&mut sequences, 2);

        assert_eq!(sequences.len(), 2);
        assert_eq!(sequences[0].successor, vec![1]);
        assert_eq!(sequences[1].predecessor, None);
        assert!(sequences[1].successor.is_empty());
    }

    fn test_sequence(target_api: usize, predecessor: Option<usize>) -> SequencePlan {
        SequencePlan {
            steps: Vec::new(),
            target_api,
            unsafe_wrapper: String::new(),
            target_step: 0,
            predecessor,
            successor: Vec::new(),
            latest_call: 0,
            mutated_param: None,
            selected_unsafe_flow_call: None,
            selected_unsafe_flow_param: None,
            selected_unsafe_flow_fields: FieldSet::new(),
            ranking_value: 0.0,
            construction_cost: 0.0,
            prefix_distance: 0.0,
            target_to_wrapper_distance: 0.0,
            total_distance: 0.0,
            reuse_penalty: 0.0,
            queue_priority: 0.0,
        }
    }
}
