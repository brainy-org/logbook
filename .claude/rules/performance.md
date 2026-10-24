# Performance and scaling

Check how a change performs on a busy server before proposing it, not
after being asked. Every proposal states its cost at scale: tens of live
meetings on one server at once, meetings of several hours, clients on a
slow or unsteady network.

- **Per sample**: what runs for every audio frame (16 000 samples a second,
  per channel, per session) multiplies by all of them. No allocation, copy
  or lock per sample; per frame only what must be.
- **Per session**: what a session keeps grows with the meeting. Audio is
  kept only back to the oldest point still needed (`KEEP`), turns only
  back to the oldest phrase still to place: never the whole recording.
- **Shared models**: the GPU and the diarization contexts are shared by
  every session. A call made per session that could be made per batch
  (transcription) is the difference between ten meetings on a GPU and a
  hundred; a lock held across a long call stalls every session behind it.
- **Latency is the product**: a change that adds time between a phrase's
  end and its text says how much, measured with `logbook bench`.

If something does not scale, say so in the proposal and give the
alternative.
