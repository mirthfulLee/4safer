#!/usr/bin/env bash
set -euo pipefail

IMAGE="${IMAGE:-cargo-4safer-experiments:latest}"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_KANI_HOST_DIR="$REPO_DIR/tmp/paper-experiments/tools/kani"
KANI_HOST_DIR="${KANI_HOST_DIR:-$DEFAULT_KANI_HOST_DIR}"
DOCKERFILE="${DOCKERFILE:-Dockerfile.experiments}"

if [[ ! -d "$KANI_HOST_DIR" ]]; then
  echo "missing Kani checkout: $KANI_HOST_DIR" >&2
  exit 2
fi

if [[ ! -x "$KANI_HOST_DIR/target/kani/bin/cargo-kani" ]]; then
  echo "missing prebuilt cargo-kani binary under $KANI_HOST_DIR/target/kani/bin" >&2
  echo "build Kani first, or point KANI_HOST_DIR at a built checkout" >&2
  exit 2
fi

rustup toolchain list | grep -q 'nightly-2026-03-26-x86_64-unknown-linux-gnu' || {
  echo "missing host rustup toolchain: nightly-2026-03-26-x86_64-unknown-linux-gnu" >&2
  exit 2
}
rustup toolchain list | grep -q 'nightly-2025-10-23-x86_64-unknown-linux-gnu' || {
  echo "missing host rustup toolchain: nightly-2025-10-23-x86_64-unknown-linux-gnu" >&2
  exit 2
}
cargo afl --version >/dev/null
cbmc --version >/dev/null
command -v goto-cc >/dev/null
command -v goto-instrument >/dev/null

export DOCKER_BUILDKIT=1
docker build \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  .

KANI_HOST_DIR="$KANI_HOST_DIR" IMAGE="$IMAGE" scripts/run_in_experiment_container.sh bash -lc \
  'rustc --version && clang --version | head -n 1 && llvm-config --version && cargo afl --version && (find "$XDG_DATA_HOME/afl.rs" -path "*/afl/bin/afl-fuzz" -type f -executable | grep -q . || cargo afl config --build >/dev/null) && find "$XDG_DATA_HOME/afl.rs" -path "*/afl/bin/afl-fuzz" -type f -executable | grep -q . && cargo-kani --version && cbmc --version | head -n 1'
