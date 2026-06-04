use super::obligation_driven;
use super::{SynthesisConfig, SynthesizedSuite};
use crate::rapx_graph::ApiDependencyGraph;
use crate::unsafe_analysis::ApiFieldFactsIndex;
use rustc_middle::ty::TyCtxt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetMode {
    Limited,
    Unlimited,
}

impl BudgetMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Limited => "budgeted",
            Self::Unlimited => "unbudgeted",
        }
    }

    pub(crate) fn from_disable_limit(disable_limit: bool) -> Self {
        if disable_limit {
            Self::Unlimited
        } else {
            Self::Limited
        }
    }
}

pub fn synthesize_suite<'tcx>(
    budget_mode: BudgetMode,
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
    config: &SynthesisConfig,
) -> SynthesizedSuite<'tcx> {
    match budget_mode {
        BudgetMode::Limited => {
            obligation_driven::synthesize_suite(tcx, graph, field_facts_index, config)
        }
        BudgetMode::Unlimited => {
            obligation_driven::synthesize_suite_unbudgeted(tcx, graph, field_facts_index, config)
        }
    }
}
