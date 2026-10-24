#!/usr/bin/env bash
# Runs logbook-server from this checkout: fetches libparakeet for this
# platform into vendor/ and the models into the Hugging Face cache when they
# are missing, then builds and starts the server. Arguments go to the server
# (`scripts/server.sh --latency very-low`).
set -euo pipefail

cd "$(dirname "$0")/.."

PARAKEET_VERSION=v0.6.1
# brainy-worker's audio_revision (MODELS_REVISION in crates/server/src/main.rs).
MODELS_REVISION=741158ae71e64ef5c89385862c18f777d07a97a1
MODELS=(tdt-0.6b-v3-q8_0.gguf silero-vad-f16.gguf nemotron-3-diarization-q8_0.gguf)

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
  name="parakeet-${PARAKEET_VERSION}-lib-${build}"
  echo "fetching ${name}…" >&2
  mkdir -p vendor
  curl -fsSL "https://github.com/mudler/parakeet.cpp/releases/download/${PARAKEET_VERSION}/${name}.tar.gz" |
    tar xz -C vendor --strip-components=1
fi

models="${LOGBOOK_MODELS:-$HOME/.cache/huggingface/hub/models--mudler--parakeet-cpp-gguf/snapshots/$MODELS_REVISION}"
missing=()
for file in "${MODELS[@]}"; do
  [[ -f "$models/$file" ]] || missing+=("$file")
done
if (( ${#missing[@]} )); then
  if [[ -z "${LOGBOOK_MODELS:-}" ]] && command -v hf >/dev/null 2>&1; then
    echo "fetching ${missing[*]}…" >&2
    hf download mudler/parakeet-cpp-gguf "${missing[@]}" --revision "$MODELS_REVISION" >/dev/null
  else
    echo "missing in $models: ${missing[*]}" >&2
    echo "hf download mudler/parakeet-cpp-gguf ${missing[*]} --revision $MODELS_REVISION" >&2
    exit 1
  fi
fi

# The CUDA build ships its own cudart and cuBLAS next to libparakeet.
export LD_LIBRARY_PATH="$PWD/vendor${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PARAKEET_LIB="$PWD/vendor/$lib"
export LOGBOOK_MODELS="$models"
cargo build --release -q -p logbook-server
exec target/release/logbook-server "$@"
