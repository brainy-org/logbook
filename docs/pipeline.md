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

- The VAD stream (`min_pause` 0.5 s, `min_speech` 0.25 s, `speech_pad`
  0.1 s) reports where speech starts and ends. An end is known once the
  pause has lasted `min_pause`: that is the floor of the latency.
- A phrase is the audio from a start to its end. One longer than
  `MAX_PHRASE` (20 s) is cut where it stands and goes on as a new phrase;
  a word on the cut can be lost or split.
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

## Measured

On the dev Mac (M3, Metal), one session: the text of a phrase arrives
0.5–1.4 s after its last word with diarization, 0.5–1.0 s without. Load
tests on this machine say little about a server: see the decision record.
