mod planner;

use super::{SequenceRankingMode, SynthesisConfig, SynthesizedSuite};
use crate::rapx_graph::ApiDependencyGraph;
use crate::unsafe_analysis::ApiFieldFactsIndex;
use rustc_middle::ty::TyCtxt;

pub(super) fn synthesize_suite<'tcx>(
    sequence_ranking_mode: SequenceRankingMode,
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    // The planner embodies the unsafe-flow-guided pipeline:
    // 1. field-sensitive facts are consumed through `field_facts_index`,
    // 2. unsafe-relevance fields guide target/parameter selection,
    // 3. target-focused call sequences are synthesized from unsafe-flow entries.
    planner::Planner::new(
        tcx,
        graph,
        field_facts_index,
        config.max_depth,
        config.max_mutators_per_target,
        config.max_sequences,
        config.max_sequences_per_wrapper,
        config.max_sequences_per_target,
        config.floor_sequences_per_target,
        config.wrapper_target_max_distance,
        config.r_flow,
        config.w_c,
        config.w_p,
        config.reuse_penalty_enabled,
        config.depth_penalty_enabled,
        matches!(sequence_ranking_mode, SequenceRankingMode::Ranked),
    )
    .synthesize()
}
