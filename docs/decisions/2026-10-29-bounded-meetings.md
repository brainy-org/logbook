# What a meeting keeps is bounded, and a long recording is a series of meetings

**Verdict.** `logbook live` (and the app's recorder) may run for months:
the recording is cut into meetings (client `series.rs`) when no live
phrase has ended for 15 minutes (`--pause`) while the live transcript
runs, or when a meeting has lasted 8 hours (`--max-length`), at the end
of its next phrase or 2 minutes later in any case. Every channel is cut
at one sample; capture never stops; the meeting that closed ends as any
meeting ends (final pass, page, report, pending folder) while the next
one records. Inside a meeting, what grew with it is bounded: a session
keeps the final segments of its last 10 minutes (`KEEP_SENT`; the
result's `from` says where they start), a channel's speaker linker keeps
64 speakers (`MAX_SPEAKERS`, the one heard least recently forgotten,
numbers never reused), and the publisher holds nothing while brainy
fails (it reads `segments.jsonl` again). What stays unbounded: the
pending folders on disk while the client is offline.

## What grew

Before, each of these grew for as long as a meeting lasted; a meeting
lasted as long as `live` ran:

- **`sent` in the engine's session**: every final segment, kept until the
  pass completed, for the context of the clean-up (the last three) and
  for `Finals` (`GET /finals/{id}`, and the local backend). About 2 KB a
  segment with its words, 10 MB for 8 hours, per session on a server
  with tens of them, then written whole to the data folder for an hour.
- **`unwritten` in the client's publisher**: every segment not on the
  page, for as long as brainy refused it.
- **The speaker linker** (engine `speakers.rs`): a centroid per speaker
  ever found, compared with every voice of every range, sent with every
  `final_segments` and saved in `meeting.json`. A noisy channel founds a
  speaker now and then; over months the list has no end.
- **A meeting itself**: its page, its `segments.jsonl`, its history entry,
  its report, its final pass's upload when the server is lost. Bounding
  the parts inside it leaves those; only an end to the meeting bounds
  them.

## How it works now

- **The result's `from`.** `FinalState::Done` carries `from`: every
  segment starting at or after it is there, none before. A session
  keeps parts of what it sent (each `final_segments` whole) while they
  are within `KEEP_SENT` of the latest `until`, and always the last
  three segments; a part dropped moves `from` to its `until`. A session
  starts with `from` at its `offset`; an upload's result starts at its
  `offset`. The client merges a result only when `from` is at or before
  its own `final_until` (`merged`); otherwise it has a gap, and does what
  it does for a `404`: uploads its audio from `final_until`, which the
  pending folder keeps. A client that lost the end of a live session has
  every segment before its last `until`, a minute or two behind, so the
  gap happens only to one that lost more than ten minutes of segments
  and still polls the old id. An older server sends no `from`: 0,
  everything, as before.
- **The publisher reads again.** It remembers where in `segments.jsonl`
  the first segment not on the page starts (a byte offset) and reads
  from there at the next attempt; `write_page` takes those from
  `published_until` on, so nothing goes twice or out of order.
- **64 speakers a channel.** Each speaker keeps its id and when it was
  last heard (the range's end, on the meeting's clock). A new one past
  the cap forgets the one heard least recently, never one of the range
  being linked. Ids are never given twice: the client's names for the
  speakers kept do not move. A pass taken up again seeds from the voices
  sent (sparse ids); its next new speaker takes the highest plus one.
  The client keeps the last voices sent, so a forgotten speaker's voice
  is not in `meeting.json` at the end and cannot be named afterwards
  from that meeting.
- **A series of meetings** (`series::meetings`, which the CLI's `live` and
  `play` and the app's recorder use; `meeting::meeting` is one meeting).
  `Closer` decides, on the meeting's samples (no clock of its own: tests
  give it any time):
  - *pause*: no live phrase has ended for `pause` since the last one's
    end, or since the live transcript came (back), or since the meeting
    started, while a live session is open (`Events::connected`). Offline
    nothing is transcribed live, and its silence says nothing.
  - *length*: the first live phrase heard past `max_length`;
  - *hard limit*: `max_length` + 2 minutes, whatever is heard: in
    continuous noise no phrase ends, and offline none comes.
  `Cutter` cuts every channel where the furthest one is (none can go
  back): channels behind it send the meeting that closes what they have
  before the cut, a frame across it is split, and the meeting's audio
  ends once all have reached it. The next meeting's sample 0 is the cut;
  its `started` is the first one's plus the cut. Each meeting runs as a
  task of its own (`meeting_at`): its events come back to the loop,
  which puts the live phrases on the recording's clock (the CLI's
  latency and the app's transcript stay right) and passes them on.
  Notes go to the meeting they were typed in (`Notebook`, see the app
  below), even when they arrive after its cut. `crates/client/tests/
  series.rs` runs the whole series against a fake server, without
  models: a pause cuts two channels at one sample, each session gets
  exactly its samples, and each meeting keeps its own notes.
  A custom title takes " (2)", " (3)"… from the second meeting on.
- **A meeting in which nothing was said publishes nothing.** The brainy
  page is created with the first final segments (or at the end), not at
  the start; at the end a meeting without a segment, a page or a note is
  removed (`Outcome::silent`): no page, no report, no history. A room
  left recording overnight makes one such meeting every 15 minutes, and
  nothing to read.

## Choices

- **A pause of 15 minutes**: a break in a meeting (coffee, waiting for
  someone) rarely lasts that long; two meetings in a row usually have a
  gap of that order, or the person stops the recording. Shorter cut real
  meetings in two; longer joins back-to-back ones.
- **8 hours**: longer than a working day's meeting, short enough that a
  meeting's segments, page and report (written in steps past six to nine
  hours) stay what was already tried. Cut at a phrase's end, not at the
  hour, so that a word is less likely to be split.
- **0 turns a limit off** (`--pause 0`, `--max-length 0`): both off is the
  old behaviour, a meeting as long as the run, for whoever wants it.
- **The hard limit is time, not the server**: it holds offline, where
  only the length can close a meeting.
- **Cut where the furthest channel is**, not where the slowest is: what
  was sent cannot be taken back; the slower channels catch up within a
  frame or two (`Timeline` moves even a silent device along).
- **Ten minutes of segments kept**: a client polls the result only after
  a lost connection at the end; its segments are then a minute or two
  behind the end. Ten leaves room, at a few hundred KB per session.
- **The app**: no settings for the limits yet (the defaults); the
  recorder draws itself again with empty notes when a meeting starts.
  Notes go to the meeting they were typed in (client `series.rs`,
  `Notebook`), not to the one current when they arrive: the last
  keystrokes before a cut reach the app just after it, and a meeting
  reads its notes only once its final pass is done.

## Cost at scale

- **Per frame**: the forwarder adds the frame's length to its channel's
  position, compares it with the cut, and moves the frame on; a frame
  across a cut is split once (one allocation, once per meeting and
  channel). The closer compares three numbers. Nothing per sample.
- **Per session, on the server**: final segments of ten minutes (a few
  hundred KB) instead of the meeting's (10 MB for 8 hours), in memory
  and in the result written to the data folder. The linker: at most 64
  centroids of 256 floats a channel (64 KB), each range's voices (up to
  eight) compared with them; `voices` in every `final_segments`: at most
  64 × 256 floats, about 160 KB of JSON a minute when full, and as much
  in `meeting.json`.
- **At a cut** two sessions of one client overlap for the old one's last
  minute or so of final pass: one more session on the server, briefly.
- **The publisher while brainy fails**: no memory; each batch (about a
  minute) reads the segments since the failure from disk, so a failure
  of hours reads a few MB a minute, bounded by the meeting's length.
- **Latency**: none added: the live path is unchanged; a cut adds no
  wait.

## What stays unbounded

- **The pending folders while offline**: a client that never reaches its
  server keeps each meeting's audio whole (115 MB an hour a channel, as
  raw PCM) until `logbook finish`; a month offline with two channels is
  about 165 GB. Cutting into meetings does not change that; deleting
  audio not transcribed would lose meetings.
- **A meeting with both limits off**, by choice.
- **The pending meetings themselves**: one folder per meeting that could
  not be published, until `finish`.

## Costs

- **A word said across a cut** may be split between two meetings: a
  length cut waits for a phrase's end, but the phrase is heard a second
  or so after it ends; a pause cut can meet speech that began in the
  last second.
- **A meeting that a pause closes keeps its last 15 minutes of silence**
  (transcribed for nothing, cheap: the VAD finds no speech).
- **A forgotten speaker** who speaks again is a new number in the same
  meeting; with 64 kept, this takes a meeting of dozens of voices.
- **The clean-up's context** is the last three segments kept: always
  there, since a session never drops below three.

## Rejected

| Alternative | Why not |
|---|---|
| Keep `sent` whole and drop the result after a delivery | the client fetches the result only when it lost the end: the server cannot know it was delivered |
| Hold `unwritten` up to a cap, drop the rest | a page with a hole in it, which is append-only and never rewritten |
| Cap the speakers by merging the closest two | changes the speaker of segments already on the page, which never change |
| Cut each channel where it is when the decision is taken | channels fall out of step in the new meeting, and the echo filter compares them by time |
| End the run at `max_length` and let the user start again | months unattended is the point; a stopped recording loses what follows |
| A meeting closes when the live transcript is silent, online or not | offline every 15 minutes would make a meeting, each kept pending |

## Tried, 2026-10-29

The desktop app on a Mac, its pause set to 15 s for the trial (by an
environment variable since removed: the app keeps the defaults), "In the room":
speech, then typing through the silence until the notes emptied, then
speech again and stop. Two meetings were kept, "Prova taglio" and
"Prova taglio (2)", 33 s apart, each with its own phrases; the first
one's notes ended with the last letter typed before the cut, and the
second's held none of them. No meeting was left pending.
