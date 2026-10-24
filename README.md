# logbook

Live meeting transcription with speakers, self-hosted. `logbook-server`
holds the models (parakeet.cpp: Silero VAD, NVIDIA parakeet-tdt-0.6b-v3,
Nemotron-3-Diarization) and transcribes the audio that `logbook` streams
to it, phrase by phrase, about a second after each is spoken.

A prototype: a server and a CLI. The desktop app, capture of the call's
audio and the final pass into brainy are still to come.

## Setup

1. **libparakeet** v0.6.1 for your platform from
   [the releases](https://github.com/mudler/parakeet.cpp/releases/tag/v0.6.1)
   (`parakeet-v0.6.1-lib-<platform>`), into `vendor/`:

   ```bash
   mkdir -p vendor && cd vendor
   curl -fsSLO https://github.com/mudler/parakeet.cpp/releases/download/v0.6.1/parakeet-v0.6.1-lib-macos-metal-arm64.tar.gz
   tar xzf parakeet-v0.6.1-lib-macos-metal-arm64.tar.gz --strip-components=1
   ```

2. **The models**, the files brainy-worker uses (already there if
   brainy-worker has read a recording on this machine):

   ```bash
   hf download mudler/parakeet-cpp-gguf tdt-0.6b-v3-q8_0.gguf silero-vad-f16.gguf \
     nemotron-3-diarization-q8_0.gguf --revision 741158ae71e64ef5c89385862c18f777d07a97a1
   ```

3. **ffmpeg** on the PATH, for `logbook play` and `bench`.

## Use

```bash
PARAKEET_LIB=vendor/libparakeet.dylib cargo run --release -p logbook-server

cargo run --release -p logbook -- play meeting.m4a       # a recording, at the pace of speech
cargo run --release -p logbook -- live                   # the microphone, until Ctrl-C
cargo run --release -p logbook -- live --diarize         # the microphone in a room
cargo run --release -p logbook -- bench meeting.m4a --sessions 8
```

```
[00:00.5] system S1  Mr Quilter is the apostle of the middle classes, and we are glad to welcome his gospel.   (+1.20 s)
[00:06.9] system S2  Well, I don't wish to see it any more, observed Phoebe, turning away her eyes.   (+1.38 s)
```

The figure in brackets is the latency: from the phrase's last word to its
text. `bench` gives it by quarter of the recording; a latency that grows
along it means the server does not keep up.

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
