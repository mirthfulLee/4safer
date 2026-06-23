mod api;
mod obligation_driven;
mod strategy;

use crate::rapx_graph::TyWrapper;
use crate::unsafe_analysis::{ApiFieldFacts, FieldSet};
use rustc_middle::ty::Ty;

#[derive(Clone, Debug)]
/// Captures the synthesized view of one callable API, including its resolved
/// signature, safety class, and output adaptation requirements.
pub struct ApiDescriptor<'tcx> {
    pub index: usize,
    pub path: String,
    pub target_family: String,
    pub instantiated_path: String,
    pub concrete_args: Vec<String>,
    pub is_mono: bool,
    pub inputs: Vec<Ty<'tcx>>,
    pub value_output: Ty<'tcx>,
    pub value_output_key: TyWrapper<'tcx>,
    pub output_adapter: OutputAdapter,
    pub api_safety: ApiSafety,
    pub field_facts: ApiFieldFacts,
    pub supported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Classifies how an API exposes unsafe behavior to the public surface.
pub enum ApiSafety {
    Safe,
    UnsafeFn,
    BuiltinUnsafe,
    UnsafeBlock,
    IndirectUnsafe,
}

impl ApiSafety {
    pub fn as_str(self) -> &'static str {
        match self {
            ApiSafety::Safe => "safe",
            ApiSafety::UnsafeFn => "unsafe_fn",
            ApiSafety::BuiltinUnsafe => "builtin_unsafe",
            ApiSafety::UnsafeBlock => "unsafe_block",
            ApiSafety::IndirectUnsafe => "indirect_unsafe",
        }
    }

    fn is_unsafe_fn(self) -> bool {
        matches!(self, ApiSafety::UnsafeFn)
    }

    pub fn is_obligation_target(self) -> bool {
        matches!(self, ApiSafety::UnsafeBlock | ApiSafety::IndirectUnsafe)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Indicates whether a call result must be unwrapped before later steps can
/// treat it as a produced value.
pub enum OutputAdapter {
    Plain,
    Option,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// Records how one sequence argument borrows or moves its source value.
pub enum BorrowMode {
    Move,
    Shared,
    Mutable,
    RawConst,
    RawMutable,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// Identifies whether an argument comes from a symbolic parameter or a prior
/// call step in the same sequence.
pub enum ValueSource {
    Param(usize),
    Step(usize),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// Plans how one argument is sourced and borrowed for a call step.
pub struct ArgPlan {
    pub source: ValueSource,
    pub borrow: BorrowMode,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// Represents one API invocation inside a synthesized sequence.
pub struct CallStep {
    pub api_index: usize,
    pub args: Vec<ArgPlan>,
}

#[derive(Clone, Debug)]
/// Describes one synthesized target-focused verification sequence and its
/// predecessor/successor metadata.
pub struct SequencePlan {
    pub steps: Vec<CallStep>,
    pub target_api: usize,
    pub unsafe_wrapper: String,
    pub target_step: usize,
    pub predecessor: Option<usize>,
    pub successor: Vec<usize>,
    pub latest_call: usize,
    pub mutated_param: Option<usize>,
    pub selected_obligation_call: Option<usize>,
    pub selected_obligation_param: Option<usize>,
    pub selected_obligation_fields: FieldSet,
    pub ranking_value: f32,
    pub construction_cost: f32,
    pub prefix_distance: f32,
    pub target_to_wrapper_distance: f32,
    pub total_distance: f32,
    pub reuse_penalty: f32,
    pub queue_priority: f32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// Identifies the mutator operation inserted by an expansion candidate.
pub struct MutatorIdentity {
    pub api_index: usize,
    pub mutated_param_index: usize,
}

#[derive(Clone, Copy, Debug)]
/// Records the priority components used for queue ordering.
pub struct CandidateEvaluation {
    pub construction_cost: f32,
    pub prefix_distance: f32,
    pub target_to_wrapper_distance: f32,
    pub total_distance: f32,
    pub priority_score: f32,
    pub reuse_penalty: f32,
    pub queue_priority: f32,
}

#[derive(Clone, Debug)]
/// Wraps one synthesized sequence together with the candidate metadata that
/// produced it.
pub struct SequenceCandidate {
    pub sequence: SequencePlan,
    pub construction_cost: f32,
    pub prefix_distance: f32,
    pub target_to_wrapper_distance: f32,
    pub mutator_identity: Option<MutatorIdentity>,
    pub expansion_depth: usize,
}

#[derive(Clone, Debug)]
/// Summarizes one mutator-expansion round for one obligation-driven target.
pub struct MutatorRoundSummary {
    pub round: usize,
    pub parent_sequences_considered: usize,
    pub raw_candidates: usize,
    pub duplicate_candidates: usize,
    pub depth_pruned_candidates: usize,
    pub selected_candidates: usize,
    pub retained_candidates: usize,
    pub stop_reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
/// Records why one synthesized target stopped growing, plus per-round stats.
pub struct TargetSynthesisSummary {
    pub target_api: usize,
    pub unsafe_wrapper: String,
    pub obligated_params: Vec<usize>,
    pub has_explicit_obligations: bool,
    pub base_sequences_before_cap: usize,
    pub base_sequences_after_cap: usize,
    pub base_materialization_failure: Option<&'static str>,
    pub wrapper_sequence_cap: usize,
    pub wrapper_sequences_accepted: usize,
    pub wrapper_target_distance: f32,
    pub wrapper_target_weight: f32,
    pub target_sequence_cap: usize,
    pub target_sequences_accepted: usize,
    pub rejected_candidates_wrapper_sequence_cap: usize,
    pub rejected_candidates_target_sequence_cap: usize,
    pub mutator_rounds: Vec<MutatorRoundSummary>,
    pub stop_reason: &'static str,
}

#[derive(Clone)]
/// Groups all supported APIs and the sequences synthesized for obligation-driven
/// verification.
pub struct SynthesizedSuite<'tcx> {
    pub apis: Vec<ApiDescriptor<'tcx>>,
    pub sequences: Vec<SequencePlan>,
    pub target_summaries: Vec<TargetSynthesisSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Configures synthesis depth and mutator expansion limits.
pub struct SynthesisConfig {
    pub max_depth: usize,
    pub max_mutators_per_target: usize,
    pub max_sequences: usize,
    pub max_sequences_per_wrapper: usize,
    pub max_sequences_per_target: usize,
    pub floor_sequences_per_target: usize,
    pub wrapper_target_max_distance: f32,
    pub r_flow: f32,
    pub w_c: f32,
    pub w_p: f32,
    pub reuse_penalty_enabled: bool,
    pub depth_penalty_enabled: bool,
}

impl<'tcx> ApiDescriptor<'tcx> {
    pub fn primary_unsafe_wrapper(&self) -> String {
        self.field_facts
            .unsafe_functions
            .iter()
            .find(|function| !is_std_internal_unsafe_function(function))
            .or_else(|| self.field_facts.unsafe_functions.first())
            .cloned()
            .unwrap_or_else(|| self.instantiated_path.clone())
    }
}

fn is_std_internal_unsafe_function(function: &str) -> bool {
    matches!(function.split("::").next(), Some("std" | "core" | "alloc"))
}

impl<'tcx> ApiDescriptor<'tcx> {
    pub fn unsafe_wrappers_without_std_internal(&self) -> Vec<String> {
        let filtered = self
            .field_facts
            .unsafe_functions
            .iter()
            .filter(|function| !is_std_internal_unsafe_function(function))
            .cloned()
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            self.field_facts.unsafe_functions.clone()
        } else {
            filtered
        }
    }
}

pub(crate) use api::{
    classify_api_safety, is_supported_callable, render_api_path, render_concrete_args,
};
pub use strategy::SequenceRankingMode;

/// Dispatch obligation-driven sequence synthesis with the selected ranking mode.
pub fn synthesize_suite<'tcx>(
    sequence_ranking_mode: SequenceRankingMode,
    tcx: rustc_middle::ty::TyCtxt<'tcx>,
    graph: &crate::rapx_graph::ApiDependencyGraph<'tcx>,
    field_facts_index: &crate::unsafe_analysis::ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    strategy::synthesize_suite(sequence_ranking_mode, tcx, graph, field_facts_index, config)
}
