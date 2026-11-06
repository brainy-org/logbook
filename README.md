# logbook

Live meeting transcription with speakers, self-hosted. `logbook`
transcribes the meeting phrase by phrase, about a second after each is
spoken, with parakeet.cpp's models (Silero VAD, NVIDIA
parakeet-tdt-0.6b-v3, Nemotron-3-Diarization): on your computer, or, with
`--server`, on a `logbook-server` that holds the models for everyone.

A prototype: a desktop app (macOS; Windows built, not tried; Android
built as a recorder for a server, not tried), a CLI and a server. Once the meeting
ends it is read all again (better speakers and phrases), a language
model can write its report from the transcript and the notes you took,
as Granola does, and the report becomes a page in brainy (the transcript
stays on your computer).

## Quick start

```bash
scripts/client.sh play meeting.m4a         # a recording, at the pace of speech, transcribed here
scripts/client.sh live --system            # the microphone and the call, until Ctrl-C
```

or with a server:

```bash
scripts/server.sh                          # terminal 1: wait for "listening on http://127.0.0.1:3300"
export LOGBOOK_SERVER=http://127.0.0.1:3300
export LOGBOOK_SERVER_TOKEN=lbk_…          # yours, from the server's admin page
scripts/client.sh live --system            # terminal 2
```

The server lets in only the people it has a token for. Open
`http://127.0.0.1:3300`, sign in with the admin token (the one made
at the server's first start, in `admin.token` in its data folder:
`~/Library/Application Support/logbook-server` on macOS,
`~/.local/share/logbook-server` on Linux; or `LOGBOOK_ADMIN_TOKEN`), and
make one per person: it is shown once. Revoked there, it stops working
at once. The server keeps only the tokens' hashes. Across a network,
put HTTPS in front of it (a proxy): the tokens travel with every
request. `--no-auth` drops them, for a server only its own computer
reaches.

Without `--server` (`LOGBOOK_SERVER`) the CLI loads the models itself
(about 1 GB of files) and runs the same pipeline as the server, in its
own process: an M3 laptop kept up with a recording played at four times
the pace of speech; a laptop without a GPU may not (`bench` tells). A server is for clients that
cannot, and gives every one of them the same quality. The audio goes to
it in Opus, 32 kbit/s a channel (15 MB an hour; an older server gets
PCM, eight times as much).

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
`~/.config/logbook/groq.key` (or `LOGBOOK_LLM_KEY`), `scripts/server.sh`
also has reports written by `qwen/qwen3.8-27b` there (`--llm-url`,
below). Groq then reads the text of every meeting whose report is asked
(`--report`): it is for testing, not for meetings that must stay on your
servers.
`play` and `bench` need ffmpeg on the PATH.

## The app

One app for macOS, Windows and Android (`apps/logbook`).

```bash
scripts/app.sh dev       # run it from this checkout (pnpm and Rust)
scripts/app.sh cert      # once per Mac: a self-signed signing certificate
scripts/app.sh build     # Logbook.app and a .dmg in target/release/bundle/
scripts/app.sh windows   # the Windows installer (x64), built on this Mac
scripts/app.sh android   # the Android app (arm64), a debug APK
```

Say what the meeting is (a call, people in the room, or a room on a
call: the app records the call and tells apart the voices around the
computer accordingly, and remembers the choice), start it, write your notes while it goes (the live transcript is
beside them; ⌘⇧1, or Ctrl+Shift+1, from any app, or the Screen button,
puts a screenshot of the screen under the pointer in the notes, and on
the brainy page; ⌘⇧2 or Area an area you select, on macOS), stop it: the final transcript, the report and its page in
brainy follow, and the meeting stays in the list on the left (one
ended with brainy off goes there later with Send to brainy). While it
records, its title can be changed in place. Afterwards, on the meeting's
page, click a
speaker ("Speaker 2") to give them a name, or pick one of the names
their voice is close to: the transcript
and the voices known by name change with it, the report is written
again with the new names a few seconds later, and the next meetings
name them by themselves. With many voices known, choose who is expected
before you start: the speakers are named from those voices only. Voices,
in the sidebar, lists the people known by voice: find one as you type,
sort them by name or by when they were last heard (recognized in a
meeting or named), either way round, see who needs more
voice (less than 30 s from any one meeting), rename or forget them. In the settings: on this computer or on a
server (a logbook-server, with your token on it), the microphone and the output to record, brainy on or off (its token is
kept on this computer, readable by you only), and the language model for the
report (a provider, Groq, OpenAI, Mistral or any OpenAI-compatible API,
and one of the models it lists; with a server, the server's). The app and the CLI share the data folder: the
pending meetings and the voices.

The app is signed with a self-signed certificate, not notarized (that
needs an Apple Developer ID): macOS keeps its permissions (microphone,
the call's audio) from one build to the next, but a Mac that did not
build it blocks it at first open: System Settings → Privacy & Security →
Open Anyway, or `xattr -dr com.apple.quarantine /Applications/Logbook.app`.

On Windows (not tried yet): `Logbook_<version>_x64-setup.exe`, in
`target/x86_64-pc-windows-msvc/release/bundle/nsis/`. It needs
`brew install llvm nsis` and `cargo install --locked cargo-xwin` on the
Mac that builds it. It is not signed: SmartScreen warns at first open
(More info → Run anyway). Transcribing on the computer itself needs the
[Visual C++ Redistributable x64](https://aka.ms/vs/17/release/vc_redist.x64.exe);
with a logbook-server it does not.

### The Android app

An experiment: the phone records the room and sends it to a
logbook-server, which transcribes it; the meeting goes to brainy and
gets its report from the server's model, as with the desktop app on a
server. No models on the phone. It keeps recording with the screen off
(a notification shows it, with Stop) and finishes meetings left pending
on its own while it is open, as the desktop app does: the home page says
where each one is (sending, transcribing on the server, with how far,
writing to brainy) or what stopped it. Why and how:
`docs/decisions/2026-10-29-android-app.md`.

To build it on a Mac: the Android SDK and NDK and a JDK
(`brew install --cask android-commandlinetools` and `brew install
openjdk@21`, then `sdkmanager "platform-tools" "platforms;android-35"
"build-tools;35.0.0" "ndk;27.2.12479018"` with `ANDROID_HOME` at
`~/Library/Android/sdk`), then `scripts/app.sh android`. The APK is
`apps/logbook/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`
(arm64 only, debug-signed): `adb install -r` it.

On the phone: set the server's address (one the phone reaches, not
`127.0.0.1`) and your token in Settings; allow the microphone and the
notifications at the first recording, and "always run in background"
when asked (or Settings → Apps → Logbook → Battery → Unrestricted). On
Samsung's One UI, also take Logbook out of Settings → Battery →
Background usage limits → Sleeping apps and Deep sleeping apps. The
tokens are kept in a file in the app's private folder, which is weaker
than the macOS keychain.

The phone's microphone is opened for speech recognition at a distance
and its level raised as far as it goes ("Raise faint voices", on by
default): voices across the room are heard, and so is more background
noise.

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
scripts/client.sh live --system --brainy --report                    # the report on the page
scripts/client.sh play meeting.m4a --final                           # the final transcript, printed
```

With `--final` (implied by `--brainy`) the server reads the meeting again
as it goes, a minute or so behind, in longer pieces and with speakers
numbered once for the whole meeting: the final transcript, printed at
the end: who spoke (your name for the microphone, "Speaker 1",
"Speaker 2"… for the call), when, and what. With `--brainy` the page is
created once the meeting is over: its first line "Recorded … · duration ·
speakers" and under it the report (`--report`), or without one the notes;
the transcript never goes to brainy. A meeting in which nothing was said
makes no page. Without `--parent` the page goes at the top of your
default workspace.

### Left running

`live` may run for days or months: the recording is cut into meetings,
each with its own page, report and pending folder, and each ends (its
final transcript, page and report) while the next one records.

```bash
scripts/client.sh live --system --brainy                  # a new meeting after 15 minutes without a word, or 8 hours
scripts/client.sh live --system --brainy --pause 30       # after 30 minutes without a word
scripts/client.sh live --system --brainy --max-length 2   # at 2 hours (the end of the next phrase)
scripts/client.sh live --pause 0 --max-length 0           # one meeting, however long (never cut)
```

A meeting closes when no phrase has ended for `--pause` minutes (15 by
default) while the live transcript runs; when it has lasted
`--max-length` hours (8), at the end of its next phrase, or two minutes
later in any case (continuous noise, or no server: offline only the
length closes it). Every channel is cut at the same instant; the next
meeting starts there, and its page counts from its first word: the start
it shows, its duration and its turns' times (00:00 at the first word). A word said right at the cut may be split
between the two. 0 turns either off; with both off a meeting, and what
it keeps, grow for as long as it runs. `play` takes the same options.

`--parent` (or `LOGBOOK_PARENT`, to set it once) takes the page's
address as the browser shows it, its id, or its path of titles from the
top of your default workspace: `"Meetings"`, `"Engineering/Meetings"`,
in any case. It is looked up when the meeting starts: a title that
matches no page, or more than one, stops there and lists what it found
(for two pages with one title, pass the address of one). A title with a
`/` in it can be named only by its address.

The final transcript is as heard. With a language model (the server's
`--llm-url`, below, or the same `LOGBOOK_LLM_URL`, `LOGBOOK_LLM_MODEL`
and `LOGBOOK_LLM_KEY` given to the CLI without a server), `--report` has
it write the meeting's report (summary, decisions, action items, details
by subject, in the meeting's language): on the brainy page under its first
line, or printed after the transcript without brainy. Without it the
meeting's text never reaches the model.

### Speakers by name

In the transcript the call's voices are "Speaker 1", "Speaker 2"… until you name
them. After a meeting:

```bash
scripts/client.sh speakers                                # known voices, the last meetings' speakers and who they could be
scripts/client.sh speakers name last 2 "Luca Bianchi"     # Speaker 2 of the last meeting is Luca Bianchi
scripts/client.sh speakers rename "Luca Bianchi" "Luca B."  # meetings already written keep the old name
scripts/client.sh speakers forget "Luca Bianchi"
```

From the next meeting on, a voice close to Luca's, and to no one else's,
is written "Luca Bianchi". A doubtful one is written "Speaker 3 (Luca
Bianchi?)". A speaker needs about 10 s of voice heard alone to be named.
30 s or more makes recognition more reliable, and naming the same person
again from another meeting (another microphone, a call) adds to their
voice (the latest 5 prints of each person are kept).

A voice that sounds like someone else's, or unlike the one Luca already
has, is not saved: one wrong voice would name a stranger "Luca" in every
meeting after. If it is Luca all the same (a new microphone, a cold),
`speakers name … --anyway` saves it; the app offers the same after a
rename.

With many voices known, more strangers sound close to someone. Say who is
expected, and the speakers are named from those voices only:

```bash
scripts/client.sh live --system --final --expect "Luca Bianchi,Anna"
```

The voices are kept only on your computer: `voices.json` and, for
the last 20 meetings, `voices/` in the data folder (a meeting deleted in
the app takes its speakers' voices with it; the people named stay known). Ask before you name
someone: a voiceprint is personal data.

### When the network or the server fails

A meeting with `--final` is also kept on your computer, in
`pending/` under `LOGBOOK_DATA` (by default `logbook` in your data folder:
`~/Library/Application Support/logbook` on macOS,
`~/.local/share/logbook` on Linux), until its transcript is published:

- the connection drops during the meeting: recording goes on, the CLI
  connects again as soon as the server answers, sends what was said in
  between, and the final transcript goes on from where it was: nothing
  is read twice, and the speakers keep their names;
- the server cannot be reached at the end (for about a minute: the
  upload is tried again), the connection drops during the final pass,
  or brainy refuses the page: the meeting stays pending
  (with `transcript.md` once the transcript exists), and

```bash
scripts/client.sh finish            # every pending meeting; or: finish DIR
scripts/client.sh finish --wait     # and again until the server and brainy answer
```

picks it up where it stopped: it uploads only the audio the final
transcript has not reached (14 MB an hour a channel). Oldest first. With
`--wait` it tries again whatever could not reach the server or brainy
(or got a "not now" from them: 5xx, 429), every 5 s, doubling up to 5
minutes, until each is finished, for as long as it takes (Ctrl-C to
stop); a meeting refused for another reason (brainy refusing the page
with a 4xx, a token refused, a final pass that failed) is reported and
left pending for that run, since waiting would not fix it. A meeting
still being recorded, or being finished by another `finish` or by the
app, is left to that process. The token is never
written: `finish` reads `BRAINY_TOKEN` again. The audio on your computer
is deleted as the final transcript passes it (a meeting of days keeps
minutes of it), the rest once the final transcript is back, the folder
once it is published; the server keeps the audio only until the final
pass has read it, and the result for an hour.

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
voice, up to 50 dB, for a quiet microphone or someone across the room;
`max` the same, reaching it sooner and with more background noise let
through (the Android app's default); a
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
| `--data` / `LOGBOOK_DATA` | `logbook-server` in the user's data folder | the people's tokens (their hashes, `tokens/`), `admin.token` and the final passes' results for an hour (`finals/`); replicas share it |
| `--admin-token` / `LOGBOOK_ADMIN_TOKEN` | the one in `admin.token`, made at first start | the admin page's (`/`), where the people's tokens are made and revoked |
| `--no-auth` / `LOGBOOK_NO_AUTH` | off | anyone who reaches the server may use it, without a token |
| `--recordings` / `LOGBOOK_RECORDINGS` | `logbook-recordings` in the system's temporary folder | audio of final passes, until they have run; emptied when the server starts, held by one server at a time |
| `--voice` / `LOGBOOK_VOICE` | `wespeaker-resnet34-lm.gguf` in the models folder | speaker encoder of the final pass (required): links each minute's speakers to the meeting's, with no limit of eight |
| `--voice-threshold` | 0.45 | voice similarity above which two minutes' voices are one speaker |
| `--llm-url` / `LOGBOOK_LLM_URL` | none | an OpenAI-compatible API up to `/v1` (`http://localhost:11434/v1` for Ollama) whose model writes reports (`POST /reports`). The meeting's text goes there: a hosted provider reads every meeting whose client asks for a report (`--report`) |
| `--llm-model` / `LOGBOOK_LLM_MODEL` | none | its model; required with `--llm-url` |
| `--llm-key` / `LOGBOOK_LLM_KEY` | none | its API key, if it asks for one |
| `--report-parallel` | 4 | reports written at once |
| `--llm-timeout` | 120 | seconds a request to the model may take |

`PARAKEET_DEVICE` picks the device (`cpu`, `CUDA0`, ...); unset, ggml takes
the first GPU.

### Docker

```bash
docker compose up --build                                        # CPU
docker compose -f compose.yaml -f compose.gpu.yaml up --build    # NVIDIA GPU
docker compose exec server cat /data/admin.token                 # the admin page's token
```

One image (`Dockerfile`) for the CPU and the GPU: it holds the server
only. At its first start the server fetches libparakeet and the models
into the `cache` volume, as `scripts/server.sh` does into
`~/.cache/brainy-org`, and keeps the tokens in the `data` volume
(`/data`). Given the GPU (`--gpus all`, or `compose.gpu.yaml`) it takes
libparakeet's CUDA build, which carries its own CUDA libraries: the host
needs only NVIDIA's driver and container toolkit, and the image is
linux/amd64 there. Settings go in as the variables of the table above
(`.env` beside `compose.yaml`); the server listens on 0.0.0.0:3300 in
the container, published on 127.0.0.1 by compose. Across a network, put
HTTPS in front of it.

### Replicas

Several servers can answer at one address (Kubernetes, behind one
Service), with no session affinity: give them one `--data`, a volume
they all mount for writing (`ReadWriteMany`: NFS, EFS, CephFS), and each
its own `--recordings` on its own disk (an `emptyDir`). A token made or
revoked on one is made or revoked on all, and any of them answers for a
final pass another one runs. Their clocks must agree within seconds
(NTP). Why and how:
`docs/decisions/2026-10-29-replicas-share-the-data-folder.md`.

## Licences

Code: parakeet.cpp and ggml MIT; the server's admin page carries the
Geist typeface, SIL OFL 1.1 (`crates/server/src/assets/geist-OFL.txt`); the CUDA build of libparakeet carries
NVIDIA's cudart and cuBLAS under their redistributable EULA. Weights:
parakeet-tdt-0.6b-v3 CC-BY-4.0 (attribution: NVIDIA),
Nemotron-3-Diarization OpenMDW-1.1, Silero VAD MIT, WeSpeaker ResNet34-LM
CC-BY-4.0 (attribution: the WeSpeaker project; trained on VoxCeleb2).
Each bundle of the mirror carries a `NOTICE.md` with its sources.
