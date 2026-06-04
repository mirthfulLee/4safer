#!/usr/bin/env bash
set -euo pipefail

IMAGE="${IMAGE:-cargo-4safer-experiments:latest}"
VARIES_KANI_HOST_DIR="${VARIES_KANI_HOST_DIR:-/data/huanli/projects/varies_root/varies_kani}"
DOCKERFILE="${DOCKERFILE:-Dockerfile.experiments}"

if [[ ! -d "$VARIES_KANI_HOST_DIR" ]]; then
  echo "missing varies_kani checkout: $VARIES_KANI_HOST_DIR" >&2
  exit 2
fi

if [[ ! -x "$VARIES_KANI_HOST_DIR/target/kani/bin/cargo-kani" ]]; then
  echo "missing prebuilt varies_kani cargo-kani binary under $VARIES_KANI_HOST_DIR/target/kani/bin" >&2
  echo "build varies_kani first, or point VARIES_KANI_HOST_DIR at a built checkout" >&2
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

VARIES_KANI_HOST_DIR="$VARIES_KANI_HOST_DIR" IMAGE="$IMAGE" scripts/run_in_experiment_container.sh bash -lc \
  'rustc --version && clang --version | head -n 1 && llvm-config --version && cargo afl --version && (find "$XDG_DATA_HOME/afl.rs" -path "*/afl/bin/afl-fuzz" -type f -executable | grep -q . || cargo afl config --build >/dev/null) && find "$XDG_DATA_HOME/afl.rs" -path "*/afl/bin/afl-fuzz" -type f -executable | grep -q . && command -v cargo-4safer-kani && python3 -m py_compile /opt/varies_kani/scripts/cargo-4safer-kani && cargo-kani --version && cbmc --version | head -n 1'
