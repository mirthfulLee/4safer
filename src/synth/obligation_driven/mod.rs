mod planner;

use super::{SynthesisConfig, SynthesizedSuite};
use crate::rapx_graph::ApiDependencyGraph;
use crate::unsafe_analysis::ApiFieldFactsIndex;
use rustc_middle::ty::TyCtxt;

pub(super) fn synthesize_suite<'tcx>(
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    // The planner embodies the obligation-driven pipeline:
    // 1. field-sensitive facts are consumed through `field_facts_index`,
    // 2. unsafe obligations are reflected in target/parameter selection,
    // 3. target-focused call sequences are synthesized under those obligations.
    planner::Planner::new(
        tcx,
        graph,
        field_facts_index,
        config.max_depth,
        config.max_mutators_per_target,
        config.max_sequences,
        config.max_sequences_per_wrapper,
        config.max_sequences_per_target,
        config.max_basic_sequences_per_target,
        config.reuse_penalty_enabled,
        config.depth_penalty_enabled,
    )
    .synthesize()
}

pub(super) fn synthesize_suite_unlimited<'tcx>(
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    planner::Planner::new(
        tcx,
        graph,
        field_facts_index,
        config.max_depth,
        config.max_mutators_per_target,
        config.max_sequences,
        config.max_sequences_per_wrapper,
        config.max_sequences_per_target,
        config.max_basic_sequences_per_target,
        config.reuse_penalty_enabled,
        config.depth_penalty_enabled,
    )
    .synthesize_unlimited()
}
