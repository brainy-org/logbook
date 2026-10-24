# The final pass runs on logbook-server; the client writes the page

**Verdict.** When a meeting ends, logbook-server reads its whole recording
again, channel by channel, the way brainy-worker reads a recording, and
returns the final transcript over the session's WebSocket. The CLI turns
it into Markdown and creates a page in brainy with the person's own
personal token (`POST /api/v1/pages`). The server keeps the audio only for
this, in files that delete themselves once read.

## Why not through brainy-worker

brainy-worker reads recordings into `extract/v2`, but brainy does not ask
for v2 and never sends it one (Galley's
`docs/decisions/2026-10-23-recordings.md`: "Brainy asks for nothing, gets
v1, and never dispatches a recording"). Teaching brainy to import audio
(v2 in the claim, segments into blocks, a player) is the largest of the
options and is not needed for a meeting page. brainy-worker also takes
one mixed file: the microphone (the person recording) and the call would
be one channel, and "who is me" would be lost, or need a new extract
version released from brainy-worker.

logbook-server already has the models loaded and receives every channel
separately on one clock. The pass is a few calls on the same library
(`parakeet_capi_vad_pcm_json`, `_transcribe_pcm_batch_json`,
`_diarize_pcm`) with brainy-worker's segment rules ported, so the result
reads like brainy-worker's.

## Why the client writes the page

- The page is written by the person's token: it lands only where they
  can write, and brainy's permissions decide, not a service account.
- logbook-server stays a transcription service with no knowledge of
  brainy and no credentials to keep: the same server could feed Galley.
- Cost: the client must stay connected until the pass is done (seconds
  to a few minutes); a lost connection loses the result, and the
  recording is deleted anyway. If brainy refuses the page, the CLI saves
  the Markdown next to the person instead of dropping it.

## The pass

Each channel is read whole (one in memory at a time: 230 MB an hour),
cut by Silero VAD into pieces of at most 30 s, transcribed 8 at a time,
and, if diarized, diarized in one call on a diarization context reserved
for final passes (one call holds it for the whole recording; a live
stream's context would stall that stream). Passes run one at a time on a
thread of their own, so they take at most one pass's worth of GPU from
the live sessions. The mic's echo is filtered as live, over the whole
meeting.

## Measured, 2026-10-24

On the dev Mac (M3, Metal), `play --final`:

- LibriSpeech fixture, 23.6 s: the pass took 1.0 s; four segments with the
  two readers right; a word the live cut had split ("said") is gone.
- 45 s in Italian with macOS voices: 1.7 s. The natural voice exact; of
  the robotic formant voice, which live transcribed (badly), the final
  pass kept only "Grazie Alice": in pieces of up to 30 s mixed with a
  natural voice the model drops it, as brainy-worker's record observed.
  Real voices are to be checked on real meetings.
- brainy reached through a mock of `/api/v1/pages` (bearer token, title,
  icon, `parent_id`, the Markdown); with brainy unreachable the
  transcript was saved as `logbook-….md`. Not yet tried against a running
  brainy.
