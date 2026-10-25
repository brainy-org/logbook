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

The model picks the language of each clip by itself (it takes no
language hint: the `_lang` functions of the C API are for prompt models),
and a short phrase alone can come out in another one: "Ma va là." read
as "Mais voilà.", two people talking Italian shown as English and French.
So each clip carries up to `--context` seconds (3) of the channel's audio
before the phrase; the words centred before the previous phrase's end
(by the VAD) are that phrase's again and are dropped
(`Channel::ready`, `Phrase::boundary`;
`docs/decisions/2026-10-24-audio-context-for-the-language.md`).

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

Without headphones, the client first cancels the call from the mic's
audio (AEC3, `crates/cli/src/aec.rs`,
`docs/decisions/2026-10-24-echo-cancellation-on-the-client.md`): the
server gets the mic mostly without the call. What is left is filtered by
words.

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
  and differently (a robotic synthetic voice left fragments), and it
  cannot give back words of the person's that the call's covered: that
  is the canceller's job.
- `RUST_LOG=logbook_server=debug` logs every mic utterance with echo as
  the filter saw it, the words it kept in [brackets]: the meeting's words,
  in the server's log, only at debug.

## Final pass

The final pass follows the meeting instead of reading it once it ends
(`crates/engine/src/progressive.rs`), so what is left at the end is the
last minute or so, whatever the meeting's length, nothing reads a whole
meeting at once, and the speakers are not limited to the diarization
model's eight
(`docs/decisions/2026-10-24-speakers-across-a-long-meeting.md`). With
`final_pass`, the session records each channel as it arrives
(`crates/server/src/recording.rs`: raw s16le in `--recordings`, 32 KB a
second per channel), and:

1. **Ranges**: the live channel tells where it is quiet
   (`Channel::quiet_point`: where the VAD last heard speech end, or a
   little before now in silence; never inside speech). Once a channel has
   45 s of new audio, its range from the last cut to the latest quiet
   point within 60 s (or, in speech that long, the first one after it)
   goes to the finalizer thread, one job at a time for every session.
2. **Words**: the finalizer reads the range from the recording, cuts it
   with Silero VAD into pieces of at most 30 s and transcribes them 8 a
   call.
3. **Speakers**: a diarized range is diarized alone (Nemotron-3, offline,
   on the final passes' context): at most eight voices in a minute, which
   the model's eight slots hold. Each slot's voice, from up to 30 s where
   it speaks alone, is a WeSpeaker embedding, matched to the meeting's
   speakers by cosine: above 0.45 the same speaker; two slots of one range
   never one speaker; only a voice of 4 s or more founds a new speaker (a
   shorter one joins the most similar above 0.2, or is no one); a slot
   heard alone for under 1.5 s has no voice, and its words go to the
   nearest turn (`crates/engine/src/speakers.rs`). A range's slots are its
   own, unrelated to the next range's: only voices link them, so the
   encoder (`--voice`) is required. The meeting has no limit of eight, and
   its numbering holds from the first minute to the last.
4. **Segments**: words become segments with brainy-worker's rules (a
   change of speaker, a pause of 1.2 s, a sentence end past 8 s, 30 s at
   most), and are sent as `final_segments` up to the earliest cut of all
   channels, so they come in order; a channel with `echo_of` stops 2 s
   before the cut of the channel it echoes, whose words the echo filter
   must have heard.
5. **The end**: after `stop`, every channel's rest is cut in ranges the
   same way; then `final` and `done`. The session lives until then even
   if the client has gone, and the result is kept under the `finalizing`
   id.

6. **Clean-up** (with `--polish-url` and `--polish-model`): before they
   are sent, the segments go to a language model that removes fillers,
   false starts and the words a speaker took back, and punctuates
   (`crates/engine/src/polish.rs`). A pool of threads asks it
   (`crates/server/src/polisher.rs`), never the finalizer; a session has
   one part out at a time, so segments still leave in order, each part
   with the last three cleaned as context. Only `text` changes; `words`
   stay as heard. A text that grew, shrank past what fillers explain, or
   is mostly words not heard in its segment keeps the heard text: a small
   model copied a neighbour's text into a segment
   (`docs/decisions/2026-10-25-llm-clean-up-of-the-final-transcript.md`).

A meeting uploaded whole (`POST /finals`) goes through the same steps:
each channel is read once for its pauses (Silero VAD), then range by range
(`final_pass.rs`).

The recording is kept in files of 5 minutes per channel, and the files the
pass has transcribed past are deleted as it goes (`Recording::forget_before`):
the server holds minutes of a meeting, not hours (115 MB an hour a channel
otherwise, on a server with tens of meetings). What is left is deleted
when dropped (`Recording`, `RecordedFile`): when the session ends, with or
without a final pass, when the connection is lost, after an upload's pass; a server that dies leaves its files, and
deletes them when it starts again, having first taken the folder's lock
(a second server on the same folder is refused, not let loose on the
first one's live recordings). An uploaded meeting (`POST /finals`)
is decoded into the same files and read by the same pass.

## Measured

On the dev Mac (M3, Metal), one session: the text of a phrase arrives
0.5–1.4 s after its last word with diarization, 0.5–1.0 s without. The
final pass read 23.6 s in 1.0 s and 45.3 s in 1.7 s. Load tests on this
machine say little about a server: see the decision records.
