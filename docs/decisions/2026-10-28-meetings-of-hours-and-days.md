# Meetings of hours and days

**Verdict.** A final pass that stopped is taken up where its final
segments stopped (`final_until`), with the speakers' voices it had, never
read again from the start: after a lost connection a new live session
gets `offset` and `voices` and is sent the audio kept from there; at the
end an upload or a pass on the client covers only the rest. Nothing on
either side holds a whole channel in memory, and the client deletes its
copy of the audio as the final segments pass it. A meeting of ten hours,
or of days, is transcribed completely, with the memory and the disk of
one of minutes, and its report is written in steps.

## What failed on a long meeting

The live pipeline and the progressive final pass were already bounded
(audio kept back to `KEEP`, the server's recording deleted as the pass
passes it, times in `f64`, samples in `u64`). What was not:

- **A lost connection threw the final pass away.** Only the first
  session had one ("a later one did not hear the whole meeting"). One
  drop of a second at hour seven, likely on hours of real networks, meant
  the client uploaded the whole meeting at its end and the server read
  all of it again.
- **That recovery held whole channels.** `Pending::audio` read every
  channel into memory, the upload encoded and sent it in one piece (with
  a copy per try), the server took the body whole, and `final_pass.rs`
  read each channel whole as floats for the VAD: for ten hours about
  1.2 GB a channel on the client and 2.3 GB on the server. The upload
  limit, 256 MB, refused anything past about eight hours of two channels.
- **The client's copy grew with the meeting**: one raw file per channel,
  115 MB an hour, deleted only at the end.
- **The publisher read `segments.jsonl` whole** at every batch (a minute
  or so), to write the page: quadratic in the meeting's length.
- **The app's live transcript** kept every phrase in the page, copying the
  list at each one.

## What it is now

- **Taken up, not started again.** The client knows how far the final
  segments have come (`final_until`, the last `until`, kept in
  `meeting.json`) and the speakers' voices last sent. A session opened
  again starts there: `offset` puts its times on the meeting's clock
  (the server moves everything it sends, results kept under its id
  included), `voices` seed the speaker linker (`Linker::seeded`) so that
  the numbers, and the names given from them, hold. The client sends the
  audio kept from `offset` to now, every channel in step, then the live
  audio; live utterances it already showed are not passed again. An
  upload or a pass on the client starts there too. The result is merged
  with what was kept: segments before `final_until` from the folder,
  after it from the pass (client `meeting.rs`, `merged`), which is
  correct whichever session's result it is.
- **Read and sent a part at a time.** `final_pass.rs` finds the pauses
  ten minutes at a time and reads ranges from the files; the client
  encodes each channel in parts on a thread as the request streams it
  (`StreamEncoder`); the server decodes each part as it arrives into its
  recording (`StreamDecoder`), so the limit is about disk: 4 GB of Opus,
  some 140 hours of two channels.
- **The client keeps minutes of audio.** Its copy is in files of five
  minutes, as the server's recording (`<channel>-<n>.pcm`), written
  unbuffered so that a session taking the pass up reads it back; a file
  whose last sample is before `final_until` is deleted. A folder kept
  before this, with one file per channel, is cut into files when opened.
- **The publisher holds only what is not on the page yet** (since
  `2026-10-29-bounded-meetings.md`, nothing: it reads it again from
  `segments.jsonl`).
- **The report of a long meeting is written in steps** (engine
  report.rs): past `ONE_REQUEST` (320 000 characters, six to nine
  hours), the model takes notes of each part of 80 000 characters, then
  writes the report from them; notes too long are taken again. Cutting
  the transcript instead would give a report of part of the meeting that
  reads as one of all of it. Tried on a real meeting with Qwen3.8 27B on
  Groq, cut into four parts on purpose: the report named the decisions
  and the tasks of every part. The notes came out in English for an
  Italian meeting until the instructions said, as the report's do, to
  write in the text's language.
- **The app shows the last 300 live phrases.**

## Measured

A synthetic meeting (macOS `say`, two voices taking 40 numbered turns,
7 min 22 s, one diarized channel), on this Mac with the Metal build:

- **Server killed at 200 s of audio, back 15 s later**, played at 2×: the
  client connected again and took the pass up from 184.1 s (`offset`);
  all 40 turns in order, each with its speaker, across the seam (turn 18
  starts at it, whole).
- **Server down until the meeting ended**, played at 3×: the meeting
  stayed pending with `final_until` 93.65; `finish` uploaded 348 s of
  442, all 40 turns with the right speakers (the voices kept the
  numbers).
- **A pending meeting of 26 minutes** finished by upload: the client
  peaked at 16 MB; 141 blocks, none with the wrong speaker.

## Costs

- **The audio between the last final segments and the drop is sent and
  transcribed live again**: a minute or two, plus the time disconnected.
  Live transcription of it is wasted work on the server (its text is
  shown only past what was shown), cheap next to reading the meeting
  again; a session that skipped live transcription for it would need
  its pauses found another way.
- **A word said across `final_until` may be cut** at the seam: the new
  pass starts there, not at a pause.
- **The clean-up's context** starts empty in a session taken up: its
  first part is cleaned without the three segments before it.
- **The server's disk for an upload of days**: decoded, 115 MB an hour a
  channel, until the pass has read it.

## Rejected

| Alternative | Why not |
|---|---|
| Keep a disconnected session alive on the server for a while, and let the client attach to it again | covers short drops only: a restarted server, a crashed client and a drop longer than the grace period still start again from zero, and the server holds sessions nobody may come back to |
| A session that takes the pass up without live transcription | the progressive pass takes its pauses from the live channel; for a minute or two of audio, not worth a second way to find them |
| Raise the upload limit and keep the rest | memory, not the limit, is what fails first: the whole meeting in memory on both sides, twice on a retry |

## Not covered

- **brainy** gets the whole transcript on one page: a page of days has
  not been tried.
- **The app's meeting page** shows the whole transcript at once.
