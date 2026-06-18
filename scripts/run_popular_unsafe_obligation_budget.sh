#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_IN_EXPERIMENT_CONTAINER="${RUN_IN_EXPERIMENT_CONTAINER:-$REPO_DIR/scripts/run_in_experiment_container.sh}"
CONTAINER_REPO_DIR="${CONTAINER_REPO_DIR:-/workspace/4safer}"

if [[ "${DISABLE_EXPERIMENT_CONTAINER:-0}" != "1" && "${IN_CARGO_4SAFER_CONTAINER:-0}" != "1" ]]; then
  exec "$RUN_IN_EXPERIMENT_CONTAINER" bash "$CONTAINER_REPO_DIR/scripts/$(basename "$0")" "$@"
fi

cd "$REPO_DIR"

CSV="${CSV:-misc/popular_crates_with_unsafe.csv}"
DEFAULT_SOURCES_DIR="$(cd "$REPO_DIR/../.." && pwd)/downloads/popular_crates"
SOURCES_DIR="${SOURCES_DIR:-$DEFAULT_SOURCES_DIR}"
OUT_ROOT="${OUT_ROOT:-$REPO_DIR/tmp/popular-unsafe-obligation-sequence}"
GEN_OUT="${GEN_OUT:-$OUT_ROOT/generation}"
EXEC_OUT="${EXEC_OUT:-$OUT_ROOT/backend-execution}"

GEN_TIMEOUT="${GEN_TIMEOUT:-1800}"
EXEC_TIMEOUT="${EXEC_TIMEOUT:-1800}"
TEST_TIMEOUT="${TEST_TIMEOUT:-300}"
JOBS="${JOBS:-16}"
MSAN_JOBS="${MSAN_JOBS:-4}"
MIRI_JOBS="${MIRI_JOBS:-4}"
TEST_JOBS="${TEST_JOBS:-1}"
MSAN_MAX_TESTS_PER_CRATE="${MSAN_MAX_TESTS_PER_CRATE:-5}"
MIRI_MAX_TESTS_PER_CRATE="${MIRI_MAX_TESTS_PER_CRATE:-5}"
RUN_KANI="${RUN_KANI:-0}"
KANI_TIMEOUT="${KANI_TIMEOUT:-0}"
FORCE="${FORCE:-0}"

FORCE_ARGS=()
if [[ "$FORCE" == "1" ]]; then
  FORCE_ARGS=(--force)
fi

echo "[popular-unsafe] generation output: $GEN_OUT"
echo "[popular-unsafe] execution output:  $EXEC_OUT"
echo "[popular-unsafe] csv:               $CSV"
echo "[popular-unsafe] sources:           $SOURCES_DIR"

python3 scripts/paper_experiments.py \
  --csv "$CSV" \
  --sources-dir "$SOURCES_DIR" \
  --out "$GEN_OUT" \
  --skip-fetch \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --backend kani \
  --jobs "$JOBS" \
  --timeout "$GEN_TIMEOUT" \
  "${FORCE_ARGS[@]}"

python3 scripts/paper_experiments.py \
  --out "$GEN_OUT" \
  --aggregate-existing

if [[ "$RUN_KANI" == "1" ]]; then
  python3 scripts/paper_backend_execution.py \
    --generation-results "$GEN_OUT/all_results.csv" \
    --out "$EXEC_OUT" \
    --experiment obligation-sequence \
    --config od_sequence_m4_d0 \
    --backend kani \
    --kani-mode config \
    --kani-config scripts/config/kani-ab-coverage.json \
    --kani-dir "${FOURSAFER_KANI_DIR:-/opt/varies_kani}" \
    --jobs "${KANI_JOBS:-1}" \
    --timeout "$KANI_TIMEOUT" \
    "${FORCE_ARGS[@]}"
fi

python3 scripts/paper_backend_execution.py \
  --generation-results "$GEN_OUT/all_results.csv" \
  --out "$EXEC_OUT" \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --tests-runner sanitizer \
  --sanitizer memory \
  --jobs "$MSAN_JOBS" \
  --timeout "$EXEC_TIMEOUT" \
  --test-timeout "$TEST_TIMEOUT" \
  --test-jobs "$TEST_JOBS" \
  --max-tests-per-crate "$MSAN_MAX_TESTS_PER_CRATE" \
  "${FORCE_ARGS[@]}"

python3 scripts/paper_backend_execution.py \
  --generation-results "$GEN_OUT/all_results.csv" \
  --out "$EXEC_OUT" \
  --experiment obligation-sequence \
  --config od_sequence_m4_d0 \
  --backend tests \
  --tests-runner miri \
  --jobs "$MIRI_JOBS" \
  --timeout "$EXEC_TIMEOUT" \
  --test-timeout "$TEST_TIMEOUT" \
  --test-jobs "$TEST_JOBS" \
  --max-tests-per-crate "$MIRI_MAX_TESTS_PER_CRATE" \
  "${FORCE_ARGS[@]}"

python3 scripts/paper_backend_execution.py \
  --out "$EXEC_OUT" \
  --aggregate-existing

echo "[popular-unsafe] generation summary:   $GEN_OUT/summary.md"
echo "[popular-unsafe] execution summary:    $EXEC_OUT/summary.md"
echo "[popular-unsafe] unsoundness cases:    $EXEC_OUT/unsoundness_cases.md"
