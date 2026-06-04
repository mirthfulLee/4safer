mod helpers;
mod selection;
mod synthesis;

use helpers::*;
use selection::*;

use super::super::api::build_api_descriptors;
use super::super::{
    ApiDescriptor, ApiSafety, ArgPlan, BorrowMode, CallStep, CandidateEvaluation, MutatorIdentity,
    MutatorRoundSummary, OutputAdapter, SequenceCandidate, SequenceCandidateKind, SequencePlan,
    SynthesizedSuite, TargetSynthesisSummary, ValueSource, classify_api_safety,
    is_supported_callable, render_api_path, render_concrete_args,
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
const DEFAULT_BASIC_SEQUENCES_PER_TARGET: usize = 32;
const DEFAULT_EXPANDED_SEQUENCES_PER_TARGET: usize = 32;
const DEFAULT_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET: usize = 8;
const DEFAULT_WRAPPER_BUDGET_BASE: f32 = 160.0;
const DEFAULT_WRAPPER_BUDGET_SCALE: f32 = 64.0;
const MAX_BASIC_CONSTRUCTOR_SEQUENCES: usize = 16;
const MAX_MATERIALIZED_VARIANTS_PER_OBLIGATED_PARAM: usize = 8;
const MAX_TARGET_PLAN_CANDIDATES: usize = 32;
const MAX_CALL_PLAN_CANDIDATES: usize = 16;
const MAX_COMBINED_PREFIXES: usize = 32;
const MAX_MUTATOR_CANDIDATES_PER_PARAM: usize = 16;
const ALLOW_REQUIRED_PROVIDED_OVERLAP: bool = true;
const CONSTRUCTION_COST_WEIGHT: f32 = 1.0;
const PREFIX_DISTANCE_WEIGHT: f32 = 1.0;
const TARGET_DISTANCE_WEIGHT: f32 = 1.0;
const REUSE_PENALTY_WEIGHT: f32 = 1.0;
const DEPTH_SATURATION_WEIGHT: f32 = 0.25;
const STAGNATION_BUDGET_THRESHOLD: f32 = 0.5;
const STAGNATION_DEPTH_WEIGHT: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
struct PlannerCaps {
    mono_targets_per_generic_function: usize,
    targets_per_shape: usize,
    basic_sequences_per_target: usize,
    expanded_sequences_per_target: usize,
    multi_obligated_param_sequences_per_target: usize,
}

impl Default for PlannerCaps {
    fn default() -> Self {
        Self {
            mono_targets_per_generic_function: DEFAULT_MONO_TARGETS_PER_GENERIC_FUNCTION,
            targets_per_shape: DEFAULT_TARGETS_PER_SHAPE,
            basic_sequences_per_target: DEFAULT_BASIC_SEQUENCES_PER_TARGET,
            expanded_sequences_per_target: DEFAULT_EXPANDED_SEQUENCES_PER_TARGET,
            multi_obligated_param_sequences_per_target:
                DEFAULT_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET,
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
    wrapper_budget_base: f32,
    wrapper_budget_scale: f32,
    construction_cost_weight: f32,
    prefix_distance_weight: f32,
    target_distance_weight: f32,
    reuse_penalty_weight: f32,
    depth_penalty_weight: f32,
    depth_saturation_weight: f32,
    stagnation_budget_threshold: f32,
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
            mutation_round0_limit: 64,
            mutation_round1_limit: 32,
            mutation_round2_limit: 16,
            mutation_round3_limit: 4,
            wrapper_budget_base: DEFAULT_WRAPPER_BUDGET_BASE,
            wrapper_budget_scale: DEFAULT_WRAPPER_BUDGET_SCALE,
            construction_cost_weight: CONSTRUCTION_COST_WEIGHT,
            prefix_distance_weight: PREFIX_DISTANCE_WEIGHT,
            target_distance_weight: TARGET_DISTANCE_WEIGHT,
            reuse_penalty_weight: REUSE_PENALTY_WEIGHT,
            depth_penalty_weight: 0.25,
            depth_saturation_weight: DEPTH_SATURATION_WEIGHT,
            stagnation_budget_threshold: STAGNATION_BUDGET_THRESHOLD,
            stagnation_depth_weight: STAGNATION_DEPTH_WEIGHT,
        }
    }
}

impl PlannerCaps {
    fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            mono_targets_per_generic_function: env_usize(
                "VARIES_OD_MONO_TARGETS_PER_GENERIC_FUNCTION",
                defaults.mono_targets_per_generic_function,
            ),
            targets_per_shape: env_usize("VARIES_OD_TARGETS_PER_SHAPE", defaults.targets_per_shape),
            basic_sequences_per_target: env_usize(
                "VARIES_OD_BASIC_SEQUENCES_PER_TARGET",
                defaults.basic_sequences_per_target,
            ),
            expanded_sequences_per_target: env_usize(
                "VARIES_OD_EXPANDED_SEQUENCES_PER_TARGET",
                defaults.expanded_sequences_per_target,
            ),
            multi_obligated_param_sequences_per_target: env_usize(
                "VARIES_OD_MULTI_OBLIGATED_PARAM_SEQUENCES_PER_TARGET",
                defaults.multi_obligated_param_sequences_per_target,
            ),
        }
    }
}

impl PlannerTuning {
    fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            top_k_candidates: env_usize("VARIES_OD_TOP_K_CANDIDATES", defaults.top_k_candidates),
            max_basic_constructor_sequences: env_usize(
                "VARIES_OD_MAX_BASIC_CONSTRUCTOR_SEQUENCES",
                defaults.max_basic_constructor_sequences,
            ),
            max_materialized_variants_per_obligated_param: env_usize(
                "VARIES_OD_MAX_MATERIALIZED_VARIANTS_PER_OBLIGATED_PARAM",
                defaults.max_materialized_variants_per_obligated_param,
            ),
            max_target_plan_candidates: env_usize(
                "VARIES_OD_MAX_TARGET_PLAN_CANDIDATES",
                defaults.max_target_plan_candidates,
            ),
            max_call_plan_candidates: env_usize(
                "VARIES_OD_MAX_CALL_PLAN_CANDIDATES",
                defaults.max_call_plan_candidates,
            ),
            max_combined_prefixes: env_usize(
                "VARIES_OD_MAX_COMBINED_PREFIXES",
                defaults.max_combined_prefixes,
            ),
            max_mutator_candidates_per_param: env_usize(
                "VARIES_OD_MAX_MUTATOR_CANDIDATES_PER_PARAM",
                defaults.max_mutator_candidates_per_param,
            ),
            mutation_round0_limit: env_usize(
                "VARIES_OD_MUTATION_ROUND0_LIMIT",
                defaults.mutation_round0_limit,
            ),
            mutation_round1_limit: env_usize(
                "VARIES_OD_MUTATION_ROUND1_LIMIT",
                defaults.mutation_round1_limit,
            ),
            mutation_round2_limit: env_usize(
                "VARIES_OD_MUTATION_ROUND2_LIMIT",
                defaults.mutation_round2_limit,
            ),
            mutation_round3_limit: env_usize(
                "VARIES_OD_MUTATION_ROUND3_LIMIT",
                defaults.mutation_round3_limit,
            ),
            wrapper_budget_base: env_f32(
                "VARIES_OD_WRAPPER_BUDGET_BASE",
                defaults.wrapper_budget_base,
            ),
            wrapper_budget_scale: env_f32(
                "VARIES_OD_WRAPPER_BUDGET_SCALE",
                defaults.wrapper_budget_scale,
            ),
            construction_cost_weight: env_f32(
                "VARIES_OD_CONSTRUCTION_COST_WEIGHT",
                defaults.construction_cost_weight,
            ),
            prefix_distance_weight: env_f32(
                "VARIES_OD_PREFIX_DISTANCE_WEIGHT",
                defaults.prefix_distance_weight,
            ),
            target_distance_weight: env_f32(
                "VARIES_OD_TARGET_DISTANCE_WEIGHT",
                defaults.target_distance_weight,
            ),
            reuse_penalty_weight: env_f32(
                "VARIES_OD_REUSE_PENALTY_WEIGHT",
                defaults.reuse_penalty_weight,
            ),
            depth_penalty_weight: env_f32(
                "VARIES_OD_DEPTH_PENALTY_WEIGHT",
                defaults.depth_penalty_weight,
            ),
            depth_saturation_weight: env_f32(
                "VARIES_OD_DEPTH_SATURATION_WEIGHT",
                defaults.depth_saturation_weight,
            ),
            stagnation_budget_threshold: env_f32(
                "VARIES_OD_STAGNATION_BUDGET_THRESHOLD",
                defaults.stagnation_budget_threshold,
            ),
            stagnation_depth_weight: env_f32(
                "VARIES_OD_STAGNATION_DEPTH_WEIGHT",
                defaults.stagnation_depth_weight,
            ),
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

fn env_f32(name: &str, default: f32) -> f32 {
    match env::var(name) {
        Ok(value) => value
            .parse::<f32>()
            .unwrap_or_else(|error| panic!("{name} must be f32, got `{value}`: {error}")),
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
    reuse_penalty_enabled: bool,
    depth_penalty_enabled: bool,
    caps: PlannerCaps,
    tuning: PlannerTuning,
    relax_unbudgeted_filters: bool,
}

#[derive(Clone, Debug)]
struct ObligatedParamCandidatePath {
    obligated_param: usize,
    remaining_needed: FieldSet,
}

#[derive(Clone, Debug)]
struct MultiObligatedParamCandidatePath {
    obligated_params: Vec<ObligatedParamCandidatePath>,
    mutator_count: usize,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CallPlan {
    prefix: Vec<CallStep>,
    args: Vec<Option<ArgPlan>>,
}

#[derive(Clone, Debug)]
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
        reuse_penalty_enabled: bool,
        depth_penalty_enabled: bool,
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
            reuse_penalty_enabled,
            depth_penalty_enabled,
            caps: PlannerCaps::from_env(),
            tuning: PlannerTuning::from_env(),
            relax_unbudgeted_filters: false,
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
            self.synthesize_target_group(
                &unsafe_wrapper,
                &target_group,
                &mut sequences,
                &mut target_summaries,
            );
        }

        sort_sequence_plans_preserving_links(&mut sequences);

        SynthesizedSuite {
            apis: self.apis,
            sequences,
            target_summaries,
        }
    }

    pub(crate) fn synthesize_unbudgeted(mut self) -> SynthesizedSuite<'tcx> {
        self.relax_unbudgeted_filters = true;
        let target_groups = self.select_target_function_groups(self.collect_obligation_targets());
        info!(
            target: "foursafer::planner",
            "obligation-driven unbudgeted planner: {} unsafe wrappers after relaxed target selection",
            target_groups.len()
        );

        let mut sequences = Vec::new();
        let mut target_summaries = Vec::new();
        for (unsafe_wrapper, target_group) in target_groups {
            self.synthesize_target_group_unbudgeted(
                &unsafe_wrapper,
                &target_group,
                &mut sequences,
                &mut target_summaries,
            );
        }

        sort_sequence_plans_preserving_links(&mut sequences);

        SynthesizedSuite {
            apis: self.apis,
            sequences,
            target_summaries,
        }
    }
}
