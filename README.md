# logbook

Live meeting transcription with speakers, self-hosted. `logbook-server`
holds the models (parakeet.cpp: Silero VAD, NVIDIA parakeet-tdt-0.6b-v3,
Nemotron-3-Diarization) and transcribes the audio that `logbook` streams
to it, phrase by phrase, about a second after each is spoken.

A prototype: a server and a CLI. Once the meeting ends, the server can
read it all again (better speakers and phrases) and the CLI writes it as
a page in brainy. The desktop app is still to come.

## Quick start

```bash
scripts/server.sh                          # terminal 1: wait for "listening on http://127.0.0.1:3300"
scripts/client.sh play meeting.m4a         # terminal 2: a recording, at the pace of speech
scripts/client.sh live --system            # the microphone and the call, until Ctrl-C
```

`scripts/server.sh` fetches libparakeet v0.6.1 for this platform into
`vendor/` (Metal on Apple silicon, CUDA on Linux with `nvidia-smi`, CPU
otherwise) and the models into `~/.cache/brainy-org/parakeet-741158ae`
when they are missing, then builds and starts the server; its arguments
go to the server. Both come from the mirror on ghcr.io/brainy-org
(github.com/brainy-org/models: the files brainy-worker uses), fetched by
sha256 with [oras](https://oras.land), which needs a login once:
`gh auth token | oras login ghcr.io -u <user> --password-stdin`.
`scripts/client.sh` builds the CLI and passes its arguments on, from the
current folder.
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

### The meeting in brainy

```bash
export BRAINY_URL=https://brainy.example.com BRAINY_TOKEN=brainy_…   # a personal token with write access
scripts/client.sh live --system --brainy --me Marco                  # Ctrl-C at the end of the meeting
scripts/client.sh live --system --brainy --parent PAGE_ID --title "Weekly sync"
scripts/client.sh play meeting.m4a --final                           # the final transcript, printed
```

After Ctrl-C the live transcript finishes, then the server reads the
whole meeting again (`--final`, implied by `--brainy`) and the CLI writes
the page: who spoke (your name for the microphone, "Speaker 1", "Speaker
2"… for the call), when, and what. Without `--parent` the page goes at the
top of your default workspace.

### When the network or the server fails

A meeting with `--final` is also kept on your computer, in
`pending/` under `LOGBOOK_DATA` (by default `logbook` in your data folder:
`~/Library/Application Support/logbook` on macOS,
`~/.local/share/logbook` on Linux), until its transcript is published:

- the connection drops during the meeting: recording goes on, the CLI
  connects again as soon as the server answers (no live text for what
  was said in between), and at the end it uploads its own copy for the
  final pass (14 MB an hour a channel);
- the server cannot be reached at the end, the connection drops during
  the final pass, or brainy refuses the page: the meeting stays pending
  (with `transcript.md` once the transcript exists), and

```bash
scripts/client.sh finish            # every pending meeting; or: finish DIR
```

picks it up where it stopped. The token is never written: `finish` reads
`BRAINY_TOKEN` again. The audio on your computer is deleted once the final
transcript is back, the folder once it is published; the server keeps
the audio only until the final pass has read it, and the result for an
hour.

### The call's audio

`--system` records what the computer plays as a second channel, whose
speakers are told apart; the microphone stays "mic". macOS asks once for
permission to record system audio (14.6 or later); Windows records the
default output through WASAPI loopback; on Linux pick PipeWire's monitor
with `--system-device "Monitor of"` (Windows and Linux not tried yet).
Without headphones the microphone hears the call too: the CLI cancels it
from the microphone (WebRTC's echo canceller, as video calls do), and the
server drops the call's words the microphone still picked up
(`docs/pipeline.md`, "Echo"); while the call speaks your own words come a
few seconds late. With headphones, say so (`--headphones`) and nothing is
cancelled or filtered. `--no-aec` keeps only the server's filter, to
compare.

### Server options

| Option | Default | |
|---|---|---|
| `--listen` / `LOGBOOK_LISTEN` | `127.0.0.1:3300` | |
| `--library` / `PARAKEET_LIB` | loader's search path | libparakeet |
| `--models` / `LOGBOOK_MODELS` | `~/.cache/brainy-org/parakeet-741158ae` | folder of the GGUF files |
| `--diarization-contexts` | 2 | copies of the diarization model (about 110 MB each) |
| `--latency` | `low` | diarization: `low` 1.04 s, `very-low` 0.64 s, `ultra-low` 0.32 s |
| `--min-pause` | 0.5 | seconds of silence that close a phrase |
| `--context` | 3 | seconds of audio before a phrase transcribed with it, so that a short phrase keeps its language; 0 for none |
| `--max-batch` | 16 | phrases per transcription call |
| `--recordings` / `LOGBOOK_RECORDINGS` | `logbook-recordings` in the system's temporary folder | audio of final passes, until they have run; emptied when the server starts, held by one server at a time |
| `--diarize-max-seconds` | 7200 | longer recordings are not diarized in the final pass |

`PARAKEET_DEVICE` picks the device (`cpu`, `CUDA0`, ...); unset, ggml takes
the first GPU.

## Licences

Code: parakeet.cpp and ggml MIT. Weights: parakeet-tdt-0.6b-v3 CC-BY-4.0
(attribution: NVIDIA), Nemotron-3-Diarization OpenMDW-1.1, Silero VAD MIT.
