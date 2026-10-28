# The client keeps the meeting until it is published

**Verdict.** A meeting with a final pass is written to the client's disk
as it is captured, and stays there until its transcript is published. The
final pass is no longer tied to the WebSocket: it has an id, its result
is kept on the server for an hour (`GET /finals/{id}`), and a client
whose live session did not hear the whole meeting uploads its own copy
(`POST /finals`, Opus). A lost connection reopens the live session; a
server that cannot be reached leaves the meeting pending for
`logbook finish`. This replaces "a lost connection loses the result" of
`2026-10-24-final-pass-on-the-live-server.md`.

Since `2026-10-28-meetings-of-hours-and-days.md` the pass is taken up
from where its segments stopped instead of read again from the start,
and the client keeps the audio only from there.

## What was lost before

The only complete copy of the audio was on the side that could go away:

- the network drops during the meeting: the server had the audio up to
  the drop, the session ended, the CLI exited and stopped recording;
- the connection drops during the final pass: the result had nowhere to
  go, and the recording was deleted anyway;
- the server dies or restarts: the recordings in its temporary folder
  were lost (and left on disk, undeleted).

The client captures the audio in any case; keeping it costs a file.

## How it works

- **Pending folder** (`crates/cli/src/pending.rs`): `meeting.json` (the
  channels, the start, how to publish, never the brainy token), a raw
  file per channel on the session's clock, `final.json` once the result
  is back (the audio is deleted then), `transcript.md` if brainy refused
  the page. The folder is removed once the transcript is published.
- **Two ways to the final pass.** A live session that lasted from the
  first sample to `stop` has the whole meeting on the server: the server
  starts the pass, says its id in `finalizing`, and the client keeps the
  id at once (a client killed now fetches it later). A session that broke
  did not hear everything: the server drops its recording, and the client
  uploads its copy. No sample counting is needed: a WebSocket that
  reaches `stop` delivered every frame before it, in order.
- **Reconnection.** On a lost connection the CLI keeps capturing and
  writing, retries at 2, 4, 8… up to 30 s, and opens a new session
  without a final pass; its times are shifted by where it started on the
  meeting's clock. What was said in between has no live text, and the
  new session numbers the speakers afresh: the final pass covers both.
- **Upload in Opus** (`crates/engine/src/codec.rs`): 32 kbit/s, 14 MB an
  hour a channel instead of 115. A format of our own (header with the
  sample count and the encoder's lookahead, length-prefixed packets), so
  the decoded audio lines up with the original sample for sample: the
  channels' clocks are compared by the echo filter.
- **The server forgets on restart**: results are in memory. A client
  whose id is unknown uploads again; the leftover recordings of a server
  that died are deleted when it starts.

## Costs

- **The audio is on the laptop** for the whole meeting and until it is
  published: 32 KB a second a channel, in the user's data folder, in the
  clear. Encrypting it with a key held in memory would make a crashed
  client's copy unreadable, which is the case this exists for.
- **A broken meeting uploads at its end**: 28 MB for an hour with two
  channels, a few seconds on an office network, minutes on a bad one.
  An unbroken one uploads nothing.
- **An id is the key to a transcript**: 128 random bits, kept an hour,
  with no other authentication (none of the prototype's endpoints has
  any). If the connection drops between `stop` and `finalizing`, the
  client uploads and the server runs the pass twice.
- Opus at 32 kbit/s changed one word on the LibriSpeech fixture ("Mr." to
  "mister"): speakers, segments and times as the PCM.

## Rejected

| What | Why not |
|---|---|
| Keep only the result on the server, by id | covers the drop during the final pass, not the commonest one: the network dropping during the meeting |
| Always upload the client's copy | doubles the traffic of every meeting to cover the broken ones; the server already has the audio of an unbroken one |
| Resume the live session, sending the missed audio | the server would transcribe minutes of backlog at once; text that late is not live, and the final pass has it anyway |
| Ogg Opus, FLAC | Ogg's framing for no reader but ours; FLAC halves PCM at best |

## Measured, 2026-10-24

Dev Mac, LibriSpeech fixture (23.6 s), `play --final`:

- server killed 9 s in and kept down: the live text stopped, the CLI
  retried, kept recording, failed to upload and left the meeting pending;
  server started again, `logbook finish` uploaded 0.1 MB and printed the
  four turns with both speakers right (pass: 0.9 s);
- server restarted 4 s in: the CLI connected again after about 6 s, the
  later phrases came live at their meeting times (+0.5–1.4 s), and the
  final pass over the upload was complete;
- CLI killed with SIGKILL during the final pass: `logbook finish` fetched
  the result by the kept id, without uploading;
- brainy unreachable: `final.json` and `transcript.md` kept, the audio
  deleted, `finish` tried again with the address saved in `meeting.json`.
