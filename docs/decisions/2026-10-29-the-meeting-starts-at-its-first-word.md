# A meeting is shown from its first word, not from its cut

**Verdict.** What people read of a meeting counts from its first word:
the page's first line and header, the default title, the history's start
and duration, the date given to the report and the tasks, and every
turn's time ("**Speaker 1** · 00:00" for the first). One offset per
meeting does it, the start of the first final segment written, down to a
whole second, fixed when the page is created and kept with it (the
`Writer`'s `offset`, in `meeting.json` and in the history). The meeting's
own clock is not moved: segments, `published_until`, `final_until`, the
live session's and the upload's `offset`, the echo filter and the final
pass all stay on its samples. The cut stays where it is, and the next
meeting still starts at it.

## Why

A recording left running is cut into meetings (client `series.rs`,
`docs/decisions/2026-10-29-bounded-meetings.md`): a meeting closes after
15 minutes without a live phrase ending, and the next starts at the cut,
at once. One in which nothing is said for another 15 minutes closes
empty and publishes nothing, and so on. The meeting in which people
finally speak may therefore have started up to 15 minutes before its
first word, and everything shown about it counted from there: "Recording
started 11:00" for a meeting that began at 11:07, a first turn at 07:12,
a duration seven minutes too long. The trailing silence was already left
out (the duration is the last utterance's end); only the leading silence
was shown.

## What was rejected

- **Cutting at the last phrase instead.** The closer knows a meeting is
  over only 15 minutes after its last phrase, and by then those 15
  minutes have gone to the closing meeting: its live session, its
  pending folder, its final pass. The cutter cannot go back in time and
  hand them to the next meeting, and moving the closed meeting's end
  would not move the next meeting's start anyway.
- **Starting the next meeting only when a phrase arrives.** A phrase is
  known from the live transcript, which needs a session to run on the
  audio: holding the audio back until a phrase is heard leaves nothing to
  hear it. Buffering it on the side, to replay into a meeting started
  late, holds an unbounded silence (a recording of months is mostly
  silence) or drops the first words. And the live transcript, the
  pending folder and the echo filter all expect a meeting's channels on
  one clock from its sample 0.
- **Rewriting the page later.** Final segments are appended to a page
  nobody rewrites (only its first line is replaced at the end): a turn
  written as "07:12" stays so. So the offset must be known before the
  first turn is written, and never change after. It is: the page is
  created with the first final segments, and they are the meeting's
  earliest (they leave the session in start order, and none before
  `published_until` is ever written), so no earlier segment can come
  after it.

## Choices

- **Down to a whole second**: the first turn reads 00:00 and the start
  shown is within a second of the first word. Down to a minute would
  read as a round start but put the first turn at up to 00:59.
- **The first meeting too**: a recording started minutes before anyone
  speaks has the same leading silence; "started" now says when the
  meeting did, not when the button was pressed.
- **A meeting of notes only** (nothing said, notes written) has no first
  word: its offset is 0, and it is shown from its first sample.
- **Unchanged**: the pending folder's name and the history's id (the
  cut, named before anything is said), the times in `segments.jsonl`,
  `final.json` and the history's utterances (the meeting's clock, shown
  through the writer: the app's `get_meeting` shifts the turns it sends),
  and the live transcript of the CLI and the app's recorder, which runs
  on the recording's clock across meetings.
- **Tasks** need nothing of their own: the model reads the times the
  transcript prints and the page's turns are read from their headings,
  both on the displayed clock.
- **Before the change**: a `meeting.json` or a history entry without the
  offset has 0, as its page was written.

## Cost

One number per meeting, saved with the writer; a subtraction per turn
heading written. Nothing per sample or per frame.
