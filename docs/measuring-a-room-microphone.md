# Measuring diarization on a room microphone

How well do logbook's diarization strategies tell people apart on audio
taken a few metres away, the way arvel's satellite (an iPad or a phone in
the room, a browser page as arvel's only microphone) hears a meeting? The
numbers in `decisions/2026-10-24-speakers-across-a-long-meeting.md` are on
AMI's headset mix and VoxConverse (broadcast audio); this is the kit to
measure the room before building meeting diarization in arvel.

Three steps: record a meeting as arvel hears it, make the reference (who
spoke when) by hand, score every strategy against it.

## 1. Record

**arvel's own input (the one that decides).** On the computer running arvel:

```bash
arvel chat --record-room ~/arvel-room --sleep-minutes 0 --room-minutes 0
```

`--record-room` (arvel's `docs/decisions/2026-11-06-recording-the-room-for-a-measurement.md`)
writes exactly what arvel's VAD and ASR get: the page's microphone after the
browser's echo cancellation and noise suppression, resampled to 16 kHz,
through `--mic-gain`/`--mic-boost`; 16-bit mono WAV, a file per span awake
and listening (`room-YYYYMMDD-HHMMSS.wav`, a `[record]` line at the terminal
at each start and stop). Privacy mode, sleep, the voice off and the page
reconnecting close the file: `--sleep-minutes 0` keeps arvel awake for the
whole meeting; `--room-minutes 0` keeps the meeting's words from going to
the language model in the room's summaries (the transcript still has them,
as always). Use your usual `--mic-gain` setting: it is part of what is
measured. Nobody says arvel's name during the meeting.

**A control at the same time (recommended).** A phone on the table in the
middle of the people, recording with Voice Memos. Same meeting, close to
everyone: if it scores well where the satellite scores badly, the distance
and the satellite's path are the problem; if both score badly, the voices
or the meeting are. Clap once at the start, loudly, so the two recordings
can be aligned (`reference.py offset`).

**No arvel at all (a baseline only).** Voice Memos on the satellite device,
at the satellite's place. It is not arvel's input: no browser echo
cancellation or noise suppression, AAC, its own gain. Noise suppression
changes the voice the embeddings see, so treat its number as an estimate.

The meeting:

- **3 to 5 people**, a real meeting or a lively discussion, **10 to 15
  minutes** (10 to 15 of the 60 s windows the final pass diarizes alone:
  long enough for every voice to come back across windows).
- **The device where it lives**, not moved closer: on its stand, 1 to 4 m
  from the people. Note the distances and the room.
- Everyone talks at least a minute in all. Keep what makes it hard and
  real: short interjections ("sì", "esatto"), a few people talking over each
  other, someone turned away from the device, two similar voices if you have
  them. No music or video playing.
- Write down the names and who sat where.

## 2. The reference

DER needs who-spoke-when. The cheapest reliable way for one recording is to
correct a draft by ear in Audacity (free), about 3 to 4 times the meeting's
length (45 to 60 minutes for 15):

```bash
scripts/room-der.sh ~/arvel-room/room-20261107-101500.wav        # the RTTMs, nothing scored yet
uv run scripts/reference.py draft ~/arvel-room/room-20261107-101500.der/chunked-linker.rttm > labels.txt
```

In Audacity: open the WAV, File > Import > Labels > `labels.txt`. Listen
through and fix every label: rename `spk0`… to the person (same spelling
each time), drag the edges to where the voice starts and ends, split a label
where the speaker changes, add the turns it missed, delete what is not
speech. Two people at once: one label `anna+marco` (or two overlapping
labels). Unsure who: `?` (left out of the scoring). Then File > Export >
Labels.

```bash
uv run scripts/reference.py check labels.txt                    # speakers, seconds, leftovers
uv run scripts/reference.py rttm labels.txt > ref.rttm
```

For the control recording, the same labels moved by the offset between the
two (labelled on the satellite's recording):

```bash
uv run scripts/reference.py offset satellite.wav phone.m4a      # prints seconds
uv run scripts/reference.py rttm labels.txt --offset 2.41 > ref-phone.rttm
```

The draft favours the strategy that made it wherever an edge was not moved,
so miss and false alarm lean its way a little; confusion (who) is fair once
corrected by ear, and it is what this measures.

Other ways considered: a close microphone per person as the reference
(several recorders to align, and crosstalk still has to be labelled by ear);
a scripted meeting with turns read in a known order (not how people talk,
and the times still have to be labelled). A clean scripted dry run with
macOS `say` voices is good only to check the pipeline: synthetic voices
fool the embeddings (see below).

## 3. Run

```bash
scripts/room-der.sh ~/arvel-room/room-20261107-101500.wav ref.rttm
scripts/room-der.sh phone.m4a ref-phone.rttm                    # the control, if any
```

It builds the `diarize` example, takes libparakeet, Nemotron-3-Diarization
and WeSpeaker from `~/.cache/brainy-org/` (fetched by the first start of
logbook or arvel), runs every strategy (`offline`, `stream-low`,
`stream-model`, `chunked`, `stream-embed`, `stream-model-embed`, `linker`,
`chunked-linker`, the voice ones with the kept 60 s window and 0.45
threshold), scores each with `der.py` (0.25 s collar, overlap scored) and
prints a table. `chunked-linker` is what logbook's final pass does. The
example conditions the audio as the final pass does (band-pass and gain,
`crates/engine/src/conditioning.rs`); numbers measured before 2026-11-07
were made without it, which `--raw` gives back. A
15-minute recording takes a few minutes on the dev Mac (the streaming
strategies run at about 10 times real time, the rest far faster).

## Reading the result

Look at `chunked-linker` (and `offline`), confusion first, then the speaker
count (ref / hyp). The anchors from logbook's measurement:

- **2.2%** is the kept strategy on vox02 (17 speakers, broadcast audio);
  0.3 to 2.5% on the other VoxConverse files: clean audio, good
  diarization.
- **39%** is one offline call on the same file, where eight output slots
  could not hold seventeen voices: what failing to tell people apart
  looks like. With 3 to 5 people the slots are not the limit, so failure
  here would be the audio.
- AMI's headset mix scored 18 to 35%, most of it missed speech of a
  word-based reference: miss and false alarm move with how the reference
  was made, confusion does not.

Then, on the satellite's recording:

| `chunked-linker` | Means |
|---|---|
| DER under ~10%, confusion under ~5%, speakers right | fine: the satellite's microphone is good enough; build on it |
| DER 10 to 25%, or confusion 5 to 15% | usable, to be tuned: look at what dominates (a speaker split in two: threshold; voices merged: embeddings), and at `offline` against `chunked-linker` |
| DER over ~25% with confusion dominating, speakers merged | the region of the 39%: people are not told apart. The microphone (distance, the browser's processing) is the problem if the control on the table scores fine; if the control is as bad, it is the voices, the meeting or the reference |

Miss and false alarm against a hand-made reference mostly measure the
edges; a large miss with little confusion is the VAD or the diarizer
missing quiet, far voices, worth noting but not a speaker problem.

## Checked end to end, 2026-11-06

On a synthetic 4-person "meeting" (227 s, macOS `say` Italian voices, the
truth known exactly), clean and with a room simulated (echoes, band limit,
pink noise), the pipeline ran from WAV to table. The numbers mean nothing
about people: on the clean mix every strategy merged voices (45 to 52%), on
the "room" one `offline` scored 2.3% and `chunked-linker` 25.8%. Synthetic
voices of one engine are too alike for the embeddings, and digital silence
between turns is not a room. Only a real meeting answers the question.
