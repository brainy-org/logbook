# logbook

Live meeting transcription with speakers: a server that holds the models
(`logbook-server`) and a client that streams the audio to it or runs the
models itself: the `logbook` CLI and a desktop app (Tauri, which also
builds for Android as a recorder for a server), both on one client
library. parakeet.cpp through
its C API, like brainy-worker, with the same model files. Rust, a cargo
workspace. A prototype: why it exists and how it is built is in
`docs/decisions/2026-10-24-live-server-with-pseudo-streaming.md`.

## Layout

```
crates/engine/   the engine, a library: libparakeet loaded at run time (parakeet.rs),
                 one channel's pipeline (channel.rs), echo (echo.rs), the final pass
                 as the meeting goes (progressive.rs) or over an upload (final_pass.rs),
                 what its diarization hears: band-passed, levelled when quiet (conditioning.rs),
                 the wire protocol (protocol.rs), Opus for the live socket and
                 the upload (codec.rs),
                 the meeting's report written by a language model (report.rs,
                 its endpoint: llm.rs),
                 libparakeet and the models from the mirror (mirror.rs),
                 files replaced whole in a folder replicas share (store.rs);
                 a meeting's session, one thread each (session.rs), and what sessions
                 share: the batcher thread for every transcription (batcher.rs), the
                 finalizer thread for final passes (finalizer.rs), their audio
                 (recording.rs) and results by id (finals.rs)
                 examples/diarize.rs: diarization strategies scored with
                 scripts/der.py                                    → docs/pipeline.md
crates/server/   logbook-server: the engine's sessions over WebSocket /live, final
                 passes over HTTP /finals, reports /reports (axum), each behind a
                 person's token made on the admin page at / (auth.rs)
                                                                   → docs/protocol.md
crates/client/   the client, a library the CLI and the app share: the audio (audio.rs,
                 the call's echo cancelled and the mic's level raised: aec.rs), the
                 session on a server with reconnection (remote.rs) or in this process
                 (local.rs), the meeting from its first sample to its page (meeting.rs):
                 kept until published (pending.rs) and finished later, waiting for
                 the server if need be (finishing.rs), the final transcript as Markdown
                 (transcript.rs), the report as a page in brainy (brainy.rs: never
                 the transcript), speakers named from voices known on this computer
                 (voices.rs), and for the app the meetings kept (history.rs) and the
                 screenshots in their notes (shots.rs); a
                 recording cut into meetings at a long pause or a meeting's length
                 (series.rs: every channel cut at one sample)
crates/cli/      logbook: play (a file at the pace of speech), live (microphone and,
                 with --system, the call), finish (pending meetings), speakers,
                 devices, bench: options and printing only
apps/logbook/    the app, macOS, Windows and Android: pages (React, brainy's look) and src-tauri/
                 (recorder, pending meetings finished and their progress, settings and
                 their secrets, logs, screenshots for the notes; android.rs and the
                 Android project in gen/android); plugins/recording: the Android
                 foreground service, wake lock and permissions (Kotlin)  → docs/app.md
scripts/         server.sh (builds and runs the server), client.sh, app.sh (the app:
                 dev, cert, build, windows, android); der.py, room-der.sh (every
                 diarization strategy scored on one recording) and reference.py
                 (a reference made by hand in Audacity)
                                                → docs/measuring-a-room-microphone.md
Dockerfile       the server's image, CPU and GPU alike (compose.yaml, compose.gpu.yaml)
docs/            pipeline.md, protocol.md, app.md, decisions/
```

## Commands

```bash
cargo build && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
scripts/server.sh                     # fetches libparakeet and the models on first start
scripts/client.sh play FILE           # or: live [--system], devices, bench FILE --sessions N
scripts/app.sh dev                    # the app; `build` signs it (`cert` once per Mac), `android`
```

## Invariants

- **The pipeline is `crates/engine`; the server and the client only drive
  it.** Pipeline logic in `crates/server` is logic the CLI without
  `--server` lacks (local.rs runs the engine's session, as the server does).
- **A meeting's run is `crates/client` (`meeting.rs`; a recording's
  meetings, `series.rs`); the CLI and the app only choose its options.**
  A step added in the CLI's `main.rs` or the app's `recorder.rs` is
  missing from the other, and from `finish` for a meeting left pending
  (`docs/app.md`).
- **libparakeet is loaded at run time and its ABI is checked at open**
  (`ABI` in `parakeet.rs`, v0.6.1). A function declared with the wrong
  signature does not fail to build: it corrupts memory. Declare from
  `parakeet_capi.h` of the pinned release (beside the library in
  `~/.cache/brainy-org/libparakeet-<build>-<sha>/`), nothing from memory;
  the one ggml call (`ggml_log_set`) from `ggml.h` at the ggml commit that
  release pins.
- **A diarization context runs one call at a time**: every call on it goes
  through `Model::serial`. Two sessions on one context without it pass a
  light test and crash or mix speakers under load.
- **Phrases leave a channel in the order they were spoken**, and only once
  diarization has reached their end (`Channel::ready`). Sending a phrase
  before then gives its words the speaker of an earlier turn.
- **Every live channel is on the session's clock** (`Timeline`, client
  `audio.rs`): sample 0 is the session's start, gaps are filled with
  silence, and a device that delivers nothing still moves along the clock
  (`Timeline::idle`). macOS's system tap delivers nothing while nothing
  plays; without it the call's times drift from the mic's, pauses vanish,
  the echo filter (`echo.rs`, which matches words by time across
  channels) drops nothing, and with nothing playing it holds every mic
  word until the end (`docs/decisions/2026-10-24-system-audio-through-cpal.md`).
- **On the server, a meeting's audio lives only in a `Recording` (or the
  `RecordedFile` of an upload), which deletes its files when dropped**
  (at the session's end, whatever it was; after an upload's pass) and as
  the progressive pass passes them (`forget_before`), or is cleared at
  start if the server died. On the client, only in its pending folder,
  from where the final segments have come (`final_until`) until the final
  transcript is back (`pending.rs`). Audio written anywhere else outlives
  the meeting, which the README promises it does not
  (`docs/decisions/2026-10-24-the-client-keeps-the-meeting.md`).
- **A live session's times are the meeting's**: one that takes a final
  pass up (after a lost connection, or an upload) is given its `offset`
  and the server moves every time it sends; one without a final pass is
  moved by the client (`remote.rs`). Opus, on the live socket as in an
  upload, keeps the sample count and drops its lookahead, for the same
  reason: channels are compared by time (docs/protocol.md, "The audio").
- **A final pass is taken up where its segments stopped, never started
  again** (`final_until`, with the speakers' voices): one started again
  reads the whole meeting, which for a meeting of hours costs as much as
  the meeting and holds it on disk until the end
  (`docs/decisions/2026-10-28-meetings-of-hours-and-days.md`).
- **Final segments are sent once and never change**: up to the earliest
  cut of all channels, by start (`Progressive::ready`). The client
  appends them to `segments.jsonl`; a segment sent again, or one that
  starts before `until`, is in the transcript twice or out of order
  (`docs/pipeline.md`, "Final pass").
- **The brainy page holds the report, never the transcript**: made once
  the meeting is finished, its header and the report (or the notes)
  under it (`meeting.rs`, `new_page`); the transcript stays on the
  client (`docs/decisions/2026-11-02-the-page-holds-the-report.md`).
- **A final range's speaker slots are its own**: each range is diarized
  alone, so slot 0 of one range and of the next are unrelated; only
  voices (`speakers::Linker`) make them the meeting's speakers. Carrying a
  slot number across ranges mixes people up silently
  (`docs/decisions/2026-10-24-speakers-across-a-long-meeting.md`).
- **What a meeting or a session keeps is bounded, never the whole
  meeting**: the final segments a session keeps (`KEEP_SENT`, the
  result's `from`), the speakers a linker keeps (`MAX_SPEAKERS`), what
  the publisher holds while brainy fails (none: `segments.jsonl`), and a
  meeting's length (`live` is cut into meetings, client `series.rs`).
  State that grows with the meeting fails only after hours or months, on
  a server with tens of meetings or a client left running
  (`docs/decisions/2026-10-29-bounded-meetings.md`).
- **What the client's side tells a session goes through its `Feed`, not
  only its inbox** (engine `session.rs`): audio counted (`Feed::audio`),
  the stop and the client gone flagged. News sent only into the inbox
  waits behind every minute of audio queued before it: a session behind
  the audio transcribes for a client long gone, or never yields to its
  live transcript
  (`docs/decisions/2026-10-29-a-server-on-a-cpu-falls-behind.md`).
- **Turn times are printed on the meeting's displayed clock: one offset,
  fixed when the final transcript is made** (`Writer::offset`, client
  `transcript.rs`), and kept with the writer. Shift only where times
  become text; an offset changed later, or applied to segments, prints a
  transcript with two clocks or moves `final_until`
  (`docs/decisions/2026-10-29-the-meeting-starts-at-its-first-word.md`).
- **Times that go out as JSON are never infinite**: serde_json writes
  infinity as `null`, which does not read back as a number. "To the end"
  is `f64::MAX` (`until`, `final_until`).
- **Voiceprints stay on the client.** The server sends a meeting's
  speakers' voices (`SpeakerVoice`) and keeps them only as long as that
  meeting's result. The library of named voices lives only in the CLI's
  data folder (client `voices.rs`). A library on the server would hold
  biometric data of people who never used logbook
  (`docs/decisions/2026-10-27-speakers-named-by-voice.md`).
- **What replicas share is a file per record, replaced whole** (engine
  `store.rs`): the data folder may be one volume for every replica, with
  no lock. A file several servers read, change and write back loses one
  server's change; a token's last use written into its file brings a
  revoked token back
  (`docs/decisions/2026-10-29-replicas-share-the-data-folder.md`).
- **Every route of a meeting is behind a person's token**: it goes in
  the `meetings` router of the server's `main.rs`, under
  `auth::require_token`. One added beside it answers anyone who reaches
  the server, and nothing fails to say so (docs/protocol.md, "Tokens").
- **A pending meeting is touched only by the process holding its
  `.lock`** (`Pending::create` while it is recorded, `Pending::claim` to
  finish it; `open` only reads). A `finish` that skipped the lock would
  upload and publish a meeting still being recorded, or publish one
  twice (client `finishing.rs`).
- **logbook-server holds no brainy credentials.** It returns the final
  transcript and the report; the client writes the page (the report) with the
  person's own token (`docs/decisions/2026-10-24-final-pass-on-the-live-server.md`).
- **The desktop app is built for macOS 11.0** (`minimumSystemVersion`):
  Tauri exports it as `MACOSX_DEPLOYMENT_TARGET`, and 14 or more makes
  macOS 27's linker write proc-macro dylibs rustc cannot load (E0463).
  Its hardened runtime needs `disable-library-validation`, or libparakeet
  is refused at `dlopen` (`docs/decisions/2026-10-27-desktop-app.md`).
- **The Android app links no model code**: engine `models` and client
  `local` (default features) are added by the app for non-Android
  targets only. A dependency or a `use` that pulls them into the Android
  build puts libparakeet's loader and the mirror on a phone that cannot
  run them, silently (`docs/decisions/2026-10-29-android-app.md`).
- **Same models as brainy-worker, from the same mirror**: the `parakeet`
  bundle of github.com/brainy-org/models (its files' sha256 in
  `crates/engine/src/pins.rs`, the same as brainy-worker's
  `models.json`), and libparakeet from its `libparakeet` bundle, fetched
  by the server itself; nothing from GitHub releases or Hugging Face. One set of licences
  (brainy-worker's `docs/decisions/2026-10-10-licences-of-code-and-weights.md`).
