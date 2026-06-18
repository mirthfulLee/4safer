#!/usr/bin/env bash
set -euo pipefail

if [[ "${IN_CARGO_4SAFER_CONTAINER:-0}" == "1" ]]; then
  exec "$@"
fi

IMAGE="${IMAGE:-cargo-4safer-experiments:latest}"
HOST_CPUS="$(nproc)"
DOCKER_CPUS="${DOCKER_CPUS:-$(( HOST_CPUS / 2 ))}"
if [[ "$DOCKER_CPUS" -lt 1 ]]; then
  DOCKER_CPUS=1
fi
DOCKER_MEMORY="${DOCKER_MEMORY:-100g}"
DOCKER_MEMORY_SWAP="${DOCKER_MEMORY_SWAP:-100g}"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER_REPO_DIR="${CONTAINER_REPO_DIR:-/workspace/4safer}"
WORKDIR="${CONTAINER_WORKDIR:-$CONTAINER_REPO_DIR}"
CONTAINER_POPULAR_CRATES_DIR="${CONTAINER_POPULAR_CRATES_DIR:-/downloads/popular_crates}"
DEFAULT_POPULAR_CRATES_HOST_DIR="$(cd "$REPO_DIR/../.." && pwd)/downloads/popular_crates"
DEFAULT_FOURSAFER_KANI_HOST_DIR="$(cd "$REPO_DIR/.." && pwd)/varies_root/varies_kani"
POPULAR_CRATES_HOST_DIR="${POPULAR_CRATES_HOST_DIR:-$DEFAULT_POPULAR_CRATES_HOST_DIR}"
FOURSAFER_KANI_HOST_DIR="${FOURSAFER_KANI_HOST_DIR:-$DEFAULT_FOURSAFER_KANI_HOST_DIR}"
RUSTUP_HOST_DIR="${RUSTUP_HOST_DIR:-${RUSTUP_HOME:-$(rustup show home)}}"
CARGO_HOST_DIR="${CARGO_HOST_DIR:-${CARGO_HOME:-$(dirname "$(dirname "$(command -v cargo)")")}}"
CBMC_HOST_BIN="${CBMC_HOST_BIN:-$(command -v cbmc)}"
GOTO_CC_HOST_BIN="${GOTO_CC_HOST_BIN:-$(command -v goto-cc)}"
GOTO_INSTRUMENT_HOST_BIN="${GOTO_INSTRUMENT_HOST_BIN:-$(command -v goto-instrument)}"
CONTAINER_HOME_HOST_DIR="${CONTAINER_HOME_HOST_DIR:-$REPO_DIR/tmp/experiment-container-home}"
CONTAINER_HOME="${CONTAINER_HOME:-/workspace_home}"

mkdir -p "$CONTAINER_HOME_HOST_DIR/.cache" "$CONTAINER_HOME_HOST_DIR/.local/share"

POPULAR_CRATES_MOUNT=()
if [[ -d "$POPULAR_CRATES_HOST_DIR" ]]; then
  POPULAR_CRATES_MOUNT=(-v "$POPULAR_CRATES_HOST_DIR:$CONTAINER_POPULAR_CRATES_DIR")
fi

docker run --rm --init \
  --cpus "$DOCKER_CPUS" \
  --memory "$DOCKER_MEMORY" \
  --memory-swap "$DOCKER_MEMORY_SWAP" \
  --shm-size "${DOCKER_SHM_SIZE:-4g}" \
  --user "$(id -u):$(id -g)" \
  -e IN_CARGO_4SAFER_CONTAINER=1 \
  -e HOME="$CONTAINER_HOME" \
  -e XDG_CACHE_HOME="$CONTAINER_HOME/.cache" \
  -e XDG_DATA_HOME="$CONTAINER_HOME/.local/share" \
  -e RUSTUP_HOME="$CONTAINER_HOME/.rustup" \
  -e CARGO_HOME="$CONTAINER_HOME/.cargo" \
  -e FOURSAFER_KANI_DIR=/opt/varies_kani \
  -e SOURCES_DIR="${CONTAINER_SOURCES_DIR:-$CONTAINER_POPULAR_CRATES_DIR}" \
  -e CARGO_KANI_TOOLCHAIN="${CARGO_KANI_TOOLCHAIN:-nightly-2025-10-23-x86_64-unknown-linux-gnu}" \
  -e RUST_BACKTRACE="${RUST_BACKTRACE:-1}" \
  -v "$FOURSAFER_KANI_HOST_DIR:/opt/varies_kani" \
  -v "$CONTAINER_HOME_HOST_DIR:$CONTAINER_HOME" \
  -v "$RUSTUP_HOST_DIR:$CONTAINER_HOME/.rustup" \
  -v "$CARGO_HOST_DIR:$CONTAINER_HOME/.cargo" \
  -v "$CBMC_HOST_BIN:/usr/local/bin/cbmc:ro" \
  -v "$GOTO_CC_HOST_BIN:/usr/local/bin/goto-cc:ro" \
  -v "$GOTO_INSTRUMENT_HOST_BIN:/usr/local/bin/goto-instrument:ro" \
  -v "$REPO_DIR:$CONTAINER_REPO_DIR" \
  "${POPULAR_CRATES_MOUNT[@]}" \
  -w "$WORKDIR" \
  "$IMAGE" \
  "$@"
