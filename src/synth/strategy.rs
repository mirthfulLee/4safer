use super::obligation_driven;
use super::{SynthesisConfig, SynthesizedSuite};
use crate::rapx_graph::ApiDependencyGraph;
use crate::unsafe_analysis::ApiFieldFactsIndex;
use rustc_middle::ty::TyCtxt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceRankingMode {
    Ranked,
    GenerationOrder,
}

impl SequenceRankingMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ranked => "ranked",
            Self::GenerationOrder => "generation-order",
        }
    }

    pub(crate) fn from_disable_ranking(disable_ranking: bool) -> Self {
        if disable_ranking {
            Self::GenerationOrder
        } else {
            Self::Ranked
        }
    }
}

pub fn synthesize_suite<'tcx>(
    sequence_ranking_mode: SequenceRankingMode,
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    obligation_driven::synthesize_suite(
        sequence_ranking_mode,
        tcx,
        graph,
        field_facts_index,
        config,
    )
}
