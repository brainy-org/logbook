# The tasks are removed

**Verdict.** logbook no longer looks for a meeting's tasks. These are
gone:
- engine `tasks.rs`;
- the server's `POST /tasks`;
- client `tasks.rs`, with the rows written to a brainy collection;
- the CLI's `--tasks` (`LOGBOOK_TASKS`);
- the app's "Tasks to the collection" setting and the meeting page's
  list of tasks.

The report writes its own action items again, as it did before
`2026-10-30-tasks-are-the-report-s-action-items.md`. The person who asked
for the removal found that it worked badly. The two earlier records
(`2026-10-29-tasks-as-collection-rows.md` and the one above) say how it
was built and what was measured.

## What goes with it

- **`ReportRequest::tasks` and the `{{actions}}` placeholder.** The
  report's model lists the action items itself. A client older than this
  that still sends `tasks` is not refused: serde ignores the field.
- **The bookkeeping that kept a retried `finish` from writing a row
  twice** (`TaskLog` in `meeting.json`). Its invariant in `CLAUDE.md` is
  gone too. A pending meeting or a kept one saved with `tasks` still
  reads, because the field is ignored.
- **What a rename did to the tasks**: setting the rows' Owner and renaming
  the bolded owner of the report's action items on the page
  (`renamed_action`). The report is now written again after a rename
  (`2026-10-30-many-voices-known-by-name.md`), so its action items take
  the new names from the model.
- **The brainy calls only the rows used**: `collection`,
  `extend_collection`, `find_row`, `create_row`, `set_rows` and `turns`.

## Not removed

The notes' task lists (`[] ` in the editor) and the checkboxes of the
report's action items stay. They belong to the notes and the report, not
to this feature.
