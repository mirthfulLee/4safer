#!/usr/bin/env bash
set -euo pipefail

TIMEOUT=7200
JOBS="${JOBS:-4}"
TEST_JOBS="${TEST_JOBS:-2}"
MIRI_MAX_TESTS_PER_CRATE="${MIRI_MAX_TESTS_PER_CRATE:-5}"
ALT_SANITIZER="${ALT_SANITIZER:-memory}"
FUZZ_JOBS="${FUZZ_JOBS:-2}"
FUZZ_TIME_LIMIT="${FUZZ_TIME_LIMIT:-86400}"
FUZZ_TARGET_TIMEOUT="${FUZZ_TARGET_TIMEOUT:-87000}"
FUZZ_CRATE_JOBS="${FUZZ_CRATE_JOBS:-1}"
KANI_JOBS="${KANI_JOBS:-1}"
KANI_TIMEOUT="${KANI_TIMEOUT:-0}"
VARIES_KANI_DIR="${VARIES_KANI_DIR:-/data/huanli/projects/varies_root/varies_kani}"
RUN_IN_EXPERIMENT_CONTAINER="${RUN_IN_EXPERIMENT_CONTAINER:-scripts/run_in_experiment_container.sh}"
CONTAINER_VARIES_KANI_DIR="${CONTAINER_VARIES_KANI_DIR:-/opt/varies_kani}"

run_experiment_container() {
  if [[ "${DISABLE_EXPERIMENT_CONTAINER:-0}" == "1" || "${IN_CARGO_4SAFER_CONTAINER:-0}" == "1" ]]; then
    "$@"
  else
    "$RUN_IN_EXPERIMENT_CONTAINER" "$@"
  fi
}

if [[ "${DISABLE_EXPERIMENT_CONTAINER:-0}" == "1" ]]; then
  KANI_EXEC_VARIES_KANI_DIR="$VARIES_KANI_DIR"
else
  KANI_EXEC_VARIES_KANI_DIR="$CONTAINER_VARIES_KANI_DIR"
fi

if [[ -d /opt/anaconda3/lib ]]; then
  export LD_LIBRARY_PATH="/opt/anaconda3/lib:${LD_LIBRARY_PATH:-}"
fi

# 实验 1：使用 budgeted obligation-driven 一次分析同时生成所有 backend 目标。
python3 scripts/paper_experiments.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend all \
  --jobs "$JOBS" \
  --timeout "$TIMEOUT"

# 实验 2：obligation-driven 消融实验。每次只跑一个消融配置和一个 backend。
for config in \
  od_budget_full \
  od_no_reuse_penalty \
  od_no_depth_penalty \
  od_no_reuse_no_depth_penalty \
  od_unbudgeted
do
  run_experiment_container python3 scripts/paper_experiments.py \
    --experiment ablation \
    --config "$config" \
    --backend kani \
    --jobs "$JOBS" \
    --timeout "$TIMEOUT"
done

# 汇总已有运行结果，不重新生成。
python3 scripts/paper_experiments.py --aggregate-existing

# Backend 执行实验：读取上面的生成结果，逐 crate 执行生成的 backend 目标。
# kani backend 使用 varies_kani 的 cargo-4safer-kani 包装器和 ab-coverage 参数。
run_experiment_container python3 scripts/paper_backend_execution.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend kani \
  --kani-mode config \
  --kani-config scripts/config/kani-ab-coverage.json \
  --varies-kani-dir "$KANI_EXEC_VARIES_KANI_DIR" \
  --jobs "$KANI_JOBS" \
  --timeout "$KANI_TIMEOUT" \
  --force

# tests backend 默认用 ASan 执行，结果写入 tests-asan。
python3 scripts/paper_backend_execution.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend tests \
  --tests-runner asan \
  --jobs "$JOBS" \
  --timeout 1800 \
  --test-timeout 300 \
  --test-jobs "$TEST_JOBS" \
  --force

# 换一个 sanitizer 做对照，默认 MemorySanitizer，结果写入 tests-$ALT_SANITIZER。
python3 scripts/paper_backend_execution.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend tests \
  --tests-runner sanitizer \
  --sanitizer "$ALT_SANITIZER" \
  --jobs "$JOBS" \
  --timeout 1800 \
  --test-timeout 300 \
  --test-jobs "$TEST_JOBS" \
  --force

# tests backend 的 Miri 执行方式，结果写入 tests-miri，不覆盖 native tests。
python3 scripts/paper_backend_execution.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend tests \
  --tests-runner miri \
  --jobs "$JOBS" \
  --timeout 1800 \
  --test-timeout 300 \
  --test-jobs "$TEST_JOBS" \
  --max-tests-per-crate "$MIRI_MAX_TESTS_PER_CRATE"

# fuzz backend：使用 AFL 按所有 fuzz target 执行，单 target 最多 24h，检测到崩溃后可早停。
run_experiment_container python3 scripts/paper_backend_execution.py \
  --experiment obligation-budget \
  --config od_budget_m4_d0 \
  --backend fuzz \
  --fuzz-targets all \
  --jobs "$FUZZ_CRATE_JOBS" \
  --fuzz-jobs "$FUZZ_JOBS" \
  --fuzz-time-limit "$FUZZ_TIME_LIMIT" \
  --fuzz-target-timeout "$FUZZ_TARGET_TIMEOUT" \
  --fuzz-stop-on-crash \
  --force

# 汇总 backend 执行结果（tests/fuzz/kani）。
python3 scripts/paper_backend_execution.py --aggregate-existing
