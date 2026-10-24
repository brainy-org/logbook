# The live pipeline

What happens to a channel's audio between the socket and an utterance.
Code: `crates/engine/src/channel.rs`, `crates/server/src/{session,batcher}.rs`.

```
audio ──▶ VAD stream (Silero) ── phrase closed at a pause ──▶ batcher ──▶ tdt-0.6b-v3
   │                                                                        │
   └──▶ diarization stream (Nemotron-3, 1.04 s) ──▶ turns                   │
                                     │                                      ▼
                                     └────────▶ speaker per word ◀── transcript
                                                     │
                                                     ▼
                                     utterances, in order, one per speaker run
```

## Phrases

- The VAD stream (`min_pause` 0.15 s, `min_speech` 0.25 s, `speech_pad`
  0.05 s) reports every pause from 0.15 s on; `Phrasing` decides which
  close a phrase: a pause of `--min-pause` (0.5 s), or any pause once the
  phrase is `SOFT_PHRASE` (6 s) long. A phrase still without a pause at
  `MAX_PHRASE` (20 s) is cut where it stands; a word on the cut can be lost
  or split. Every word waits for its phrase's end: continuous speech (a
  talk, a video) closed only at 0.5 s pauses made words wait up to 20 s.
- A clip is the phrase plus `CLIP_PAD` (0.1 s) on both ends.
- The channel keeps audio only back to the open phrase's start minus
  `KEEP` (2 s): a session of hours holds seconds of audio.

## Transcription

The ASR model (tdt-0.6b-v3) is offline, not a streaming model: it reads
whole phrases, which is why text arrives per phrase rather than per word.
The batcher thread owns every call to it. It takes the first phrase
waiting and whatever else is queued (up to `--max-batch`), and transcribes
them in one call; a phrase never waits for a batch to fill. The log line
`batch` gives the phrases, the audio, the compute time and the longest
wait in the queue of each call.

## Speakers

- Each diarized channel has its own diarization stream; streams are
  spread over `--diarization-contexts` copies of the model, and calls on
  one copy are serialized (`Model::serial`).
- A phrase leaves only once the stream's diarized time has reached its
  end: diarization trails the audio by up to its latency (1.04 s in mode
  `low`), so on a diarized channel the latency is about the larger of that
  and the transcription.
- Each word goes to the speaker whose turns (ended, plus those still open)
  overlap it most, or to the nearest turn up to 1 s away: the rule of
  brainy-worker's `_speaker_of`. Turns older than the oldest phrase still
  to place, minus 10 s, are dropped.

## Echo

A channel with `echo_of` (the mic, when the client records the call too
and the person has no headphones) is filtered against that channel
(`crates/engine/src/echo.rs`). Its utterances are held until the other
channel has sent everything up to their end (`Channel::settled`); then
each word that the other channel said within 0.8 s, as the same word or
one or two letters off in a long word, is dropped. Of an utterance with
echo, only runs of at least 3 words of its own are kept (someone talking
over the call); an utterance without echo is untouched.

- It needs both channels on one clock: the client's `Timeline`.
- While the call speaks, the mic waits for the call's phrase to close
  (seconds); when the call is quiet, it leaves at once.
- It compares words, so it fails where both copies are transcribed badly
  and differently (a robotic synthetic voice left fragments); real echo
  cancellation on the client is the step after.

## Measured

On the dev Mac (M3, Metal), one session: the text of a phrase arrives
0.5–1.4 s after its last word with diarization, 0.5–1.0 s without. Load
tests on this machine say little about a server: see the decision record.
