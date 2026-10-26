# The CLI transcribes the meeting itself; the server is optional

**Verdict.** Without `--server` (`LOGBOOK_SERVER`) the CLI loads the
models and runs the meeting's session in its own process: the same
session, batcher, finalizer and polisher as logbook-server, which moved
from `crates/server` into `crates/engine` for it. The CLI drives them
through channels (`crates/cli/src/local.rs`), the server through its
WebSocket. With `--server` nothing changes. This is the "later local
mode on the same engine" of
`2026-10-24-live-server-with-pseudo-streaming.md`; the reasons for a
server there still hold for clients that cannot keep up.

## Why

- **No server to run** for one person on one laptop: one command, the
  models fetched from the mirror on first use, as the server does.
- **Apple silicon keeps up**: on an M3 (Metal), a recording played at
  four times the pace of speech had its text 0.4–0.55 s after each
  phrase, against about a second through a server on the same machine.

## How

- **One session, two drivers.** `session::run` takes the same inputs
  (audio, stop, close) and sends the same `ServerMessage`s; the CLI hands
  them to `client::handle`, the function its WebSocket session uses. A
  copy of the session in the CLI would have been a second place for the
  rules that fail silently (one part with the clean-up model at a time,
  final segments sent once).
- **The audio stays in the pending folder.** A local session records its
  final pass's audio there, not in a temporary folder; `finish` without a
  server copies the kept audio there for the pass. Both are deleted as
  the pass reads them, and the folder once the meeting is published.
- **Models before audio.** `play`, `live` and `bench` load the models
  before anything is recorded: on Linux, finding the CUDA build's
  libraries starts the process again (`mirror::beside_its_libraries`),
  which must not happen with a meeting under way. `--polish` without a
  model (`LOGBOOK_POLISH_URL`, `LOGBOOK_POLISH_MODEL`) is refused then too.
- **No registry credentials.** The three packages logbook fetches
  (`models/parakeet`, `models/parakeet-voice`, `models/libparakeet`) were
  made public, their licences read for redistribution (each bundle's
  `NOTICE.md`): a token per person is no way to install a CLI. The
  token options of 2026-10-25-the-server-fetches-its-models.md
  (`LOGBOOK_REGISTRY_TOKEN`, `LOGBOOK_REGISTRY_USERNAME`) are gone from the
  server and the CLI; the mirror asks the registry for an anonymous token.
- **ggml's log keeps only warnings and errors** (`ggml_log` in
  parakeet.rs, server and CLI alike). Its info and debug lines, a hundred
  as Metal compiles its kernels while the meeting starts, went to the
  terminal the transcript is printed in, between its lines. libparakeet's
  C API has no call for it; `ggml_log_set` is ggml's, exported by
  libparakeet, declared from `ggml.h` at the ggml commit parakeet.cpp
  v0.6.1 pins (e705c5fe). A build without the symbol keeps ggml's default.
- **Defaults in one place.** The model files, `MIN_PAUSE`, `CONTEXT`,
  `VOICE_THRESHOLD`, `MAX_BATCH` and the clean-up timeout are engine
  constants that the server's options and the CLI share.

## Not done

- **libparakeet linked into the binary**: the release ships prebuilt
  builds per platform and GPU; the models come from the mirror anyway.
- **The final pass reading the pending audio directly**: the session
  records it a second time (minutes of it, deleted as the pass goes) to
  keep one code path with the server.
- **No reconnection or upload**: in one process there is no connection
  to lose. A meeting left pending by a server can be finished locally,
  and the other way round: a local pass id is unknown to a server, which
  then gets the audio uploaded.
