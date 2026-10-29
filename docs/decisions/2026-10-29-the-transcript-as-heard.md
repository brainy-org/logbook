# The transcript as heard: the clean-up removed

**Verdict.** The clean-up of the final transcript by a language model
(fillers, false starts, punctuation) is removed: the final transcript is
published as heard, always. The person who owns logbook decided to drop
it. The language model stays for the meeting's report only.

What it was (`docs/decisions/2026-10-25-llm-clean-up-of-the-final-transcript.md`):
a meeting that asked for it (`"polish": true` in `start` or the upload,
the CLI's `--polish`, the app's "Clean the transcript up") had its final
segments sent, a part at a time, to an OpenAI-compatible model before
they left the session. It lived in engine `polish.rs` (the prompt, the
checks against what was heard) and `polisher.rs` (the threads that asked
the model), in the session (`Final::polishing`, one part out at a time,
the last segments sent kept as the next part's context) and in the
finalizer (an upload's result went through it too).

What changed with it:

- The endpoint's client the report shares (options, chat completion, the
  models listed for the app's settings) moved to engine `llm.rs`. Its
  settings are named after it, not after the clean-up: the server's and
  the CLI's `--llm-url`, `--llm-model`, `--llm-key`, `--llm-timeout`
  (`LOGBOOK_LLM_URL`, `LOGBOOK_LLM_MODEL`, `LOGBOOK_LLM_KEY`), and
  `--report-parallel` on the server, for `--polish-*` and
  `LOGBOOK_POLISH_*`, which are no longer read.
- A session's `Sent` keeps the last ten minutes of segments
  (`KEEP_SENT`) and no longer at least three for the clean-up's context.
- `"polish"` is gone from `start`, the upload, a pending meeting's
  `meeting.json` and the app's settings.
