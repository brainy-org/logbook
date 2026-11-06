# A voice is checked before it is saved under a name

**Verdict.** Naming a speaker saves its voice under the name only when
the voice could be that person's (`Library::doubt`, client `voices.rs`).
It is refused when it sounds like someone else beyond doubt (another
person at `SURE` or more, ahead of the name by `MARGIN`), or when the
name has prints and the voice is no closer to any of them than a
stranger's (below `MAYBE`). The rename still happens; only the print is
held back, and the person can save it anyway (`--anyway` in the CLI,
`save_voice` from the app's notice). The idea comes from another
product's handoff (Verbale 0.16.2), which refuses a profile whose samples
disagree (a pair below cosine 0.50) without an explicit confirmation.

## Why

A person is matched by their best print, not an average
(`Library::scores`). One print of someone else's voice, from a slip of
the rename field or a speaker the diarization made of two people, makes
that voice the person, sure, in every meeting after: the failure that
withdrew naming during the meeting
(`2026-11-04-speakers-named-during-the-meeting.md`). Nothing checked a
rename's print before this.

## The thresholds

No new number: the check reuses the naming thresholds, measured on AMI
and Italian parliament speeches (`2026-10-27-speakers-named-by-voice.md`).

- "Unlike the name's own" is below `MAYBE` (0.55), where one stranger in
  a thousand matches a voice: a same-person score below it is as far as
  a stranger's. Verbale's 0.50 is an unmeasured heuristic.
- "Sounds like someone else" is the rule that would have named the voice
  as that other person by itself.
- A real change of voice (a new microphone, a call, a cold) can trip
  the first rule. That is why it asks instead of refusing for good, and
  the print saved anyway is kept like any other.

## Cost

Up to `MAX_PRINTS` cosines per person in the library, once per rename:
under a millisecond for hundreds of voices. Nothing in a meeting's run.

## Not done, from the same handoff

- **Re-evaluating the meeting's other speakers after a rename.** Within
  one meeting it changes nothing: the print added is the renamed
  person's, who is now taken there (one person, one speaker). Where it
  would help is the other kept meetings (the last 20), whose numbered
  speakers may now be that person: a suggestion to confirm, never a
  rename or a print by itself. Not built yet.
- **Speakers made of two people.** A final speaker's voice is one
  centroid; a diarization that merged two people gives one between
  them, which may still pass `SURE`. Keeping a few range embeddings per
  speaker would show it, at a threshold still to measure on AMI.
- **Listening to samples before saving them** (Verbale's enrollment):
  the client keeps a meeting's audio only until its final transcript, so
  there is nothing to play afterwards
  (`2026-10-24-the-client-keeps-the-meeting.md`).
