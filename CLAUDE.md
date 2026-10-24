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
                 one channel's pipeline (channel.rs), the wire protocol (protocol.rs)
                                                                → docs/pipeline.md
crates/server/   logbook-server: WebSocket /live (axum), a thread per session,
                 one batcher thread for every transcription      → docs/protocol.md
crates/cli/      logbook: play (a file at the pace of speech), live (microphone and,
                 with --system, the call), devices, bench (many sessions at once)
scripts/         server.sh (fetches libparakeet and the models, runs the server), client.sh
vendor/          libparakeet for this machine, fetched by scripts/server.sh (not committed)
docs/            pipeline.md, protocol.md, decisions/
```

## Commands

```bash
cargo build && cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
scripts/server.sh                     # PARAKEET_VERSION and MODELS_REVISION pinned inside
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
  silence. macOS's system tap delivers nothing while nothing plays; without
  it the call's times drift from the mic's and pauses vanish
  (`docs/decisions/2026-10-24-system-audio-through-cpal.md`).
- **Same models as brainy-worker** (`MODELS_REVISION` = its
  `audio_revision`): one download, one set of licences
  (brainy-worker's `docs/decisions/2026-10-10-licences-of-code-and-weights.md`).
