# Logbook

Live meeting transcription with speakers, without the cloud. Logbook
transcribes a meeting phrase by phrase, about a second after each is
spoken, tells the speakers apart, and once the meeting ends reads it all
again for a final transcript. A language model can then write its report
from the transcript and the notes you took (summary, your notes
completed, decisions, action items), and the report can become a page in
[brainy](https://github.com/brainy-org/brainy).

The speech models (parakeet.cpp: Silero VAD, NVIDIA
parakeet-tdt-0.6b-v3, Nemotron-3-Diarization, a WeSpeaker voice encoder)
run on your computer, or on a `logbook-server` that holds them for a
team. The audio and the transcript stay with you.

Three ways to use it:

- **the app** for macOS and Windows (and an experimental Android recorder
  for a server);
- **the `logbook` CLI**, the same meeting from a terminal;
- **`logbook-server`**, which transcribes for clients that cannot, or
  should not, run the models themselves.

A prototype: macOS is used daily, Windows is built but not yet tried.

## Install the app

Download the latest release from
[GitHub Releases](https://github.com/brainy-org/logbook/releases/latest):

- **macOS** (Apple Silicon, macOS 11 or later): `Logbook_<version>_aarch64.dmg`.
  Open it and drag Logbook to Applications. The app is signed but not
  notarized, so macOS blocks it the first time: System Settings → Privacy &
  Security → Open Anyway (or `xattr -dr com.apple.quarantine
  /Applications/Logbook.app` in a terminal).
- **Windows** (x64): `Logbook_<version>_x64-setup.exe`. It is not signed,
  so SmartScreen warns: More info → Run anyway. Transcribing on the
  computer itself needs the
  [Visual C++ Redistributable x64](https://aka.ms/vs/17/release/vc_redist.x64.exe);
  with a server it does not.

The speech models (about 1 GB) download at the first meeting transcribed
on the computer, once. An M3 laptop keeps up with four meetings' worth of
speech; a laptop without a GPU may fall behind, and is better off with a
server.

**Updates.** Logbook looks for a new version shortly after it starts and
every few hours, and offers it at the bottom right of the window (or
Settings → Version → Check for updates). Installing it restarts the app,
so it waits while a meeting is recorded; a meeting still being finished is
taken up again after the restart.

## Use the app

1. **Say what the meeting is**: a call, people in the room, or a room on a
   call. The app records the call's audio as its own channel and tells
   apart the voices around the computer when there are several; it
   remembers the choice. On macOS it can also notice a call that starts
   (Teams, Zoom, Webex, Slack, Discord or WhatsApp opening the
   microphone; macOS 14.4 or later, not calls in a browser): a
   notification offers to record it, or it is recorded at once, and the
   recording stops when the call ends.
2. **Write your notes** while the live transcript runs beside them. ⌘⇧1
   (Ctrl+Shift+1), from any app, or the Screen button, puts a screenshot of
   the screen under the pointer in the notes; ⌘⇧2 or Area, a part of the
   screen you select (macOS). The title can be changed while it records.
3. **Stop it**: the final transcript, the report and the brainy page
   follow. The meeting stays in the list on the left; one ended with
   brainy off can be sent later.
4. **Name the speakers**: on the meeting's page, click "Speaker 2" to give
   them a name, or pick one of the names their voice is close to. The
   transcript changes, the report is written again with the new names,
   and the next meetings name them by themselves. With many voices known,
   choose who is expected before you start. Voices, in the sidebar, lists
   the people known by voice: rename or forget them, see who needs more
   voice.

In the settings:

- **Where meetings are transcribed**: on this computer, or on a
  logbook-server with your token on it.
- **What is recorded**: the microphone, the output (the call), headphones
  or not (without them the call is cancelled from the microphone), a quiet
  microphone raised.
- **brainy**: on or off, its address, your token and the page meetings go
  under.
- **The report's language model**, used without a server (with one, the
  server's): a hosted provider with an OpenAI-compatible API (Groq,
  OpenAI, Mistral, or any other at an address you give, such as Ollama),
  with your key and one of the models it lists; or **Claude Code** or
  **Codex** installed on this computer, with the account they are signed in
  with, no API key needed. The settings check that the CLI is signed in,
  and a meeting with a report does not start while it is not (`claude auth
  login`, `codex login`).

Keys and tokens are kept on the computer, readable by you only, and never
shown again.

## What stays where

- **The audio** stays on your computer, in the meeting's pending folder,
  only until the final transcript has read it. A server keeps it only
  until its final pass has read it.
- **The transcript** stays on your computer: the brainy page holds the
  report (or your notes), never the transcript.
- **The voices** known by name are kept only on your computer (a
  voiceprint is personal data: ask before you name someone). The server
  sends a meeting's voices back and keeps none.
- **The report's model** reads the meeting's text: a hosted provider, or
  Anthropic and OpenAI behind Claude Code and Codex, reads every meeting it
  reports on. For meetings that must stay in the company, use a model you
  host, or no report.

## The CLI

```bash
scripts/client.sh play meeting.m4a         # a recording, at the pace of speech, transcribed here
scripts/client.sh live --system            # the microphone and the call, until Ctrl-C
```

`scripts/client.sh` builds the CLI (Rust) and passes its arguments on.
Without `--server` (`LOGBOOK_SERVER`) it loads the models itself and runs
the same pipeline as the server, in its own process. `play` and `bench`
need ffmpeg on the PATH.

```bash
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

### The final transcript, the report and brainy

```bash
scripts/client.sh play meeting.m4a --final                           # the final transcript, printed
export BRAINY_URL=https://brainy.example.com BRAINY_TOKEN=brainy_…   # a personal token with write access
scripts/client.sh live --system --brainy --me Marco                  # Ctrl-C at the end of the meeting
scripts/client.sh live --system --brainy --parent "Engineering/Meetings" --title "Weekly sync"
scripts/client.sh live --system --brainy --report                    # the report on the page
```

With `--final` (implied by `--brainy` and `--report`) the meeting is read
again as it goes, a minute or so behind, in longer pieces and with
speakers numbered once for the whole meeting: the final transcript,
printed at the end, with who spoke (your name for the microphone,
"Speaker 1", "Speaker 2"… for the call), when, and what.

`--report` has a language model write the meeting's report (summary,
decisions, action items, details by subject, in the meeting's language):
the server's, or without a server one of these:

```bash
export LOGBOOK_LLM_URL=http://localhost:11434/v1 LOGBOOK_LLM_MODEL=qwen3:32b   # an OpenAI-compatible API (LOGBOOK_LLM_KEY if it asks)
export LOGBOOK_LLM_PROVIDER=claude                                             # Claude Code on this computer (or codex), signed in
```

Without `--report` the meeting's text never reaches a model.

With `--brainy` the page is created once the meeting is over: its first
line "Recorded … · duration · speakers" and under it the report, or
without one the notes. A meeting in which nothing was said makes no page.
`--parent` (or `LOGBOOK_PARENT`) takes the page's address as the browser
shows it, its id, or its path of titles from the top of your default
workspace (`"Engineering/Meetings"`, in any case); without it the page
goes at the top of your default workspace. A title that matches no page,
or more than one, stops the meeting at its start and lists what it found.

### Left running

`live` may run for days or months: the recording is cut into meetings,
each with its own page, report and pending folder, and each ends while
the next one records.

```bash
scripts/client.sh live --system --brainy                  # a new meeting after 15 minutes without a word, or 8 hours
scripts/client.sh live --system --brainy --pause 30       # after 30 minutes without a word
scripts/client.sh live --system --brainy --max-length 2   # at 2 hours (the end of the next phrase)
scripts/client.sh live --pause 0 --max-length 0           # one meeting, however long (never cut)
```

A meeting closes when no phrase has ended for `--pause` minutes while the
live transcript runs, or when it has lasted `--max-length` hours, at the
end of its next phrase (two minutes later in any case). Every channel is
cut at the same instant, and the next meeting's page counts from its first
word. A word said right at the cut may be split between the two. `play`
takes the same options.

### Speakers by name

```bash
scripts/client.sh speakers                                  # known voices, the last meetings' speakers and who they could be
scripts/client.sh speakers name last 2 "Luca Bianchi"       # Speaker 2 of the last meeting is Luca Bianchi
scripts/client.sh speakers rename "Luca Bianchi" "Luca B."  # meetings already written keep the old name
scripts/client.sh speakers forget "Luca Bianchi"
scripts/client.sh live --system --final --expect "Luca Bianchi,Anna"   # name speakers from these voices only
```

From the next meeting on, a voice close to Luca's, and to no one else's,
is written "Luca Bianchi"; a doubtful one "Speaker 3 (Luca Bianchi?)". A
speaker needs about 10 s of voice heard alone to be named; 30 s or more
makes recognition more reliable, and naming the same person from another
meeting (another microphone, a call) adds to their voice (the latest 5
prints of each person are kept). A voice that sounds like someone else's,
or unlike the one Luca already has, is not saved, since one wrong voice
would name a stranger "Luca" in every meeting after; `speakers name …
--anyway` saves it all the same (the app offers the same). With many
voices known, more strangers sound close to someone: `--expect` says who
is there.

The voices are kept in `voices.json` and, for the last 20 meetings,
`voices/` in the data folder.

### When the network or the server fails

A meeting with `--final` is kept on your computer, in `pending/` under
`LOGBOOK_DATA` (by default `logbook` in your data folder:
`~/Library/Application Support/logbook` on macOS,
`~/.local/share/logbook` on Linux), until its transcript is published. The
app and the CLI share this folder.

- The connection drops during the meeting: recording goes on, the CLI
  connects again as soon as the server answers, sends what was said in
  between, and the final transcript goes on from where it was.
- The server cannot be reached at the end, the connection drops during
  the final pass, or brainy refuses the page: the meeting stays pending,
  and

```bash
scripts/client.sh finish            # every pending meeting; or: finish DIR
scripts/client.sh finish --wait     # and again until the server and brainy answer
```

picks it up where it stopped, uploading only the audio the final
transcript has not reached. With `--wait` it tries again whatever could
not reach the server or brainy (or got a 5xx or 429 from them), every 5 s
doubling up to 5 minutes, until each is finished; a meeting refused for
another reason is reported and left pending. A meeting still being
recorded, or finished by another process, is left to it. Tokens are never
written: `finish` reads `BRAINY_TOKEN` again. Without a server, `finish`
reads the pending meeting again on this computer.

### The call's audio

`--system` records what the computer plays as a second channel, whose
speakers are told apart; the microphone stays "mic". macOS asks once for
permission to record system audio (14.6 or later); Windows records the
default output through WASAPI loopback; on Linux pick PipeWire's monitor
with `--system-device "Monitor of"` (Windows and Linux not tried yet).
Without headphones the microphone hears the call too: the CLI cancels it
from the microphone (WebRTC's echo canceller), and the pipeline drops the
call's words the microphone still picked up (`docs/pipeline.md`,
"Echo"); while the call speaks your own words come a few seconds late.
With headphones, say so (`--headphones`) and nothing is cancelled.
`--no-aec` keeps only the word filter, to compare.

`--mic-gain` raises the microphone's level before it is sent (WebRTC's
AGC2, with a limiter): `auto` follows the voice, up to 50 dB; `max` the
same, reaching it sooner and letting more background noise through; a
number is a fixed gain in dB (0 to 50); `off`, the default, keeps the
device's level.

## The server

```bash
scripts/server.sh                          # terminal 1: wait for "listening on http://127.0.0.1:3300"
export LOGBOOK_SERVER=http://127.0.0.1:3300
export LOGBOOK_SERVER_TOKEN=lbk_…          # yours, from the server's admin page
scripts/client.sh live --system            # terminal 2
```

`scripts/server.sh` builds and starts the server; its arguments go to the
server, and it reads any `LOGBOOK_*` setting from `.env` in the checkout
(not committed). With a [Groq](https://groq.com) key in
`~/.config/logbook/groq.key` it also has reports written by
`qwen/qwen3.8-27b` there: Groq then reads every meeting whose report is
asked, so it is for testing.

The server lets in only the people it has a token for. Open
`http://127.0.0.1:3300`, sign in with the admin token (made at the
server's first start, in `admin.token` in its data folder:
`~/Library/Application Support/logbook-server` on macOS,
`~/.local/share/logbook-server` on Linux; or `LOGBOOK_ADMIN_TOKEN`), and
make one per person: it is shown once, and revoked there it stops working
at once. The server keeps only the tokens' hashes. Across a network, put
HTTPS in front of it (a proxy): the tokens travel with every request.
`--no-auth` drops them, for a server only its own computer reaches.

The audio goes to the server in Opus, 32 kbit/s a channel (15 MB an
hour). On its first start the server (or the CLI or app without one)
fetches what it lacks into `~/.cache/brainy-org`: libparakeet v0.6.1 for
this platform (Metal on Apple Silicon, CUDA on Linux with NVIDIA's driver,
CPU otherwise) and the models, from the mirror on ghcr.io/brainy-org
([brainy-org/models](https://github.com/brainy-org/models), the same files
brainy-worker uses), checked by sha256, with no token and no other tool.

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
| `--llm-provider` / `LOGBOOK_LLM_PROVIDER` | `api` | what writes reports: `api`, the model of `--llm-url`; `claude` or `codex`, that CLI on the server's machine, signed in with its account (`claude setup-token` and `CLAUDE_CODE_OAUTH_TOKEN`, or `codex login`), checked at start: a server that is not signed in does not start. Its default model, or `--llm-model`. The CLIs are not in the Docker image. `LOGBOOK_CLAUDE_BIN`, `LOGBOOK_CODEX_BIN`: a CLI that is neither on PATH nor in `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin` |
| `--llm-url` / `LOGBOOK_LLM_URL` | none | an OpenAI-compatible API up to `/v1` (`http://localhost:11434/v1` for Ollama) whose model writes reports (`POST /reports`). The meeting's text goes there: a hosted provider reads every meeting whose client asks for a report |
| `--llm-model` / `LOGBOOK_LLM_MODEL` | none | its model; required with `--llm-url` |
| `--llm-key` / `LOGBOOK_LLM_KEY` | none | its API key, if it asks for one |
| `--report-parallel` | 4 | reports written at once |
| `--llm-timeout` | 120 (600 for a CLI) | seconds a request to the model may take |

`PARAKEET_DEVICE` picks the device (`cpu`, `CUDA0`, ...); unset, ggml takes
the first GPU.

### Docker

```bash
docker compose up --build                                        # CPU
docker compose -f compose.yaml -f compose.gpu.yaml up --build    # NVIDIA GPU
docker compose exec server cat /data/admin.token                 # the admin page's token
```

One image (`Dockerfile`) for the CPU and the GPU, holding the server only.
At its first start the server fetches libparakeet and the models into the
`cache` volume and keeps the tokens in the `data` volume (`/data`). Given
the GPU (`compose.gpu.yaml`) it takes libparakeet's CUDA build, which
carries its own CUDA libraries: the host needs only NVIDIA's driver and
container toolkit, and the image is linux/amd64 there. Settings go in as
the variables of the table above (`.env` beside `compose.yaml`); the
server listens on 0.0.0.0:3300 in the container, published on 127.0.0.1
by compose.

### Replicas

Several servers can answer at one address (Kubernetes, behind one
Service), with no session affinity: give them one `--data`, a volume they
all mount for writing (`ReadWriteMany`: NFS, EFS, CephFS), and each its
own `--recordings` on its own disk (an `emptyDir`). A token made or
revoked on one is made or revoked on all, and any of them answers for a
final pass another one runs. Their clocks must agree within seconds
(NTP). Why and how:
`docs/decisions/2026-10-29-replicas-share-the-data-folder.md`.

## The Android app

An experiment: the phone records the room and sends it to a
logbook-server, which transcribes it; the meeting goes to brainy and gets
its report from the server's model. No models on the phone. It keeps
recording with the screen off (a notification shows it, with Stop) and
finishes meetings left pending on its own while it is open. It is not on
the releases: build it (below) and `adb install -r` the APK. Why and how:
`docs/decisions/2026-10-29-android-app.md`.

On the phone: set the server's address (one the phone reaches, not
`127.0.0.1`) and your token in Settings; allow the microphone and the
notifications at the first recording, and "always run in background" when
asked (or Settings → Apps → Logbook → Battery → Unrestricted). On
Samsung's One UI, also take Logbook out of Settings → Battery → Background
usage limits → Sleeping apps and Deep sleeping apps. The tokens are kept
in a file in the app's private folder, which is weaker than the macOS
keychain. The microphone is opened for speech at a distance, its level
raised as far as it goes ("Raise faint voices", on by default).

## Build from source

Rust (a cargo workspace), and pnpm for the app's pages.

```bash
cargo build && cargo test
scripts/app.sh dev       # the app from this checkout, its pages reloading as they change
scripts/app.sh cert      # once per Mac: a self-signed signing certificate
scripts/app.sh build     # Logbook.app and a .dmg in target/release/bundle/
scripts/app.sh windows   # the Windows installer (x64), cross-built on this Mac
scripts/app.sh android   # the Android app (arm64), a debug APK
```

- **macOS**: the app is signed with the certificate `scripts/app.sh cert`
  makes, which keeps macOS's permissions (microphone, the call's audio)
  from one build to the next; `LOGBOOK_SIGNING_IDENTITY` picks another one.
- **Windows**: needs `brew install llvm nsis` and `cargo install --locked
  cargo-xwin` on the Mac that builds it; the installer is in
  `target/x86_64-pc-windows-msvc/release/bundle/nsis/`.
- **Android**: the Android SDK and NDK and a JDK (`brew install --cask
  android-commandlinetools` and `brew install openjdk@21`, then
  `sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0"
  "ndk;27.2.12479018"` with `ANDROID_HOME` at `~/Library/Android/sdk`). The
  APK is `apps/logbook/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.

How it works: `docs/pipeline.md` (the pipeline), `docs/protocol.md` (the
server's API), `docs/app.md` (the app), `docs/decisions/` (why).

### Releasing

```bash
scripts/release.sh 0.2.0
gh release edit v0.2.0 --draft=false     # once the draft is checked
```

`scripts/release.sh X.Y.Z` sets the version, builds the `.dmg` and the
Windows installer, commits "Logbook X.Y.Z", tags `vX.Y.Z`, pushes both and
makes a draft release with the installers and what the installed apps
update from (`latest.json`, the macOS bundle as a `.tar.gz`, the Windows
installer, each signed with the updater's key). The apps are offered a
version only once its release is published. It runs from a clean `main`
equal to `origin/main`, on the Mac that holds two keys, never committed:

- the macOS signing certificate (in the login keychain): every release
  must be signed with the same one, or macOS asks every user for the
  microphone and the call's audio again;
- the updater's key, `~/.config/logbook/updater.key`
  (`LOGBOOK_UPDATER_KEY`), whose public half is in
  `apps/logbook/src-tauri/tauri.conf.json`: lost, the apps already
  installed can never be updated again. Keep a copy somewhere safe.

A build without the updater's key has no update files (a build to try,
not one to release).

## Licences

Code: parakeet.cpp and ggml MIT; the server's admin page carries the
Geist typeface, SIL OFL 1.1 (`crates/server/src/assets/geist-OFL.txt`);
the CUDA build of libparakeet carries NVIDIA's cudart and cuBLAS under
their redistributable EULA. Weights: parakeet-tdt-0.6b-v3 CC-BY-4.0
(attribution: NVIDIA), Nemotron-3-Diarization OpenMDW-1.1, Silero VAD MIT,
WeSpeaker ResNet34-LM CC-BY-4.0 (attribution: the WeSpeaker project;
trained on VoxCeleb2). Each bundle of the mirror carries a `NOTICE.md`
with its sources.
