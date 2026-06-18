use super::*;
use std::collections::BTreeMap;

impl<'tcx, 'a> Planner<'tcx, 'a> {
    /// Collect all API instances that are valid obligation-driven targets
    /// before representative-shape capping.
    pub(super) fn collect_obligation_targets(&self) -> Vec<usize> {
        self.apis
            .iter()
            .filter(|api| api.supported && api.api_safety.is_obligation_target())
            .map(|api| api.index)
            .collect()
    }

    /// Group target API instances by their primary unsafe wrapper and retain a
    /// representative subset per function/shape family.
    pub(super) fn select_target_function_groups(
        &mut self,
        target_indices: Vec<usize>,
    ) -> Vec<(String, Vec<usize>)> {
        self.target_materialization_rank_cache.clear();
        self.rank_target_function_groups(target_indices)
    }

    fn rank_target_function_groups(
        &mut self,
        target_indices: Vec<usize>,
    ) -> Vec<(String, Vec<usize>)> {
        let mut grouped = BTreeMap::<String, Vec<usize>>::new();
        for api_index in target_indices {
            let api = &self.apis[api_index];
            grouped
                .entry(api.primary_unsafe_wrapper())
                .or_default()
                .push(api_index);
        }

        let mut groups = grouped.into_iter().collect::<Vec<_>>();
        self.populate_target_materialization_rank_cache(&groups);
        groups.sort_by(|left, right| {
            let left_rank = self.cached_group_materialization_rank(&left.1);
            let right_rank = self.cached_group_materialization_rank(&right.1);
            left_rank
                .cmp(&right_rank)
                .then_with(|| right.1.len().cmp(&left.1.len()))
                .then_with(|| left.0.cmp(&right.0))
        });
        for (_, api_indices) in &mut groups {
            api_indices.sort_unstable();
            api_indices.dedup();
            if !self.relax_sequence_limit_filters {
                *api_indices = self.select_representative_targets(api_indices);
            }
        }
        groups
    }

    fn select_representative_targets(&self, api_indices: &[usize]) -> Vec<usize> {
        let mut selected = Vec::new();
        let mut mono_groups = BTreeMap::<&str, Vec<usize>>::new();

        for &api_index in api_indices {
            let api = &self.apis[api_index];
            if api.is_mono {
                mono_groups
                    .entry(&api.target_family)
                    .or_default()
                    .push(api_index);
            } else {
                selected.push(api_index);
            }
        }

        for mut group in mono_groups.into_values() {
            if group.len() <= self.caps.mono_targets_per_generic_function {
                selected.append(&mut group);
                continue;
            }
            selected.extend(self.select_representative_mono_targets(&group));
        }

        let mut shape_groups = BTreeMap::<String, Vec<usize>>::new();
        for api_index in selected {
            shape_groups
                .entry(self.target_shape_key(api_index))
                .or_default()
                .push(api_index);
        }

        let mut capped = Vec::new();
        for mut group in shape_groups.into_values() {
            group.sort_by(|left, right| {
                self.cached_target_materialization_rank(*left)
                    .cmp(&self.cached_target_materialization_rank(*right))
                    .then_with(|| self.apis[*left].path.cmp(&self.apis[*right].path))
            });
            if group.len() <= self.caps.targets_per_shape {
                capped.append(&mut group);
            } else {
                capped.extend(self.select_representative_mono_targets(&group));
            }
        }

        capped.sort_unstable();
        capped
    }

    fn select_representative_mono_targets(&self, api_indices: &[usize]) -> Vec<usize> {
        let mut remaining = api_indices.to_vec();
        remaining.sort_by(|&left, &right| {
            self.cached_target_materialization_rank(left)
                .cmp(&self.cached_target_materialization_rank(right))
                .then_with(|| {
                    mono_target_priority(&self.apis[left])
                        .cmp(&mono_target_priority(&self.apis[right]))
                })
                .then_with(|| {
                    mono_target_tiebreak(&self.apis[left])
                        .cmp(&mono_target_tiebreak(&self.apis[right]))
                })
        });

        let first = remaining.remove(0);
        let mut selected = vec![first];

        while !remaining.is_empty() && selected.len() < self.caps.mono_targets_per_generic_function
        {
            let (best_pos, _) = remaining
                .iter()
                .enumerate()
                .map(|(pos, candidate)| {
                    let diversity = selected
                        .iter()
                        .map(|selected_api| {
                            mono_target_distance(&self.apis[*candidate], &self.apis[*selected_api])
                        })
                        .min()
                        .unwrap_or(0);
                    (pos, diversity)
                })
                .min_by(|(left_pos, left_diversity), (right_pos, right_diversity)| {
                    let left_priority = mono_target_priority(&self.apis[remaining[*left_pos]]);
                    let right_priority = mono_target_priority(&self.apis[remaining[*right_pos]]);
                    self.cached_target_materialization_rank(remaining[*left_pos])
                        .cmp(&self.cached_target_materialization_rank(remaining[*right_pos]))
                        .then_with(|| left_priority.cmp(&right_priority))
                        .then_with(|| right_diversity.cmp(left_diversity))
                        .then_with(|| {
                            mono_target_tiebreak(&self.apis[remaining[*left_pos]])
                                .cmp(&mono_target_tiebreak(&self.apis[remaining[*right_pos]]))
                        })
                })
                .expect("remaining mono target candidates should be non-empty");
            selected.push(remaining.remove(best_pos));
        }

        selected
    }

    fn cached_group_materialization_rank(
        &self,
        api_indices: &[usize],
    ) -> TargetMaterializationRank {
        api_indices
            .iter()
            .map(|api_index| self.cached_target_materialization_rank(*api_index))
            .min()
            .unwrap_or_default()
    }

    fn cached_target_materialization_rank(&self, api_index: usize) -> TargetMaterializationRank {
        self.target_materialization_rank_cache
            .get(&api_index)
            .cloned()
            .expect("target materialization rank cache should be populated before ranking")
    }

    fn target_materialization_rank(&self, api_index: usize) -> TargetMaterializationRank {
        let api = &self.apis[api_index];
        let obligated = select_obligated_params(api)
            .params
            .into_iter()
            .collect::<HashSet<_>>();
        let mut unsupported_inputs = 0usize;
        let mut constructor_scan_inputs = 0usize;
        let mut constructor_inputs = 0usize;
        let mut symbolic_inputs = 0usize;
        let mut context_inputs = 0usize;

        for (param_index, input) in api.inputs.iter().enumerate() {
            let required_fields = target_required_fields_for_param(api, param_index);
            if is_pyo3_python_token_input(self.tcx, *input) {
                context_inputs += 1;
                continue;
            }
            if obligated.contains(&param_index) {
                if supports_direct_symbolic_input(self.tcx, *input) {
                    symbolic_inputs += 1;
                    continue;
                }
            } else if prefers_direct_symbolic_input(self.tcx, *input) {
                symbolic_inputs += 1;
                continue;
            }

            match self.cheap_constructor_support_for_input(*input, &required_fields) {
                CheapConstructorSupport::Known => constructor_inputs += 1,
                CheapConstructorSupport::NeedsScan => constructor_scan_inputs += 1,
                CheapConstructorSupport::Missing => unsupported_inputs += 1,
            }
        }

        TargetMaterializationRank {
            unsupported_inputs,
            constructor_scan_inputs,
            constructor_inputs,
            input_count: api.inputs.len(),
            indirect_target: usize::from(api.api_safety != ApiSafety::UnsafeBlock),
            context_inputs,
            symbolic_inputs,
        }
    }

    fn cheap_constructor_support_for_input(
        &self,
        input: Ty<'tcx>,
        required_fields: &FieldSet,
    ) -> CheapConstructorSupport {
        let key = normalize_value_ty(input);
        let candidates = self.constructors.get(&key).cloned().unwrap_or_default();
        if self.any_constructor_candidate_can_supply(candidates, input, required_fields) {
            return CheapConstructorSupport::Known;
        }
        if input_requires_constructor_scan(input) {
            return CheapConstructorSupport::NeedsScan;
        }
        CheapConstructorSupport::Missing
    }

    fn any_constructor_candidate_can_supply(
        &self,
        candidates: Vec<usize>,
        input: Ty<'tcx>,
        required_fields: &FieldSet,
    ) -> bool {
        candidates.into_iter().any(|api_index| {
            can_supply_to_input(self.tcx, self.apis[api_index].value_output, input)
                && (required_fields.is_empty()
                    || self.apis[api_index]
                        .field_facts
                        .output_overlaps_relevant(required_fields))
        })
    }

    fn populate_target_materialization_rank_cache(&mut self, groups: &[(String, Vec<usize>)]) {
        let api_indices = groups
            .iter()
            .flat_map(|(_, api_indices)| api_indices.iter().copied())
            .collect::<Vec<_>>();
        for api_index in api_indices {
            if self
                .target_materialization_rank_cache
                .contains_key(&api_index)
            {
                continue;
            }
            let rank = self.target_materialization_rank(api_index);
            self.target_materialization_rank_cache
                .insert(api_index, rank);
        }
    }

    fn target_shape_key(&self, api_index: usize) -> String {
        let api = &self.apis[api_index];
        let receiver_shape = api
            .inputs
            .first()
            .map(|ty| self.receiver_shape(*ty))
            .unwrap_or_else(|| "_".to_owned());
        let borrow_shape = api
            .inputs
            .iter()
            .map(|input| match input.kind() {
                TyKind::Ref(_, _, ty::Mutability::Mut) => "&mut",
                TyKind::Ref(_, _, ty::Mutability::Not) => "&",
                TyKind::RawPtr(_, ty::Mutability::Mut) => "*mut",
                TyKind::RawPtr(_, ty::Mutability::Not) => "*const",
                _ => "val",
            })
            .collect::<Vec<_>>()
            .join(",");
        let obligation_shape = api
            .field_facts
            .obligation_inputs
            .iter()
            .map(|fields| fields.len().to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{receiver_shape}|{}|{borrow_shape}|{}|{obligation_shape}",
            api.inputs.len(),
            api.output_adapter as u8
        )
    }

    fn receiver_shape(&self, ty: Ty<'tcx>) -> String {
        let value_ty = ty.peel_refs();
        match value_ty.kind() {
            TyKind::Adt(def, _) => self.tcx.def_path_str(def.did()),
            TyKind::Tuple(items) => format!("tuple{}", items.len()),
            TyKind::Slice(_) => "slice".to_owned(),
            TyKind::Str => "str".to_owned(),
            _ => self.tcx.erase_and_anonymize_regions(value_ty).to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct TargetMaterializationRank {
    unsupported_inputs: usize,
    constructor_scan_inputs: usize,
    constructor_inputs: usize,
    input_count: usize,
    indirect_target: usize,
    context_inputs: usize,
    symbolic_inputs: usize,
}

enum CheapConstructorSupport {
    Known,
    NeedsScan,
    Missing,
}

/// Select the parameters whose state must be materialized or repaired before
/// the target call can serve as a verification target.
pub(super) fn select_obligated_params(api: &ApiDescriptor<'_>) -> ObligatedParamSelection {
    let has_explicit_obligations = api
        .inputs
        .iter()
        .enumerate()
        .any(|(index, _input)| has_explicit_target_obligations(api, index));
    if matches!(api.api_safety, ApiSafety::UnsafeBlock) {
        let mut params = (0..api.inputs.len()).collect::<Vec<_>>();
        params.sort_by_key(|index| {
            usize::from(!matches!(
                api.inputs[*index].kind(),
                TyKind::Ref(_, _, ty::Mutability::Mut)
            ))
        });
        return ObligatedParamSelection {
            params,
            has_explicit_obligations,
        };
    }

    let mut params = api
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(index, _input)| {
            let obligations = initial_target_obligations(api, index);
            (!obligations.is_empty()).then_some(index)
        })
        .collect::<Vec<_>>();
    params.sort_by_key(|index| {
        (
            usize::from(initial_target_obligations(api, *index).is_empty()),
            usize::from(!matches!(
                api.inputs[*index].kind(),
                TyKind::Ref(_, _, ty::Mutability::Mut)
            )),
        )
    });
    if params.is_empty() {
        params = (0..api.inputs.len()).collect();
        params.sort_by_key(|index| {
            usize::from(!matches!(
                api.inputs[*index].kind(),
                TyKind::Ref(_, _, ty::Mutability::Mut)
            ))
        });
    }
    ObligatedParamSelection {
        params,
        has_explicit_obligations,
    }
}

pub(super) fn has_explicit_target_obligations(api: &ApiDescriptor<'_>, param_index: usize) -> bool {
    api.field_facts
        .obligation_inputs
        .get(param_index)
        .is_some_and(|fields| !fields.is_empty())
}

pub(super) fn mono_target_priority(api: &ApiDescriptor<'_>) -> (usize, usize, usize) {
    (
        usize::from(api.api_safety != ApiSafety::UnsafeBlock),
        usize::from(api.field_facts.obligation_inputs.is_empty()),
        api.concrete_args
            .iter()
            .map(|arg| canonical_symbolic_arg_rank(arg))
            .sum(),
    )
}

fn mono_target_tiebreak<'a, 'tcx>(api: &'a ApiDescriptor<'tcx>) -> (usize, &'a str) {
    (
        api.concrete_args.iter().map(|arg| arg.len()).sum(),
        api.instantiated_path.as_str(),
    )
}

pub(super) fn mono_target_type_complexity<'tcx>(api: &ApiDescriptor<'tcx>) -> usize {
    let input_complexity = api
        .inputs
        .iter()
        .map(|ty| ty_complexity(*ty))
        .max()
        .unwrap_or(1);
    let output_complexity = ty_complexity(api.value_output);
    input_complexity.max(output_complexity)
}

fn ty_complexity<'tcx>(ty: Ty<'tcx>) -> usize {
    match ty.peel_refs().kind() {
        TyKind::Ref(_, inner, _) | TyKind::Array(inner, _) | TyKind::Slice(inner) => {
            ty_complexity(*inner) + 1
        }
        TyKind::Tuple(tys) => tys.iter().fold(0, |max, ty| max.max(ty_complexity(ty))) + 1,
        TyKind::Adt(_, args) => {
            args.iter().fold(0, |max, arg| {
                max.max(arg.as_type().map(ty_complexity).unwrap_or(0))
            }) + 1
        }
        _ => 1,
    }
}

fn canonical_symbolic_arg_rank(arg: &str) -> usize {
    let trimmed = arg.trim();
    let (borrow_penalty, value) = strip_borrow_prefix(trimmed);
    borrow_penalty + canonical_symbolic_value_rank(value)
}

fn strip_borrow_prefix(arg: &str) -> (usize, &str) {
    if let Some(rest) = arg.strip_prefix("&mut ") {
        return (24, rest);
    }
    if let Some(rest) = arg.strip_prefix('&') {
        return (16, rest);
    }
    (0, arg)
}

fn canonical_symbolic_value_rank(value: &str) -> usize {
    if value == "std::string::String" {
        return 0;
    }
    if value == "f32" {
        return 1;
    }
    if value == "u8" {
        return 2;
    }
    if is_local_concrete_arg(value) {
        return 2;
    }
    if value == "::std::vec::Vec::<u8>" || value == "std::vec::Vec::<u8>" {
        return 3;
    }
    if value == "std::io::Cursor::<::std::vec::Vec::<u8>>"
        || value == "std::io::Cursor::<std::vec::Vec::<u8>>"
    {
        return 4;
    }
    if matches!(
        value,
        "bool"
            | "char"
            | "f64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "isize"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
    ) {
        return 5;
    }
    if value == "std::io::Empty" || value == "std::io::Sink" || value == "std::io::Repeat" {
        return 6;
    }
    8
}

fn is_local_concrete_arg(value: &str) -> bool {
    value.contains("::") && !value.starts_with("std::") && !value.starts_with("::std::")
}

pub(super) fn mono_target_distance(left: &ApiDescriptor<'_>, right: &ApiDescriptor<'_>) -> usize {
    let shared_len = left.concrete_args.len().min(right.concrete_args.len());
    let mut distance = left.concrete_args.len().abs_diff(right.concrete_args.len());
    for index in 0..shared_len {
        if left.concrete_args[index] != right.concrete_args[index] {
            distance += 1;
        }
    }
    distance
}
