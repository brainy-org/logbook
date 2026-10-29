# A server on a CPU falls behind: the live transcript first, a client gone drops its session

**Verdict.** A session with audio queued (more than 2 s, `HOLD_FINAL` in
engine `session.rs`) asks for no progressive range: the final pass waits
and catches up at the end. A client gone before the end of its audio (or
gone without a final pass to complete) drops its session at once, the
audio queued for it included: the news reaches the session through its
`Feed` (shared counters), not its inbox, where it would wait behind
that audio. A socket that brings nothing for 30 s before the `stop` is a
client gone (`SILENT` in the server's `main.rs`).

## What happened

logbook-server in Docker on a laptop's CPU (i7-1255U under WSL, no GPU)
for the Android app in a room (one channel, diarized), 2026-10-29:

- Each live phrase took 0.5-0.9 of its length to transcribe (after the
  laptop's power mode was raised, 0.25-0.40), and the batcher had no
  queue (`max_wait_ms` 0). Yet the phone showed the live transcript at
  2:45 eight minutes into the meeting: the audio waited before the
  channel (VAD, streaming diarization), in the session's inbox.
- Live transcription, live diarization and the progressive pass ran at
  once, each with libparakeet's compute threads on every core. Its
  header says so: "the compute thread teams spin, and oversubscribing the
  machine slows everything". A GPU hides it; a CPU does not. The final
  pass's ranges came every 4-7 minutes and fell behind as the live one
  did: their cuts are the live channel's quiet points.
- The phone's connection dropped; it took the meeting up in a new session
  (`offset` 328.6 s). The old session never ended: `Closed` sat in its
  inbox behind minutes of audio, which it went on transcribing for
  nobody, beside the new one. The new one's first range took 70 s for
  46 s of audio.

## Tried, on a Mac with `PARAKEET_DEVICE=cpu`

Six `bench` sessions and one `play --final` at once:

- the `play` session fell 32 s behind; its final pass logged "the live
  transcript is behind: the final pass waits";
- killed with its audio queued, its session logged "the client has gone:
  the meeting is dropped" within 1 s;
- frozen (SIGSTOP: the socket open, nothing sent), its session was
  dropped 30 s later;
- the six `bench` sessions, killed after their `stop` with audio still
  queued and no final pass, were dropped within 0.3 s;
- a meeting of 5 minutes with its final pass, unloaded, completed as
  before.

## Not done

- One session's pass yields to its own live transcript only: another
  session's final ranges still take the cores. A finalizer that waits
  while any session is behind would cover that.
- Dividing the cores between the models (libparakeet's
  `parakeet_capi_set_concurrency` for transcription, `threads_each`)
  would cut the spinning itself; not measured.
- `--context 0` saves the 3 s of audio transcribed before each phrase
  (some 30-40% of the live transcription on a CPU), at some cost to the
  language of short phrases; not measured.
