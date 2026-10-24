#!/usr/bin/env bash
# Runs logbook-server from this checkout: fetches libparakeet for this
# platform into vendor/ and the models into ~/.cache/brainy-org when they
# are missing, from the mirror, then builds and starts the server. Arguments go to the server
# (`scripts/server.sh --latency very-low`).
set -euo pipefail

cd "$(dirname "$0")/.."

# Everything comes from the mirror on ghcr.io/brainy-org (github.com/brainy-org/models),
# never from GitHub releases or Hugging Face: each file is a blob of a
# bundle, fetched by its sha256 with oras (which checks it). The mirror is
# private: `gh auth token | oras login ghcr.io -u <user> --password-stdin`
# once (read:packages).
MIRROR=ghcr.io/brainy-org/models
# The `libparakeet` bundle (v0.6.1): file and sha256 per platform.
LIBPARAKEET=(
  "linux-cpu-x64 5a2b439e83d1b6ab66c55bcfc48751ee48e53727d5b3ea33a6bc0e8626f1397c"
  "linux-cpu-arm64 d9e56bab07f87b6c05593a039f7cfe5b6033995da80f21721f167bfb41698c42"
  "linux-cuda-x64 f717526c49d56cd4691390ca4c8e521a27eec698a93f3897bf3817ceb744378a"
  "macos-metal-arm64 1847118600359355c899619e263275d83b59f81dcc34fcca6ba4e6b9aac67db0"
  "macos-cpu-x64 4575814a5abb2d8d78b657750a3418cf2178765aa8f2f4ebfd692bf78fda9e7d"
)
# The `parakeet` bundle, the one brainy-worker loads (its models.json):
# file and sha256. MODELS_DIR in crates/server/src/main.rs names the folder.
MODELS=(
  "tdt-0.6b-v3-q8_0.gguf 4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757"
  "silero-vad-f16.gguf 8160489282352accc0e95925c2f6bf3d76fb8f7bccdce5c7e46808cb15e8443c"
  "nemotron-3-diarization-q8_0.gguf 76c5bb1fb20d82706142ad32769b7ab496d2458489473a000fd7074c52ceec22"
)

fetch() { # repository sha256 destination
  command -v oras >/dev/null 2>&1 || { echo "oras is needed (brew install oras)" >&2; exit 1; }
  oras blob fetch --no-tty --output "$3.partial" "$MIRROR/$1@sha256:$2" && mv "$3.partial" "$3"
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) build=macos-metal-arm64; lib=libparakeet.dylib ;;
  Darwin-x86_64) build=macos-cpu-x64; lib=libparakeet.dylib ;;
  Linux-x86_64)
    if command -v nvidia-smi >/dev/null 2>&1; then build=linux-cuda-x64; else build=linux-cpu-x64; fi
    lib=libparakeet.so ;;
  Linux-aarch64 | Linux-arm64) build=linux-cpu-arm64; lib=libparakeet.so ;;
  *) echo "no libparakeet build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

if [[ ! -f "vendor/$lib" ]]; then
  for entry in "${LIBPARAKEET[@]}"; do
    read -r name sha <<<"$entry"
    [[ $name == "$build" ]] || continue
    echo "fetching libparakeet ${build}…" >&2
    mkdir -p vendor
    fetch libparakeet "$sha" vendor/lib.tar.gz
    tar xzf vendor/lib.tar.gz -C vendor --strip-components=1
    rm vendor/lib.tar.gz
  done
fi

models="${LOGBOOK_MODELS:-$HOME/.cache/brainy-org/parakeet-741158ae}"
for entry in "${MODELS[@]}"; do
  read -r file sha <<<"$entry"
  [[ -f "$models/$file" ]] && continue
  if [[ -n "${LOGBOOK_MODELS:-}" ]]; then
    echo "missing in $models: $file" >&2
    exit 1
  fi
  echo "fetching ${file}…" >&2
  mkdir -p "$models"
  fetch parakeet "$sha" "$models/$file"
done

# The CUDA build ships its own cudart and cuBLAS next to libparakeet.
export LD_LIBRARY_PATH="$PWD/vendor${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PARAKEET_LIB="$PWD/vendor/$lib"
export LOGBOOK_MODELS="$models"
cargo build --release -q -p logbook-server
exec target/release/logbook-server "$@"
