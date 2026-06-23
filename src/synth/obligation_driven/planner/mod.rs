mod helpers;
mod selection;
mod synthesis;

use helpers::*;
use selection::*;

use super::super::api::build_api_descriptors;
use super::super::{
    ApiDescriptor, ApiSafety, ArgPlan, BorrowMode, CallStep, CandidateEvaluation, MutatorIdentity,
    MutatorRoundSummary, OutputAdapter, SequenceCandidate, SequencePlan, SynthesizedSuite,
    TargetSynthesisSummary, ValueSource, classify_api_safety, is_supported_callable,
    render_api_path, render_concrete_args,
};
use crate::rapx_graph::{ApiDependencyGraph, TyWrapper};
use crate::rapx_mono;
use crate::std_adapters::{
    is_pyo3_python_token_input, seed_value_ty, supports_direct_symbolic_input,
};
use crate::type_relations::{can_supply_to_input, input_requires_constructor_scan};
use crate::unsafe_analysis::{ApiFieldFactsIndex, FieldDistanceMap, FieldPath, FieldSet};
use log::{debug, info};
use rustc_middle::ty::{self, Ty, TyCtxt, TyKind};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::time::Instant;

const DEFAULT_TOP_K_CANDIDATES: usize = 8;
const DEFAULT_MONO_TARGETS_PER_GENERIC_FUNCTION: usize = 4;
const DEFAULT_TARGETS_PER_SHAPE: usize = 3;
const DEFAULT_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET: usize = 16;
const MAX_BASIC_CONSTRUCTOR_SEQUENCES: usize = 16;
const MAX_MATERIALIZED_VARIANTS_PER_OBLIGATED_PARAM: usize = 8;
const MAX_TARGET_PLAN_CANDIDATES: usize = 32;
const MAX_CALL_PLAN_CANDIDATES: usize = 128;
const MAX_COMBINED_PREFIXES: usize = 32;
const MAX_MUTATOR_CANDIDATES_PER_PARAM: usize = 16;
const ALLOW_REQUIRED_PROVIDED_OVERLAP: bool = true;
const R_FLOW: f32 = 0.5;
const COST_WEIGHT: f32 = 0.1;
const DIVERSITY_PENALTY_WEIGHT: f32 = 0.2;
const DEPTH_SATURATION_WEIGHT: f32 = 0.25;
const STAGNATION_SEQUENCE_THRESHOLD: f32 = 0.5;
const STAGNATION_DEPTH_WEIGHT: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
struct PlannerCaps {
    mono_targets_per_generic_function: usize,
    targets_per_shape: usize,
    floor_sequences_per_target: usize,
}

impl Default for PlannerCaps {
    fn default() -> Self {
        Self {
            mono_targets_per_generic_function: DEFAULT_MONO_TARGETS_PER_GENERIC_FUNCTION,
            targets_per_shape: DEFAULT_TARGETS_PER_SHAPE,
            floor_sequences_per_target: DEFAULT_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PlannerTuning {
    top_k_candidates: usize,
    max_basic_constructor_sequences: usize,
    max_materialized_variants_per_obligated_param: usize,
    max_target_plan_candidates: usize,
    max_call_plan_candidates: usize,
    max_combined_prefixes: usize,
    max_mutator_candidates_per_param: usize,
    mutation_round0_limit: usize,
    mutation_round1_limit: usize,
    mutation_round2_limit: usize,
    mutation_round3_limit: usize,
    r_flow: f32,
    cost_weight: f32,
    diversity_penalty_weight: f32,
    depth_saturation_weight: f32,
    stagnation_sequence_threshold: f32,
    stagnation_depth_weight: f32,
}

impl Default for PlannerTuning {
    fn default() -> Self {
        Self {
            top_k_candidates: DEFAULT_TOP_K_CANDIDATES,
            max_basic_constructor_sequences: MAX_BASIC_CONSTRUCTOR_SEQUENCES,
            max_materialized_variants_per_obligated_param:
                MAX_MATERIALIZED_VARIANTS_PER_OBLIGATED_PARAM,
            max_target_plan_candidates: MAX_TARGET_PLAN_CANDIDATES,
            max_call_plan_candidates: MAX_CALL_PLAN_CANDIDATES,
            max_combined_prefixes: MAX_COMBINED_PREFIXES,
            max_mutator_candidates_per_param: MAX_MUTATOR_CANDIDATES_PER_PARAM,
            mutation_round0_limit: 16,
            mutation_round1_limit: 8,
            mutation_round2_limit: 4,
            mutation_round3_limit: 2,
            r_flow: R_FLOW,
            cost_weight: COST_WEIGHT,
            diversity_penalty_weight: DIVERSITY_PENALTY_WEIGHT,
            depth_saturation_weight: DEPTH_SATURATION_WEIGHT,
            stagnation_sequence_threshold: STAGNATION_SEQUENCE_THRESHOLD,
            stagnation_depth_weight: STAGNATION_DEPTH_WEIGHT,
        }
    }
}

impl PlannerCaps {
    fn from_config(floor_sequences_per_target: usize) -> Self {
        let defaults = Self::default();
        Self {
            mono_targets_per_generic_function: defaults.mono_targets_per_generic_function,
            targets_per_shape: defaults.targets_per_shape,
            floor_sequences_per_target,
        }
    }
}

impl PlannerTuning {
    fn from_config(r_flow: f32, cost_weight: f32, diversity_penalty_weight: f32) -> Self {
        let defaults = Self::default();
        Self {
            top_k_candidates: defaults.top_k_candidates,
            max_basic_constructor_sequences: defaults.max_basic_constructor_sequences,
            max_materialized_variants_per_obligated_param: defaults
                .max_materialized_variants_per_obligated_param,
            max_target_plan_candidates: defaults.max_target_plan_candidates,
            max_call_plan_candidates: env_usize(
                "FOURSAFER_OD_MAX_CALL_PLAN_CANDIDATES",
                defaults.max_call_plan_candidates,
            ),
            max_combined_prefixes: defaults.max_combined_prefixes,
            max_mutator_candidates_per_param: defaults.max_mutator_candidates_per_param,
            mutation_round0_limit: defaults.mutation_round0_limit,
            mutation_round1_limit: defaults.mutation_round1_limit,
            mutation_round2_limit: defaults.mutation_round2_limit,
            mutation_round3_limit: defaults.mutation_round3_limit,
            r_flow,
            cost_weight,
            diversity_penalty_weight,
            depth_saturation_weight: defaults.depth_saturation_weight,
            stagnation_sequence_threshold: defaults.stagnation_sequence_threshold,
            stagnation_depth_weight: defaults.stagnation_depth_weight,
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .unwrap_or_else(|error| panic!("{name} must be usize, got `{value}`: {error}")),
        Err(_) => default,
    }
}

pub(crate) struct Planner<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    graph: &'a ApiDependencyGraph<'tcx>,
    field_facts_index: &'a ApiFieldFactsIndex,
    apis: Vec<ApiDescriptor<'tcx>>,
    constructors: HashMap<TyWrapper<'tcx>, Vec<usize>>,
    mutators: HashMap<TyWrapper<'tcx>, Vec<(usize, usize)>>,
    basic_cache: HashMap<usize, Vec<Vec<CallStep>>>,
    constructor_sequence_cache: HashMap<(usize, usize), Vec<Vec<CallStep>>>,
    call_plan_cache: HashMap<(usize, usize), Option<Vec<CallPlan>>>,
    generic_constructor_cache: HashMap<String, Vec<usize>>,
    target_materialization_rank_cache: HashMap<usize, TargetMaterializationRank>,
    max_depth: usize,
    max_mutators_per_target: usize,
    max_sequences: usize,
    max_sequences_per_wrapper: usize,
    max_sequences_per_target: usize,
    wrapper_target_max_distance: f32,
    reuse_penalty_enabled: bool,
    depth_penalty_enabled: bool,
    ranking_enabled: bool,
    caps: PlannerCaps,
    tuning: PlannerTuning,
}

#[derive(Clone, Debug)]
struct ObligatedParamCandidatePath {
    obligated_param: usize,
    remaining_needed: FieldSet,
}

#[derive(Clone, Debug)]
struct MultiObligatedParamCandidatePath {
    obligated_params: Vec<ObligatedParamCandidatePath>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CallPlan {
    prefix: Vec<CallStep>,
    args: Vec<Option<ArgPlan>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PartialCallPlan {
    prefix: Vec<CallStep>,
    args: Vec<Option<ArgPlan>>,
    next_param_index: usize,
}

#[derive(Clone, Debug)]
struct ObligatedParamSteps {
    obligated_param: usize,
    steps: Vec<CallStep>,
    obligated_source: ValueSource,
    mutator_count: usize,
    materialization_cost: f32,
}

#[derive(Clone, Debug)]
struct CombinedObligatedParamSteps {
    steps: Vec<CallStep>,
    obligated_sources: HashMap<usize, ValueSource>,
    mutator_count: usize,
    materialization_cost: f32,
}

#[derive(Clone, Debug)]
struct MutatorExpansionResult {
    candidates: Vec<SequenceCandidate>,
    depth_pruned_candidates: usize,
}

#[derive(Clone, Debug)]
struct ObligatedParamSelection {
    params: Vec<usize>,
    has_explicit_obligations: bool,
}

impl<'tcx, 'a> Planner<'tcx, 'a> {
    pub(crate) fn new(
        tcx: TyCtxt<'tcx>,
        graph: &'a ApiDependencyGraph<'tcx>,
        field_facts_index: &'a ApiFieldFactsIndex,
        max_depth: usize,
        max_mutators_per_target: usize,
        max_sequences: usize,
        max_sequences_per_wrapper: usize,
        max_sequences_per_target: usize,
        floor_sequences_per_target: usize,
        wrapper_target_max_distance: f32,
        r_flow: f32,
        cost_weight: f32,
        diversity_penalty_weight: f32,
        reuse_penalty_enabled: bool,
        depth_penalty_enabled: bool,
        ranking_enabled: bool,
    ) -> Self {
        let apis = build_api_descriptors(tcx, graph, field_facts_index);
        let mut constructors: HashMap<TyWrapper<'tcx>, Vec<usize>> = HashMap::new();
        let mut mutators: HashMap<TyWrapper<'tcx>, Vec<(usize, usize)>> = HashMap::new();

        for api in &apis {
            if !api.supported {
                continue;
            }

            if !is_unit_ty(api.value_output)
                && !api
                    .inputs
                    .iter()
                    .any(|input| normalize_value_ty(*input) == api.value_output_key)
            {
                constructors
                    .entry(api.value_output_key)
                    .or_default()
                    .push(api.index);
            }

            for (param_index, input) in api.inputs.iter().enumerate() {
                let TyKind::Ref(_, inner, ty::Mutability::Mut) = input.kind() else {
                    continue;
                };
                if call_provided_fields(api, param_index).is_empty() {
                    continue;
                }
                mutators
                    .entry(TyWrapper::from(*inner))
                    .or_default()
                    .push((api.index, param_index));
            }
        }

        Self {
            tcx,
            graph,
            field_facts_index,
            apis,
            constructors,
            mutators,
            basic_cache: HashMap::new(),
            constructor_sequence_cache: HashMap::new(),
            call_plan_cache: HashMap::new(),
            generic_constructor_cache: HashMap::new(),
            target_materialization_rank_cache: HashMap::new(),
            max_depth,
            max_mutators_per_target,
            max_sequences,
            max_sequences_per_wrapper,
            max_sequences_per_target,
            wrapper_target_max_distance,
            reuse_penalty_enabled,
            depth_penalty_enabled,
            ranking_enabled,
            caps: PlannerCaps::from_config(floor_sequences_per_target),
            tuning: PlannerTuning::from_config(r_flow, cost_weight, diversity_penalty_weight),
        }
    }

    pub(crate) fn synthesize(mut self) -> SynthesizedSuite<'tcx> {
        let target_groups = self.select_target_function_groups(self.collect_obligation_targets());
        info!(
            target: "foursafer::planner",
            "obligation-driven planner: {} ranked unsafe wrappers",
            target_groups.len()
        );

        // Phase 3: obligation-driven sequence synthesis.
        let mut sequences = Vec::new();
        let mut target_summaries = Vec::new();
        for (unsafe_wrapper, target_group) in target_groups {
            if self.sequence_limit_reached(sequences.len()) {
                info!(
                    target: "foursafer::planner",
                    "global suite sequence cap reached; skipping remaining target groups"
                );
                break;
            }
            self.synthesize_target_group(
                &unsafe_wrapper,
                &target_group,
                &mut sequences,
                &mut target_summaries,
            );
        }

        self.cap_and_sort_suite_sequences(&mut sequences);

        SynthesizedSuite {
            apis: self.apis,
            sequences,
            target_summaries,
        }
    }

    fn cap_and_sort_suite_sequences(&self, sequences: &mut Vec<SequencePlan>) {
        if !self.ranking_enabled {
            if self.max_sequences != 0 && sequences.len() > self.max_sequences {
                let uncapped_len = sequences.len();
                truncate_sequence_plans_preserving_order(sequences, self.max_sequences);
                info!(
                    target: "foursafer::planner",
                    "global suite sequence cap retained {} of {} sequences (limit={})",
                    sequences.len(),
                    uncapped_len,
                    self.max_sequences
                );
            }
            return;
        }

        if self.max_sequences == 0 {
            sort_sequence_plans_preserving_links(sequences);
            return;
        }

        let uncapped_len = sequences.len();
        cap_sequence_plans_preserving_predecessors(sequences, self.max_sequences);
        if sequences.len() < uncapped_len {
            info!(
                target: "foursafer::planner",
                "global suite sequence cap retained {} of {} sequences (limit={})",
                sequences.len(),
                uncapped_len,
                self.max_sequences
            );
        }
        sort_sequence_plans_preserving_links(sequences);
    }

    pub(super) fn sequence_limit_reached(&self, sequence_count: usize) -> bool {
        self.max_sequences != 0 && sequence_count >= self.max_sequences
    }

    pub(super) fn remaining_sequence_capacity(&self, sequence_count: usize) -> Option<usize> {
        (self.max_sequences != 0).then(|| self.max_sequences.saturating_sub(sequence_count))
    }

    pub(super) fn sequence_generation_limit(&self) -> Option<usize> {
        (self.max_sequences != 0).then_some(self.max_sequences)
    }
}
