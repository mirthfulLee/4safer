use crate::backend::{BackendRenderContext, write_suite};
use crate::cli::FourSaferArgs;
use crate::rapx_graph::{ApiDependencyGraph, Config};
use crate::synth::{
    SequenceRankingMode, SynthesisConfig, classify_api_safety, is_supported_callable,
    render_api_path, synthesize_suite,
};
use crate::unsafe_analysis::{ApiFieldFactsIndex, analyze_public_unsafe_surfaces};
use crate::workspace::discover_target_for_analysis;
use anyhow::{Context, Result};
use log::{debug, info};
use rustc_middle::ty::{self, TyCtxt};
use std::collections::{BTreeSet, HashSet};
use std::env;
use std::time::Instant;

pub struct AnalysisSummary {
    pub total_api_count: usize,
    pub mono_api_count: usize,
    pub supported_api_count: usize,
    pub pre_mono_target_api_count: usize,
    pub target_api_count: usize,
    pub pre_mono_synthesized_target_api_count: usize,
    pub synthesized_target_api_count: usize,
    pub sequence_count: usize,
}

pub fn run_pipeline<'tcx>(tcx: TyCtxt<'tcx>, cli: &FourSaferArgs) -> Result<AnalysisSummary> {
    let cwd = env::current_dir().context("failed to resolve current directory in wrapper mode")?;
    let target = discover_target_for_analysis(&cwd, cli.package.as_deref())?;

    let pipeline_start = Instant::now();
    info!(target: "foursafer::pipeline", "building API dependency graph");
    let field_facts_start = Instant::now();
    let field_facts_index = analyze_public_unsafe_surfaces(tcx);
    info!(
        target: "foursafer::pipeline",
        "unsafe surface analysis completed in {:.2?}",
        field_facts_start.elapsed()
    );
    let graph_start = Instant::now();
    let mut graph = ApiDependencyGraph::new(tcx);
    graph.build(Config {
        pub_only: true,
        resolve_generic: true,
        ignore_const_generic: true,
    });
    info!(
        target: "foursafer::pipeline",
        "api dependency graph built in {:.2?} with {} api instances",
        graph_start.elapsed(),
        graph.num_api()
    );

    debug!(target: "foursafer::pipeline", "graph contains {} api instances after generic resolution", graph.num_api());
    info!(target: "foursafer::pipeline", "planning verification sequences");
    let synth_start = Instant::now();
    let sequence_ranking_mode = SequenceRankingMode::from_disable_ranking(cli.disable_ranking);
    let suite = synthesize_suite(
        sequence_ranking_mode,
        tcx,
        &graph,
        &field_facts_index,
        &SynthesisConfig {
            max_depth: cli.max_depth,
            max_mutators_per_target: cli.max_mutators_per_target,
            max_sequences: cli.max_sequences,
            max_sequences_per_wrapper: cli.max_sequences_per_wrapper,
            max_sequences_per_target: cli.max_sequences_per_target,
            floor_sequences_per_target: cli.floor_sequences_per_target,
            wrapper_target_max_distance: cli.wrapper_target_max_distance,
            r_flow: cli.r_flow,
            w_c: cli.w_c,
            w_p: cli.w_p,
            reuse_penalty_enabled: !cli.disable_reuse_penalty,
            depth_penalty_enabled: !cli.disable_depth_penalty,
        },
    );
    info!(
        target: "foursafer::pipeline",
        "sequence planning completed in {:.2?}",
        synth_start.elapsed()
    );
    let total_api_count = suite.apis.len();
    let mono_api_count = suite.apis.iter().filter(|api| api.is_mono).count();
    let supported_api_count = suite.apis.iter().filter(|api| api.supported).count();
    let pre_mono_target_api_count =
        pre_mono_obligation_target_api_count(tcx, &graph, &field_facts_index);
    let target_api_count = obligation_target_api_count(&suite);
    let synthesized_target_api_indices = suite
        .sequences
        .iter()
        .map(|sequence| sequence.target_api)
        .collect::<BTreeSet<_>>();
    let pre_mono_synthesized_target_api_count = synthesized_target_api_indices
        .iter()
        .map(|api_index| graph.api_at(*api_index).0)
        .collect::<HashSet<_>>()
        .len();
    let synthesized_target_api_count = synthesized_target_api_indices.len();
    let sequence_count = suite.sequences.len();

    debug!(
        target: "foursafer::pipeline",
        "suite stats: total_api_instances={}, mono_instances={}, supported_apis={}, target_instances={}, synthesized_target_instances={}, sequences={}",
        total_api_count,
        mono_api_count,
        supported_api_count,
        target_api_count,
        synthesized_target_api_count,
        sequence_count
    );
    info!(target: "foursafer::pipeline", "rendering generated harness crate(s)");
    let render_start = Instant::now();

    for backend in &cli.backends {
        let backend_harness_dir = cli.backend_harness_dir(&cwd, *backend);
        write_suite(
            *backend,
            BackendRenderContext {
                tcx,
                graph: &graph,
                target: &target,
                harness_dir: &backend_harness_dir,
                suite: &suite,
                cli,
            },
        )?;
    }

    info!(
        target: "foursafer::pipeline",
        "harness emission complete in {:.2?} (pipeline total {:.2?})",
        render_start.elapsed(),
        pipeline_start.elapsed()
    );

    Ok(AnalysisSummary {
        total_api_count,
        mono_api_count,
        supported_api_count,
        pre_mono_target_api_count,
        target_api_count,
        pre_mono_synthesized_target_api_count,
        synthesized_target_api_count,
        sequence_count,
    })
}

fn pre_mono_obligation_target_api_count<'tcx>(
    tcx: TyCtxt<'tcx>,
    graph: &ApiDependencyGraph<'tcx>,
    field_facts_index: &ApiFieldFactsIndex,
) -> usize {
    graph
        .all_apis()
        .iter()
        .filter(|def_id| {
            let def_id = **def_id;
            let args = ty::GenericArgs::identity_for_item(tcx, def_id);
            let path = render_api_path(tcx, graph, def_id, args);
            let api_safety = classify_api_safety(def_id, tcx, field_facts_index);
            is_supported_callable(tcx, graph, def_id, args, &path, api_safety)
                && api_safety.is_obligation_target()
        })
        .count()
}

fn obligation_target_api_count(suite: &crate::synth::SynthesizedSuite<'_>) -> usize {
    suite
        .apis
        .iter()
        .filter(|api| api.supported && api.api_safety.is_obligation_target())
        .count()
}
