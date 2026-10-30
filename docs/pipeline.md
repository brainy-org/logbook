# The live pipeline

What happens to a channel's audio between the socket and an utterance.
Code: `crates/engine/src/{channel,session,batcher}.rs`, run by the server
or, without `--server`, by the CLI itself (`crates/cli/src/local.rs`).
On the server the audio arrives in Opus (or PCM from an older client)
and is decoded before it reaches the session, sample for sample what the
client captured (docs/protocol.md, "The audio"); without a server there
is no codec.

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
(`crates/engine/src/recording.rs`: raw s16le in the server's
`--recordings`, or the CLI's pending folder, 32 KB a second per channel), and:

1. **Ranges**: the live channel tells where it is quiet
   (`Channel::quiet_point`: where the VAD last heard speech end, or a
   little before now in silence; never inside speech). Once a channel has
   45 s of new audio, its range from the last cut to the latest quiet
   point between 45 and 60 s on (or, with none there, the first one
   after 45 s) goes to the finalizer thread, one job at a time for every
   session; none while the session has more than 2 s of audio queued,
   until it is under 1 s (`HOLD_FINAL`): the live transcript first, the final pass catches up at
   the end (`docs/decisions/2026-10-29-a-server-on-a-cpu-falls-behind.md`).
   A pause before 45 s is never an end: taken as "the latest
   within 60 s" after more than 60 s of speech, it held the channel, and
   with it every channel's final segments, until the meeting ended.
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
   its numbering holds from the first minute to the last. A channel keeps
   64 speakers at most (`MAX_SPEAKERS`): past that, the one heard least
   recently is forgotten, and a voice like it later is a new speaker with
   a new number. Numbers are never given twice in a meeting, so those
   kept are not renumbered and the client's names hold; the voices sent
   and saved may skip numbers.
4. **Segments**: words become segments with brainy-worker's rules (a
   change of speaker, a pause of 1.2 s, a sentence end past 8 s, 30 s at
   most), and are sent as `final_segments` up to the earliest cut of all
   channels, so they come in order; a channel with `echo_of` stops 2 s
   before the cut of the channel it echoes, whose words the echo filter
   must have heard.
5. **The end**: after `stop`, every channel's rest is cut in ranges the
   same way; then `final` and `done`. The session lives until then even
   if the client has gone, and the result is kept under the `finalizing`
   id: the segments of the last ten minutes or so of meeting
   (`KEEP_SENT`), from `from`, not the whole meeting's. A client that lost
   the end has every segment before its last `until`, minutes behind at
   most; one whose segments stop before `from` uploads from there
   (docs/protocol.md, `GET /finals/{id}`). A session holds minutes of
   segments, not hours, on a server with tens of meetings.

A meeting read whole (uploaded with `POST /finals`, or kept by a client
without a server) goes through the same steps (`final_pass.rs`): each
channel is read once for its pauses (Silero VAD, ten minutes at a time; a
pause at a part's very end is not one), then range by range. Neither
holds a whole channel.

**Taken up, never started again.** A pass that stopped (the connection
was lost, the server restarted, the client crashed) goes on from where
its final segments stopped, with the speakers it had found: a session
opened again after a lost connection is sent the audio kept from there
(docs/protocol.md, "Reconnection"), and a pass at the end reads or
uploads only that. The client merges: the segments it kept before that
point, the new pass's after it (client `meeting.rs`, `merged`). A word
said across that point may be cut; a meeting of days with a lost
connection costs a minute or so of audio read twice, not days.

The recording is kept in files of 5 minutes per channel, and the files the
pass has transcribed past are deleted as it goes (`Recording::forget_before`):
the server holds minutes of a meeting, not hours (115 MB an hour a channel
otherwise, on a server with tens of meetings). What is left is deleted
when dropped (`Recording`, `RecordedFile`): when the session ends, with or
without a final pass, when the connection is lost, after an upload's pass; a server that dies leaves its files, and
deletes them when it starts again, having first taken the folder's lock
(a second server on the same folder is refused, not let loose on the
first one's live recordings). An uploaded meeting (`POST /finals`)
is decoded into the same files as it arrives and read by the same pass.

A pass's state and result are in the data folder, not in the recordings
folder (engine `finals.rs`): replicas sharing it answer `GET /finals/{id}`
for one another, while each runs the passes whose audio it holds. One
that stops leaves its running passes unknown within a minute
(`docs/decisions/2026-10-29-replicas-share-the-data-folder.md`).

## The page's clock

A meeting is shown from its first word (client `transcript.rs`,
`Writer::offset`): the page's first line ("Meeting started …") and its
header at the end ("Recorded … · duration · speakers"), the default
title, the history's start and duration, and every turn's time, which reads 00:00 at the first word. The
offset is the start of the first final segment, down to a whole second,
fixed when the page is created (`write_page`) and saved with the writer
in `meeting.json`, so `finish` and a resumed meeting print the same
clock; without brainy it is fixed when the Markdown is written. A
meeting of notes only has 0. Nothing else moves: segments, `until`,
`published_until` and `final_until` stay on the meeting's samples. What
is printed and read back (the report's transcript, the app's turns) is
all on the displayed
clock
(`docs/decisions/2026-10-29-the-meeting-starts-at-its-first-word.md`).

## Report

Once the meeting is published, a language model can write its report
(`crates/engine/src/report.rs`), as Granola does: a summary, the notes
the person took during the meeting completed from what was said (nested
under each of their points, in their order), the decisions and the
action items with their owners. One request to an
OpenAI-compatible model (`crates/engine/src/llm.rs`), with the whole transcript, up to
320 000 characters (six to nine hours). A longer meeting is reported in
steps, never cut (a report of part of a meeting reads as one of all of
it): the transcript is split between paragraphs into parts of 80 000
characters (two to three hours), the model takes detailed notes of each
in order (subjects with who said what, names and numbers exactly,
decisions, tasks with owners and dates, open questions, what touches the
person's notes; at most about 1500 words), and the report is written
from the notes as from the transcript. Notes still too long for one
request (a meeting of weeks) are taken again the same way. Ten hours
cost six requests instead of one. The transcript is the one
on the page, with the names the client gave the speakers: the server
never sees the client's voices, so the client sends the Markdown
(`POST /reports`, docs/protocol.md), not the segments. The server answers
it only with `--llm-url`; without a server the client asks the model
itself (`LOGBOOK_LLM_URL`, the app's settings).

The report goes on the page under its header line, and holds the notes.
Without one (not asked for, or the model failed: a wrong key, a model
down), the notes go there as they were written, under "Notes", in the
report's place: a report written again later replaces them
(`meeting.rs`, `finish`). Notes written only into a report that failed
reach the page nowhere.

The report is written in the meeting's language, headings included. The
instructions say so in so many words and give the headings in two
languages: with an Italian example alone, Qwen3.8 27B wrote an English
meeting's report in Italian; with no example, an Italian meeting's
headings in English. A German meeting still gets English headings over
German text.

On the page the report goes right under the header line, above the
transcript (`meeting.rs`, `insert_under_header`); asked again, it
replaces the blocks of the last one (`history::Kept::report_blocks`). A
report that fails leaves the meeting published without it. A report
written is kept in the pending folder (`report.md`) until it is on the
page: a meeting finished again does not ask the model again.

A speaker renamed afterwards (the app's meeting page) is renamed on the
page and in the history; the app then writes the report again with the
new names (docs/app.md).

## Measured

On the dev Mac (M3, Metal), one session: the text of a phrase arrives
0.5–1.4 s after its last word with diarization, 0.5–1.0 s without. The
final pass read 23.6 s in 1.0 s and 45.3 s in 1.7 s. Load tests on this
machine say little about a server: see the decision records.
