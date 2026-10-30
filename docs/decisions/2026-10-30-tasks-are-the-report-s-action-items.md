# Tasks are the report's action items, asked before it

The tasks are found first and the report is written with them: its action
items are the tasks' lines, put in by the engine in place of a placeholder
the model writes (`report::with_actions`), not a second list of the
model's. The tasks prompt asks for what the meeting agreed (asked and
accepted, taken on, agreed on a condition), not a person's own plan in a
round of updates, each task written to be understood without the meeting.
A speaker renamed afterwards is renamed in the rows' Owner too. The
feature stays; removing it was the other choice.

## What was wrong

A stand-up of 11 minutes (2026-10-30, eleven people, Italian) gave nine
rows. Three were useful. "Continuare questa", "Chiedere a David" said
nothing of what; "Stopinare i commenti a Carlo" copied a misheard word;
three were each person's own plan for the day ("Scrivere test case").
Missing were what the meeting did agree: an e-mail if Task Force Crystal
did not answer by the morning (a condition, which the prompt's "not a
maybe" kept out), the comments on the API's controller and model, the two
versions of an API to publish. The report written beside it, by a second
request, listed seven other action items: two lists of one meeting.

The owners read "Speaker 3", "Speaker 5": the speakers were named on the
page afterwards, and the rename left the rows as they were.

## Measured

`crates/client/examples/tasks.rs` asks again for a kept meeting's tasks
(and report) from the transcript the client sends. On this meeting, the
transcript with the names given since:

- the old prompt, `openai/gpt-oss-20b` (Groq), twice: 14 and 7 tasks,
  "continua questa", "pubblica questo", "stopingi di nuovo a Carlo", no
  e-mail;
- the new prompt, same model, four runs: 8, 3, 8, 9 tasks; no vague one,
  the numbers said kept ("user story 41768", "API Major List"), the
  e-mail in three of four; owners and the plans of the day still vary;
- the new prompt, `openai/gpt-oss-120b`, three runs: 13, 12, 11 tasks,
  the same ones each time (the e-mail, the hours on Zveri's order, Kafka,
  the two API versions and the JSON for Data Platform, the alerting
  release, the chart library to have fixed), plus one person's plans for
  the day with their story numbers. The model matters more than the
  prompt past this point: the README advises the larger one.
- temperature 0 (for the same rows every time) made gpt-oss-20b reason
  until its 8192 tokens ran out, with no answer, in one run of three: the
  tasks keep the report's 0.2.

## Found on the way

- **Groq's free tier allows 8000 tokens a minute**, counting the answer's
  limit (`MAX_ANSWER_TOKENS`, 8192) as asked for: a meeting's request asks
  for 8000 to 10 700, and a second one in the same minute is refused
  (413 "Request too large … rate_limit_exceeded", or 429). With the tasks
  now before the report, the report is that second request.
  `llm::complete` waits what the provider says (`retry-after`, at most
  70 s) and tries again, three times in all.
- A meeting whose brainy page was deleted (trashed) stayed pending
  forever: every append answered 404, a refusal not tried again. It now
  gets a new page with the whole transcript (`write_or_anew`).
