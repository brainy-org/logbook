# logbook

Live meeting transcription with speakers, self-hosted. `logbook`
transcribes the meeting phrase by phrase, about a second after each is
spoken, with parakeet.cpp's models (Silero VAD, NVIDIA
parakeet-tdt-0.6b-v3, Nemotron-3-Diarization): on your computer, or, with
`--server`, on a `logbook-server` that holds the models for everyone.

A prototype: a desktop app (macOS), a CLI and a server. Once the meeting
ends it is read all again (better speakers and phrases), written as a
page in brainy, and a language model can write its report from the
transcript and the notes you took, as Granola does.

## Quick start

```bash
scripts/client.sh play meeting.m4a         # a recording, at the pace of speech, transcribed here
scripts/client.sh live --system            # the microphone and the call, until Ctrl-C
```

or with a server:

```bash
scripts/server.sh                          # terminal 1: wait for "listening on http://127.0.0.1:3300"
export LOGBOOK_SERVER=http://127.0.0.1:3300
scripts/client.sh live --system            # terminal 2
```

Without `--server` (`LOGBOOK_SERVER`) the CLI loads the models itself
(about 1 GB of files) and runs the same pipeline as the server, in its
own process: an M3 laptop kept up with a recording played at four times
the pace of speech; a laptop without a GPU may not (`bench` tells). A server is for clients that
cannot, and gives every one of them the same quality.

`scripts/server.sh` builds and starts the server; its arguments go to the
server. On its first start the server (or the CLI without one) fetches
what it lacks into
`~/.cache/brainy-org`: libparakeet v0.6.1 for this platform (Metal on
Apple silicon, CUDA on Linux with NVIDIA's driver, CPU otherwise) and the
models (`parakeet-741158ae`, the speaker encoder from the
`parakeet-voice` bundle). Both come from the mirror on ghcr.io/brainy-org
(github.com/brainy-org/models: the files brainy-worker uses), by sha256,
over the registry's API: no other tool is needed, and no token: the
three packages logbook fetches (`parakeet`, `parakeet-voice`,
`libparakeet`) are public. `scripts/server.sh` reads any `LOGBOOK_*`
setting from `.env` in the checkout (not committed).
`scripts/client.sh` builds the CLI and passes its arguments on, from the
current folder.

For now, with a [Groq](https://groq.com) API key in
`~/.config/logbook/groq.key` (or `LOGBOOK_POLISH_KEY`), `scripts/server.sh`
also has the final transcript cleaned up by `qwen/qwen3.8-27b` there
(`--polish-url`, below). Groq then reads every meeting's text: it is for
testing, not for meetings that must stay on your servers: only meetings
started with `--polish` go there.
`play` and `bench` need ffmpeg on the PATH.

## The desktop app

```bash
scripts/desktop.sh dev       # run it from this checkout (pnpm and Rust)
scripts/desktop.sh cert      # once per Mac: a self-signed signing certificate
scripts/desktop.sh build     # Logbook.app and a .dmg in target/release/bundle/
```

Start a meeting, write your notes while it goes (the live transcript is
beside them), stop it: the final transcript, the report and the page in
brainy follow, and the meeting stays in the list on the left. Click a
speaker ("Speaker 2") to give them a name: the transcript, the brainy
page and the voices known by name change with it, and the next meetings
name them by themselves. In the settings: on this computer or on a
logbook-server, the call recorded or not, brainy on or off (its token is
kept in the keychain), and the language model for the clean-up and the
report (Groq's Qwen, or one on this computer through Ollama; with a
server, the server's). The app and the CLI share the data folder: the
pending meetings and the voices.

The app is signed with a self-signed certificate, not notarized (that
needs an Apple Developer ID): macOS keeps its permissions (microphone,
the call's audio) from one build to the next, but a Mac that did not
build it blocks it at first open: System Settings → Privacy & Security →
Open Anyway, or `xattr -dr com.apple.quarantine /Applications/Logbook.app`.

## Use

```bash
scripts/client.sh play meeting.m4a        # a recording, at the pace of speech
scripts/client.sh live                    # the microphone, until Ctrl-C
scripts/client.sh live --system           # the microphone and the call
scripts/client.sh live --system --no-mic  # only the call
scripts/client.sh live --diarize-mic      # the microphone in a room
scripts/client.sh live --mic-gain auto    # a quiet microphone, or voices far from it
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
scripts/client.sh live --system --brainy --parent "Engineering/Meetings" --title "Weekly sync"
scripts/client.sh live --system --brainy --polish                    # cleaned up by the server's model
scripts/client.sh live --system --brainy --report                    # and a report on the page
scripts/client.sh play meeting.m4a --final                           # the final transcript, printed
```

With `--final` (implied by `--brainy`) the server reads the meeting again
as it goes, a minute or so behind, in longer pieces and with speakers
numbered once for the whole meeting: the final transcript. With
`--brainy` the page is created when the meeting starts and the final
transcript is added to it as it comes: who spoke (your name for the
microphone, "Speaker 1", "Speaker 2"… for the call), when, and what.
After Ctrl-C only the last minute or so is left (seconds), then the
page's first line becomes "Recorded … · duration · speakers". Without
`--parent` the page goes at the top of your default workspace.

`--parent` (or `LOGBOOK_PARENT`, to set it once) takes the page's
address as the browser shows it, its id, or its path of titles from the
top of your default workspace: `"Meetings"`, `"Engineering/Meetings"`,
in any case. It is looked up when the meeting starts: a title that
matches no page, or more than one, stops there and lists what it found
(for two pages with one title, pass the address of one). A title with a
`/` in it can be named only by its address.

With a language model (the server's `--polish-url`, below, or the same
`LOGBOOK_POLISH_URL`, `LOGBOOK_POLISH_MODEL` and `LOGBOOK_POLISH_KEY`
given to the CLI without a server), `--polish` has the final transcript
cleaned up there: fillers, false starts,
punctuation. Without it the transcript stays as heard and the meeting's
text never reaches the model; a pending meeting keeps the choice for
`finish`. `--report` has the same model write the meeting's report
(summary, decisions, action items, in the meeting's language): on the
brainy page under its first line, or printed after the transcript.

### Speakers by name

On the page the call's voices are "Speaker 1", "Speaker 2"… until you name
them. After a meeting:

```bash
scripts/client.sh speakers                                # known voices, the last meetings' speakers
scripts/client.sh speakers name last 2 "Luca Bianchi"     # Speaker 2 of the last meeting is Luca Bianchi
scripts/client.sh speakers forget "Luca Bianchi"
```

From the next meeting on, a voice close to Luca's, and to no one else's,
is written "Luca Bianchi". A doubtful one is written "Speaker 3 (Luca
Bianchi?)". A speaker needs about 10 s of voice heard alone to be named.
30 s or more makes recognition more reliable, and naming the same person
again from another meeting (another microphone, a call) adds to their
voice. The voices are kept only on your computer: `voices.json` and, for
the last 20 meetings, `voices/` in the data folder. Ask before you name
someone: a voiceprint is personal data.

### When the network or the server fails

A meeting with `--final` is also kept on your computer, in
`pending/` under `LOGBOOK_DATA` (by default `logbook` in your data folder:
`~/Library/Application Support/logbook` on macOS,
`~/.local/share/logbook` on Linux), until its transcript is published:

- the connection drops during the meeting: recording goes on, the CLI
  connects again as soon as the server answers (no live text for what
  was said in between), and at the end it uploads its own copy for the
  final pass (14 MB an hour a channel);
- the server cannot be reached at the end (for about a minute: the
  upload is tried again), the connection drops during the final pass,
  or brainy refuses the page: the meeting stays pending
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

`--mic-gain` raises the microphone's level before it is sent (WebRTC's
AGC2, after the echo is cancelled, with a limiter): `auto` follows the
voice, up to 50 dB, for a quiet microphone or someone across the room; a
number is a fixed gain in dB (0 to 50); `off`, the default, keeps the
device's level. The system's input volume (macOS: System Settings →
Sound → Input) is the other knob.

Without a server, `finish` reads the pending meeting again on this
computer, and the audio is kept only in the meeting's pending folder.
The CLI takes the server's `--models` too (same variable), for its own
models.

### Server options

| Option | Default | |
|---|---|---|
| `--listen` / `LOGBOOK_LISTEN` | `127.0.0.1:3300` | |
| `--models` / `LOGBOOK_MODELS` | `~/.cache/brainy-org/parakeet-741158ae`, the missing files fetched | folder of the GGUF files; one given is used as it is |
| `--diarization-contexts` | 2 | copies of the diarization model (about 110 MB each) |
| `--latency` | `low` | diarization: `low` 1.04 s, `very-low` 0.64 s, `ultra-low` 0.32 s |
| `--min-pause` | 0.5 | seconds of silence that close a phrase |
| `--context` | 3 | seconds of audio before a phrase transcribed with it, so that a short phrase keeps its language; 0 for none |
| `--max-batch` | 16 | phrases per transcription call |
| `--recordings` / `LOGBOOK_RECORDINGS` | `logbook-recordings` in the system's temporary folder | audio of final passes, until they have run; emptied when the server starts, held by one server at a time |
| `--voice` / `LOGBOOK_VOICE` | `wespeaker-resnet34-lm.gguf` in the models folder | speaker encoder of the final pass (required): links each minute's speakers to the meeting's, with no limit of eight |
| `--voice-threshold` | 0.45 | voice similarity above which two minutes' voices are one speaker |
| `--polish-url` / `LOGBOOK_POLISH_URL` | none | an OpenAI-compatible API up to `/v1` (`http://localhost:11434/v1` for Ollama) whose model cleans the final transcript up (fillers, false starts, punctuation) and writes reports (`POST /reports`). The meeting's text goes there: a hosted provider reads every meeting whose client asks for it (`--polish`, `--report`) |
| `--polish-model` / `LOGBOOK_POLISH_MODEL` | none | its model; required with `--polish-url` |
| `--polish-key` / `LOGBOOK_POLISH_KEY` | none | its API key, if it asks for one |
| `--polish-parallel` | 4 | parts the model is asked to clean at once (one per meeting at most), and reports written at once |
| `--polish-timeout` | 120 | seconds a request may take; a part that takes longer stays as heard |

`PARAKEET_DEVICE` picks the device (`cpu`, `CUDA0`, ...); unset, ggml takes
the first GPU.

## Licences

Code: parakeet.cpp and ggml MIT; the CUDA build of libparakeet carries
NVIDIA's cudart and cuBLAS under their redistributable EULA. Weights:
parakeet-tdt-0.6b-v3 CC-BY-4.0 (attribution: NVIDIA),
Nemotron-3-Diarization OpenMDW-1.1, Silero VAD MIT, WeSpeaker ResNet34-LM
CC-BY-4.0 (attribution: the WeSpeaker project; trained on VoxCeleb2).
Each bundle of the mirror carries a `NOTICE.md` with its sources.
