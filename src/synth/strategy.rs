use super::obligation_driven;
use super::{SynthesisConfig, SynthesizedSuite};
use crate::rapx_graph::ApiDependencyGraph;
use crate::unsafe_analysis::ApiFieldFactsIndex;
use rustc_middle::ty::TyCtxt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceLimitMode {
    Limited,
    Unlimited,
}

impl SequenceLimitMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Limited => "sequence-limited",
            Self::Unlimited => "sequence-unlimited",
        }
    }

    pub(crate) fn from_disable_sequence_limits(disable_sequence_limits: bool) -> Self {
        if disable_sequence_limits {
            Self::Unlimited
        } else {
            Self::Limited
        }
    }
}

pub fn synthesize_suite<'tcx>(
    sequence_limit_mode: SequenceLimitMode,
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    match sequence_limit_mode {
        SequenceLimitMode::Limited => {
            obligation_driven::synthesize_suite(tcx, graph, field_facts_index, config)
        }
        SequenceLimitMode::Unlimited => {
            obligation_driven::synthesize_suite_unlimited(tcx, graph, field_facts_index, config)
        }
    }
}
