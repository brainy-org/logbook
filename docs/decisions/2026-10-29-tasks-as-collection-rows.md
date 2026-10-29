# A meeting's tasks as rows to confirm in a brainy collection

**Verdict.** The tasks agreed in a meeting become rows of a brainy
collection the person chooses (`--tasks`, the app's "Tasks to the
collection"), written with their own token at the meeting's end. The rows
are proposals, not facts: each is created with Status "To confirm" and
links to the moment of the meeting's page where it was said, and the
person promotes or deletes it in brainy. Decisions stay in the report.
With no collection set, the step is skipped: nothing is created and the
model is not asked. Extraction is one request per part of at most 80 000
characters (two to three hours), whatever the meeting's length, beside
the report's. Every row is written once, however often the meeting is
finished again (`meeting.json`, `tasks`).

## Proposals, not facts

A language model that lists tasks misses some and invents others: it
reads "we should look at the contract someday" as a task, gives a task
to the person who spoke instead of the one named, turns "soon" into a
date. People who were in the meeting disagree about its tasks too. A row
in a team's tracker that nobody agreed to is worse than no row: it gets
assigned, chased and closed as if someone had committed to it. So:

- **Every row starts "To confirm".** It is an option of the collection's
  Status (added if missing: a status option added later is in the to-do
  group, beside Open), never Open. Promoting it to Open, assigning it or
  deleting it is the person's call, in brainy. This needs no review step
  in logbook: the CLI, the app and `finish` all write the same way, and
  the review happens where the team works.
- **Every row links to its moment** (Moment: `<page>#block-<id>`, the
  turn heading at or before the time the model gave; brainy's page view
  scrolls to `#block-<id>` and flashes it). Checking a row costs one
  click. A task without a time of the transcript is dropped rather than
  written without a link, so a task found only in the notes is not
  proposed.
- **The prompt is conservative**: explicit commitments only, a person
  and an action ("I'll send the quote", "Luca does X"), not ideas,
  maybes or "we should someday"; an owner only when named or clearly
  self-assigned, and the doer, not the speaker ("Luca will do it", said
  by Anna, is Luca's); a due date only when said and resolvable from the
  meeting's date, never guessed. Missing a vague task is cheaper than a
  row to delete.

## Choices

- **Decisions are not rows.** A decision has no owner, no due date and
  nothing to close: as rows they would sit open forever in the filters a
  team uses for work, or need a kind column every view must filter on.
  They are read with their context, which the report on the meeting's
  page gives (its "Decisions" section). A kind of row can come later
  without changing the rows written now.
- **No collection set: skipped, not created.** A collection made next to
  the meeting pages would appear unasked in what may be a team's space,
  each client that finds none would make its own (two meetings ending at
  once, the app and the CLI), and the transcript would go to the model a
  second time for every meeting of everyone who never asked for tasks.
  Setting one is a line in the settings, and it may be any collection, a
  team's existing board included: the columns it lacks are added
  (`extend_collection` only adds), one it has with another type is left
  alone and not written (a person Owner: speakers' labels are not
  members), and "To confirm" is added to its Status.
- **Owner is text, the speaker's label as on the page** ("Speaker 2",
  "Luca"). brainy resolves person values only among the workspace's
  members, and refuses a name it does not know; a label is what logbook
  knows. Promoting the row is when a person is assigned. An owner the
  model names who is not one of the transcript's speakers is dropped.
- **Meeting is a URL, not a relation.** A brainy relation links only rows
  of a collection, and the meeting's page is a page. The row's text
  mentions the page too, so the page lists its tasks among its
  backlinks.
- **A separate request, `POST /tasks`, not a section of the report's
  answer.** One answer holding both Markdown and JSON has to be cut
  apart, a model that forgets the JSON loses the tasks, and a report
  that failed would take the tasks with it (not measured: chosen for
  robustness, at the cost of reading the transcript twice).
  The two are asked at the same time (`tokio::join!`), so the report is
  not later for it; each takes one of the server's `--report-parallel`
  places.
- **An older server** answers `404` to `/tasks`: no tasks, a warning, the
  meeting published as before.

## How extraction scales

The report sends up to 320 000 characters in one request and takes notes
of parts beyond that; tasks do not reuse those notes (they lose the
turn's time, which every row needs) and do not send the whole transcript
either. They go in the report's `PART`s, 80 000 characters (two to three
hours), one request each, in order: a request stays within a small
model's context, and an answer within the 8192 tokens a request may
write (a day's tasks as JSON with reasoning would not fit one). Each
part is given the tasks already found (at most `MAX_TASKS`, 100, a line
each), so a task discussed across a cut is not listed twice; repeats are
also dropped by their text.

## Exactly once

Written as `published_until` is for the page: the pending meeting records
how far it got.

1. The tasks found are saved in `meeting.json` (`tasks.found`) before any
   row is written; a meeting finished again never asks the model again
   (nor for the report: `report.md` is kept until it is on the page).
2. Before a row is asked of brainy, `writing` is saved; once brainy
   answers, the row (its address and moment) is appended to `tasks.rows`
   and `writing` cleared. `found[..rows.len()]` are in the collection.
3. A `finish` that finds `writing` set (the process died, or the answer
   was lost on the way back) looks the row up first (title is the task,
   Meeting is the page) and records it if it is there.
4. Only the process holding the meeting's `.lock` writes (`write_rows`
   refuses otherwise).
5. brainy not answering (a network error, a 5xx, 429) keeps the meeting
   pending, before the report goes on the page (so the report is not put
   twice either); `finish` writes the rest. A refusal (4xx: the
   collection deleted, no write access) gives up the rest with a
   warning: waiting would not fix it.

The rows' order in the collection is the order found; a crash leaves at
most one row looked up, never two of one task.

## Cost

- **Per meeting with a collection**: one request to the model per 80 000
  characters of transcript (one for a meeting of up to two or three
  hours, five for ten hours), with the notes and the date. Input: the
  transcript once more, as the report reads it; a meeting of up to six
  to nine hours is as many input tokens as its report again, a longer one
  a little less than the report (which reads the transcript and then its
  notes). Output: a few hundred tokens, against the report's one or two
  thousand. No added wait for the report: both are asked at once.
- **brainy**: one query and at most one extension of the collection,
  one read of the page's blocks (every block, once, for the moments), one
  write per row plus a lookup only for a row in doubt. A hundred tasks is
  about 105 requests, within brainy's 600 a minute.
- **On the server**: a task list holds a blocking thread and a
  `--report-parallel` place for as long as its requests take, like a
  report; a meeting asks for both at once.
- **Nothing per sample, nothing during the meeting**: it all happens
  once the meeting is published.

## Left out

- **"Since last time: 3 closed, 2 open"** across a series of meetings.
  It needs to know which meetings are a series, which will come from the
  calendar. The seam is the Meeting column: the rows of a series are
  those whose Meeting is one of the series' pages, queried with brainy's
  `query_collection` filters, Status grouped; nothing in logbook knows of
  series yet.
- **Updating a row** when a later meeting talks about its task again, and
  linking rows to people (a person property).
