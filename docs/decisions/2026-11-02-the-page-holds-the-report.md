# The brainy page holds the report, never the transcript

The meeting's brainy page is made once the meeting is finished, after
the report: its header line ("Recorded … · duration · speakers") and,
under it, the report, or the notes as written when there is no report
(not asked for, or the model failed). The transcript never goes to
brainy: it stays on the client, in the history (the app), printed (the
CLI), or in the pending folder (`transcript.md`) while the page cannot
be made.

## Before

The page was created with the meeting's first final segments and the
transcript appended to it as it came, a minute or so behind
(`write_page`, `published_until`, a new page from the first segment when
brainy no longer had it: `write_or_anew`); at the end its first line was
rewritten as the header and the report went under it, above the
transcript.

## Why

What people read on the page is the report. The transcript of an hour is
pages of text that buries it, is searched in brainy as much as the
report and puts every word said, by people who never used logbook, in a
shared workspace. The client keeps it for whoever needs it.

## What follows

- Nothing is written to brainy during the meeting: the publisher only
  keeps the final segments in `segments.jsonl`. A brainy that fails
  during the meeting no longer matters; one that fails at the end keeps
  the meeting pending, as before.
- The page is made in one step, so there is no "how far it is written"
  to keep: `published_until`, `write_or_anew` and
  `Brainy::rewrite_first_block` are gone. The page is saved in
  `meeting.json` as soon as it is made, so a `finish` that tries again
  after it does not make a second one.
- The displayed clock (`Writer::offset`) is fixed when the final
  transcript is made, not when the page is created.
- A meeting left pending by an older version, with a page and part of
  its transcript on it, keeps that page: the report goes under its
  header, the transcript already there stays.
- Without `--report` (or the app's report off), the page holds the
  header and the notes only.
