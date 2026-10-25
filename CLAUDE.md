# logbook

Live meeting transcription with speakers: a server that holds the models
(`logbook-server`) and a client that streams the audio to it (`logbook`, a
CLI for now; a desktop app later on the same engine). parakeet.cpp through
its C API, like brainy-worker, with the same model files. Rust, a cargo
workspace. A prototype: why it exists and how it is built is in
`docs/decisions/2026-10-24-live-server-with-pseudo-streaming.md`.

## Layout

```
crates/engine/   the engine, a library: libparakeet loaded at run time (parakeet.rs),
                 one channel's pipeline (channel.rs), echo (echo.rs), the final pass
                 as the meeting goes (progressive.rs) or over an upload (final_pass.rs),
                 the wire protocol (protocol.rs), the upload's Opus (codec.rs),
                 the final transcript cleaned up by a language model (polish.rs)
                 examples/diarize.rs: diarization strategies scored with
                 scripts/der.py                                    → docs/pipeline.md
crates/server/   logbook-server: WebSocket /live and HTTP /finals (axum), a thread per
                 session, one batcher thread for every transcription, recordings
                 (recording.rs), one finalizer thread for final passes, their results
                 by id (finals.rs), the threads that ask the clean-up model
                 (polisher.rs)                                     → docs/protocol.md
crates/cli/      logbook: play (a file at the pace of speech), live (microphone and,
                 with --system, the call; its echo cancelled from the mic: aec.rs),
                 finish (pending meetings), devices, bench;
                 the session with reconnection (client.rs), the meeting kept until
                 published (pending.rs), the final transcript as Markdown, a part
                 at a time (transcript.rs), into brainy as it comes (brainy.rs)
scripts/         server.sh (fetches libparakeet and the models, runs the server), client.sh
vendor/          libparakeet for this machine, fetched by scripts/server.sh (not committed)
docs/            pipeline.md, protocol.md, decisions/
```

## Commands

```bash
cargo build && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
scripts/server.sh                     # libparakeet and the models, by sha256, from the mirror
scripts/client.sh play FILE           # or: live [--system], devices, bench FILE --sessions N
```

## Invariants

- **The pipeline is `crates/engine`; the server and the client only drive
  it.** Pipeline logic in `crates/server` is logic the future local mode
  lacks.
- **libparakeet is loaded at run time and its ABI is checked at open**
  (`ABI` in `parakeet.rs`, v0.6.1). A function declared with the wrong
  signature does not fail to build: it corrupts memory. Declare from
  `parakeet_capi.h` of the pinned release, nothing from memory.
- **A diarization context runs one call at a time**: every call on it goes
  through `Model::serial`. Two sessions on one context without it pass a
  light test and crash or mix speakers under load.
- **Phrases leave a channel in the order they were spoken**, and only once
  diarization has reached their end (`Channel::ready`). Sending a phrase
  before then gives its words the speaker of an earlier turn.
- **Every live channel is on the session's clock** (`Timeline`, cli
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
  until the final transcript is back (`pending.rs`). Audio written
  anywhere else outlives the meeting, which the README promises it does
  not (`docs/decisions/2026-10-24-the-client-keeps-the-meeting.md`).
- **A live session's times are the meeting's**: after a reconnection the
  CLI shifts them by where the new session started (`client.rs`). The
  upload's codec keeps the sample count and drops Opus's lookahead, for
  the same reason: channels are compared by time.
- **Final segments are sent once and never change**: up to the earliest
  cut of all channels, by start (`Progressive::ready`). The CLI appends
  them to the brainy page and records how far it got
  (`published_until`); sending a segment again, or one that starts
  before `until`, writes it twice or out of order on a page nobody
  rewrites (`docs/pipeline.md`, "Final pass").
- **A final range's speaker slots are its own**: each range is diarized
  alone, so slot 0 of one range and of the next are unrelated; only
  voices (`speakers::Linker`) make them the meeting's speakers. Carrying a
  slot number across ranges mixes people up silently
  (`docs/decisions/2026-10-24-speakers-across-a-long-meeting.md`).
- **A session has one part with the clean-up model at a time**
  (`Final::polishing`, server `session.rs`): final segments leave in the
  order they were ready. Two parts out at once can come back swapped and
  be appended to the page out of order, and the second misses the first
  as context (`docs/pipeline.md`, "Final pass", step 6).
- **Times that go out as JSON are never infinite**: serde_json writes
  infinity as `null`, which does not read back as a number. "To the end"
  is `f64::MAX` (`until`, `published_until`).
- **logbook-server holds no brainy credentials.** It returns the final
  transcript; the CLI writes the page with the person's own token
  (`docs/decisions/2026-10-24-final-pass-on-the-live-server.md`).
- **Same models as brainy-worker, from the same mirror**: the `parakeet`
  bundle of github.com/brainy-org/models (its files' sha256 in
  `scripts/server.sh`, the same as brainy-worker's `models.json`), and
  libparakeet from its `libparakeet` bundle; nothing from GitHub releases or
  Hugging Face. One set of licences
  (brainy-worker's `docs/decisions/2026-10-10-licences-of-code-and-weights.md`).
