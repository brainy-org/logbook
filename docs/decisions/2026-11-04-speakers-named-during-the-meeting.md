# Speakers are not named during the meeting

Withdrawn the day after it was built: naming a speaker from the live
transcript filled the library of voices with wrong voiceprints. A
speaker is named afterwards only, on the meeting's page, from the final
transcript the person reads (`docs/decisions/2026-10-27-speakers-named-by-voice.md`).
The title, changed in place while a meeting records, stays (docs/app.md,
client `series.rs`, `Notebook::set_title`).

## What it was

The person clicked a speaker's label in the live transcript and gave a
name. Live and final speakers are numbered apart (docs/protocol.md): the
live ones by the streaming diarization, per session; the final ones once
for the meeting, linked across ranges by voice. So the name went to a
live speaker, and each final segment voted, with the seconds it
overlapped that live speaker's phrases, for the final speaker that took
it: the most heard pair first, at 2 s together at least. The name came
before any voice known by name, and the final speaker's voice was saved
under it as a print, as a rename afterwards does.

## Why it failed

- **A guess saved as a fact.** A rename afterwards is the person's
  decision on the final transcript. Here the print was saved with no one
  looking, on a match made only by time.
- **The match is weak.** The streaming diarization has few slots and no
  voice: with several people, or similar voices, one live speaker is two
  people, and the name votes for both final speakers. 2 s together is
  enough to take it.
- **One wrong print spoils a name for good.** A person is matched by
  their closest print (`Library::scores`, the best over prints, not an
  average): one print of someone else's voice makes that voice the
  person, sure, in every meeting after. Each meeting of a recording cut
  into several added a print again.
- **The name hid the voice.** A name given live was taken away from every
  other speaker: when the vote put it on the wrong one, the speaker the
  library knew by voice lost it, and the wrong voice was saved under it.

## If it comes back

Not as a print saved by itself. A name given live could at most be
offered on the meeting's page as a suggestion the person confirms, and
saved as a voice only then.
