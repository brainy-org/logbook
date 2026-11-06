#!/usr/bin/env bash
# Diarizes one recording with every strategy of the `diarize` example and
# scores each against a reference (scripts/der.py): a table of DER.
# What it is for: docs/measuring-a-room-microphone.md.
#
#   scripts/room-der.sh RECORDING.wav REF.rttm [OUT_DIR]
#   scripts/room-der.sh RECORDING.wav           # no reference yet: only the RTTMs
#
# RTTMs and timings go to OUT_DIR (default: RECORDING.der/ beside it). The
# models come from the shared cache (~/.cache/brainy-org/, the `parakeet`
# bundle and libparakeet), fetched by the first start of logbook or arvel.
set -euo pipefail

cd "$(dirname "$0")/.."
wav=${1:?usage: scripts/room-der.sh RECORDING.wav [REF.rttm] [OUT_DIR]}
ref=${2:-}
out=${3:-${wav%.*}.der}
cache=${XDG_CACHE_HOME:-$HOME/.cache}/brainy-org

first() { for f in "$@"; do [ -e "$f" ] && { echo "$f"; return; }; done; }
case "$(uname -s)" in
  Darwin) lib=$(first "$cache"/libparakeet-macos-*/libparakeet.dylib) ;;
  *) lib=$(first "$cache"/libparakeet-linux-*/libparakeet.so) ;;
esac
diar=$(first "$cache"/parakeet-*/nemotron-3-diarization-q8_0.gguf)
voice=$(first "$cache"/parakeet-*/wespeaker-resnet34-lm.gguf)
for x in lib diar voice; do
  [ -n "${!x}" ] || { echo "no $x in $cache: start logbook (scripts/server.sh) or arvel once" >&2; exit 1; }
done

cargo build --release -q -p logbook-engine --example diarize
bin=target/release/examples/diarize
mkdir -p "$out"

# The strategies of docs/decisions/2026-10-24-speakers-across-a-long-meeting.md,
# with the kept one's window and threshold (60 s, 0.45) wherever they apply.
# chunked-linker is what the final pass does.
strategies=(offline stream-low stream-model chunked stream-embed stream-model-embed linker chunked-linker)
for s in "${strategies[@]}"; do
  args=(--lib "$lib" --diar "$diar" --strategy "$s")
  case $s in offline | stream-low | stream-model) ;; *) args+=(--voice "$voice" --window 60 --threshold 0.45) ;; esac
  "$bin" "${args[@]}" "$wav" >"$out/$s.rttm" 2>"$out/$s.log"
done

if [ -z "$ref" ]; then
  echo "RTTMs in $out (no reference given: nothing scored)"
  echo "draft of the reference: uv run scripts/reference.py draft $out/chunked-linker.rttm > labels.txt"
  exit 0
fi

echo "| Strategy | DER | miss | false alarm | confusion | speakers ref / hyp | real time |"
echo "|---|---|---|---|---|---|---|"
for s in "${strategies[@]}"; do
  # "DER  12.3%  miss  4.5  fa  1.2  conf  6.6  speakers ref 4 hyp 5"
  read -r _ der _ miss _ fa _ conf _ _ nref _ nhyp < <(uv run -q scripts/der.py "$ref" "$out/$s.rttm")
  speed=$(sed -n 's/.*(\([0-9.]*x\) real time).*/\1/p' "$out/$s.log" | tail -1)
  echo "| $s | $der | $miss% | $fa% | $conf% | $nref / $nhyp | $speed |"
done
