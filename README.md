# logbook

Live meeting transcription with speakers, self-hosted. `logbook-server`
holds the models (parakeet.cpp: Silero VAD, NVIDIA parakeet-tdt-0.6b-v3,
Nemotron-3-Diarization) and transcribes the audio that `logbook` streams
to it, phrase by phrase, about a second after each is spoken.

A prototype: a server and a CLI. The desktop app, echo cancellation and
the final pass into brainy are still to come.

## Quick start

```bash
scripts/server.sh                          # terminal 1: wait for "listening on ws://127.0.0.1:3300/live"
scripts/client.sh play meeting.m4a         # terminal 2: a recording, at the pace of speech
scripts/client.sh live --system            # the microphone and the call, until Ctrl-C
```

`scripts/server.sh` fetches libparakeet v0.6.1 for this platform into
`vendor/` (Metal on Apple silicon, CUDA on Linux with `nvidia-smi`, CPU
otherwise) and the models into the Hugging Face cache when they are
missing (with the `hf` command; the files brainy-worker uses, so already
there if brainy-worker has read a recording on this machine), then builds
and starts the server; its arguments go to the server. `scripts/client.sh`
builds the CLI and passes its arguments on, from the current folder.
`play` and `bench` need ffmpeg on the PATH.

## Use

```bash
scripts/client.sh play meeting.m4a        # a recording, at the pace of speech
scripts/client.sh live                    # the microphone, until Ctrl-C
scripts/client.sh live --system           # the microphone and the call
scripts/client.sh live --system --no-mic  # only the call
scripts/client.sh live --diarize-mic      # the microphone in a room
scripts/client.sh devices                 # what live can record from
scripts/client.sh bench meeting.m4a --sessions 8
```

```
[00:00.5] system S1  Mr Quilter is the apostle of the middle classes, and we are glad to welcome his gospel.   (+1.20 s)
[00:06.9] system S2  Well, I don't wish to see it any more, observed Phoebe, turning away her eyes.   (+1.38 s)
```

The figure in brackets is the latency: from the phrase's last word to its
text. `bench` gives it by quarter of the recording; a latency that grows
along it means the server does not keep up.

### The call's audio

`--system` records what the computer plays as a second channel, whose
speakers are told apart; the microphone stays "mic". macOS asks once for
permission to record system audio (14.6 or later); Windows records the
default output through WASAPI loopback; on Linux pick PipeWire's monitor
with `--system-device "Monitor of"` (Windows and Linux not tried yet). Wear headphones: without them the
microphone hears the call too and every phrase comes twice
(`docs/decisions/2026-10-24-system-audio-through-cpal.md`).

### Server options

| Option | Default | |
|---|---|---|
| `--listen` / `LOGBOOK_LISTEN` | `127.0.0.1:3300` | |
| `--library` / `PARAKEET_LIB` | loader's search path | libparakeet |
| `--models` / `LOGBOOK_MODELS` | brainy-worker's revision in the Hugging Face cache | folder of the GGUF files |
| `--diarization-contexts` | 2 | copies of the diarization model (about 110 MB each) |
| `--latency` | `low` | diarization: `low` 1.04 s, `very-low` 0.64 s, `ultra-low` 0.32 s |
| `--min-pause` | 0.5 | seconds of silence that close a phrase |
| `--max-batch` | 16 | phrases per transcription call |

`PARAKEET_DEVICE` picks the device (`cpu`, `CUDA0`, ...); unset, ggml takes
the first GPU.

## Licences

Code: parakeet.cpp and ggml MIT. Weights: parakeet-tdt-0.6b-v3 CC-BY-4.0
(attribution: NVIDIA), Nemotron-3-Diarization OpenMDW-1.1, Silero VAD MIT.
