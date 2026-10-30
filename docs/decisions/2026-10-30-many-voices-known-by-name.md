# Many voices known by name: the people expected, names offered, the report written again

**Verdict.** A library of hundreds of voices is kept. It costs nothing to
search, but the more voices, the more strangers sound close to someone.
So a meeting can be started with the people expected (`--expect`, the
app's picker on the home page). Its speakers are then named from those
voices only (`Library::only`, client `voices.rs`; kept as
`Saved::expected` for `finish`). No one expected means every voice, as
before. Names stay as they are written during the call: sure names go on
the page at once. A wrong one is fixed after the meeting by renaming it.
The rename offers the names the voice is close to (`candidates`, at most
3, at least `MAYBE`). The app writes the report again 8 s after the last
rename, without being asked. A person keeps their latest 5 prints
(`MAX_PRINTS`). When the likeliest person is already another speaker of
the meeting, the next one is offered as a question, never given.

## The problem

Naming was measured with a library of ~95 people
(`2026-10-27-speakers-named-by-voice.md`), and `SURE` = 0.70 was chosen
because "a personal library is smaller". A library that grows for months
breaks that assumption:

- Compute is not the limit. A few thousand 256-d cosines per speaker take
  under a millisecond.
- Accuracy is. `MAYBE` = 0.55 is where one stranger in a thousand matches
  one voice, so with 500 voices an unknown speaker is often offered a
  wrong "(Luca?)". Wrong sure names become more frequent too. `MARGIN`
  helps but does not prevent it.
- Prints grew without end. A person named in every meeting had more and
  more chances to be someone's best match, since they are scored by their
  best print. That also broke `2026-10-29-bounded-meetings.md`.

## What was decided, with the person who asked

- **The report is not held for a review of the names.** It is written as
  soon as the meeting ends and written again after a rename. Holding it
  would leave a meeting nobody reviews without a report, and would need a
  new pending state for `finish` to take up.
- **Sure names are published at once.** Holding them until a review would
  mean a page of "Speaker N" during the call.
- **The people expected are chosen by hand at the start.** The calendar
  is not read (it is still untested: `docs/app.md`).
- **The CLI is not interactive.** It has `--expect` at the start.
  `logbook speakers` lists who each recent speaker could be, for
  `speakers name`.

## Choices made along the way

- **A name not in the library is refused before any audio is taken** (CLI
  `Publish::check`; the app only offers names the library has).
  `meeting_at` itself leaves such a name out with a warning instead of
  failing. A voice forgotten during a recording of many meetings must not
  stop the next one from starting.
- **The expected people are the only ones in the running.** There is no
  lower threshold for them and no higher one for the others. The
  thresholds for a small pool were not measured, and guessing them was
  not worth it. Someone who comes unexpected stays "Speaker N", and is
  named by renaming them.
- **The runner-up is a question only.** If Anna is already Speaker 1 and
  Speaker 3's voice is closest to Anna's, Speaker 3 is most likely Anna
  split in two by diarization, not Luca. So Luca is offered as "(Luca?)",
  never written as a name: the margin is measured against Anna as well.
- **The oldest print goes first**, not the shortest. Voices and
  microphones change, and the latest naming says how the person sounds
  now.
- **The report is written again from the app, not from `rename_speaker`.**
  The app waits until no rename has followed for 8 s, so several renames
  ask the model once. It also writes the report on leaving the meeting.
  It never writes it while a name is being typed, and it disables
  renaming while it writes. `write_report` and `rename_speaker` each load
  and save the kept meeting, so running both at once would lose one of
  the two.

## Not done

- **Naming during the call.** The live bubbles' "Speaker N" are the live
  diarization's slots, which are unrelated to the final speakers.
- **Naming a speaker later in the meeting.** A speaker whose label was
  fixed with less than 8 s of voice keeps "Speaker N", even once their
  voice would be recognized. That needs headings relabelled in place
  while the meeting runs.
- **Expiring voices heard once, and telling apart people with the same
  name.** `add` and `forget` still go by name.
- **Measuring naming with a pool of 5 against a library of hundreds**
  (`examples/diarize.rs`, AMI).
