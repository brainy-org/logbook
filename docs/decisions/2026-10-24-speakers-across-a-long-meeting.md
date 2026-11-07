# The final pass follows the meeting, a minute at a time, and links speakers by voice

**Verdict.** The final pass no longer reads the whole meeting once it
ends. It follows it: about every minute, each channel's new audio, cut
at a quiet point, is transcribed again, diarized alone (Nemotron-3,
offline, at most eight voices in a minute) and its speakers matched to
the meeting's by voice (WeSpeaker ResNet34-LM embeddings, cosine above
0.45). The segments go to the client as they are ready
(`final_segments`), and the CLI adds them to the brainy page during the
meeting. What is left at the end is the last range, whatever the
meeting's length; nothing reads a whole meeting at once; the speakers are
not limited to the diarization model's eight. Measured: a 12-minute
recording with 17 speakers went from 39.1% diarization error (one offline
call, as before) to 2.2%.

## What did not scale

The first final pass (`2026-10-24-final-pass-on-the-live-server.md`)
read the whole recording after `stop`: minutes of waiting for an hour's
meeting (on the dev Mac 23.6 s took 1 s, so 3 h about 8 min), one
diarization call over the whole of it (refused past 2 h), the whole
recording on the server's disk (115 MB an hour a channel), and the page
in brainy only at the end. And Nemotron-3-Diarization has eight output
slots: a ninth voice is given one of them.

## What was measured

A tool of the engine (`crates/engine/examples/diarize.rs`) diarizes a
recording with each strategy and writes RTTM (on the audio as it came:
since 2026-11-07 it conditions it as the final pass does, and `--raw`
reproduces these numbers); `scripts/der.py` scores it
(DER, 10 ms frames, 0.25 s collar, overlap scored, one-to-one speaker
map). Data, both CC-BY 4.0: four AMI meetings (Mix-Headset, 13–35 min,
4 speakers; references from BUTSpeechFIT's AMI-diarization-setup,
word-based, so their "miss" is high for every strategy alike) and six
VoxConverse dev recordings (3–16 min, 7 to 17 speakers). Dev Mac, M3,
Metal.

| Strategy | AMI (4) | vox12 (8) | vox23 (10) | vox27 (10) | vox02 (17) |
|---|---|---|---|---|---|
| Offline, one call (before) | 24.8–33.4% | 1.3% | 11.1% | 12.2% | 39.1% |
| Stream, live latency (1.04 s) | 25.2–33.4% | 2.7% | 10.2% | 12.9% | 44.2% |
| Stream, checkpoint chunks (21 s) | = offline | = offline | = offline | = offline | = offline |
| Stream + voices, 60 s windows | — | — | 9.4% | 0.9% | 21.2% |
| Each window diarized alone + voices, 120 s | — | 2.0% | 0.3% | 0.2% | 10.8% |
| **Each window diarized alone + voices, 60 s (kept)** | **20.0–34.9%** | **1.4%** | **2.5%** | **1.4%** | **3.1%** |

- The stream in the checkpoint's configuration gives exactly the offline
  turns (every file), with one numbering over any length: the first
  design used it, until the eight slots showed on VoxConverse.
- Voices on a stream's windows help where a slot was given to a ninth
  voice, but cannot split a slot two voices share inside a window.
- Diarizing each window alone keeps a window's voices within the eight
  slots, and voices link the windows: the best everywhere with more than
  eight speakers. Shorter windows hold fewer voices each (vox02: 2.2% at
  60 s, 6.7% at 90 s, 10.8% at 120 s).
- Holding a slot's speaker from one window to the next ("slot prior")
  made it worse: it is exactly what stops a reused slot from becoming a
  new speaker (vox02 39.3%, vox27 10.3% against 0.9%).
- The similarity threshold matters little between 0.45 and 0.65 (the
  same and different voices are far apart with WeSpeaker); 0.45 is kept.
- Short embeddings are noisy: with a new speaker for each voice below the
  threshold, AMI meetings showed 7 to 14 speakers for 4. Only a voice of
  4 s or more founds a speaker; a shorter one joins the most similar
  (above 0.2) or is no one, its words going to the nearest turn. With
  that, every file has its number of speakers, but for vox02, whose five
  least heard speakers (seconds each) go to their neighbours: 12 of 17.
- The diarization of a window can split one voice into two slots; with
  "two slots of a range are never one speaker", the second founded a new
  speaker who then took over that person for the rest of an AMI meeting
  (end to end, DER 44.9%). Two slots of a range are one speaker when both
  match it above 0.6 (18.0% on the same run); neutral or better on every
  file.
- 10 s of audio per embedding gives the same DER as 30 s, in a third of
  the time (the embeddings were most of a range's diarization).
- On the AMI meetings the result is the offline one within noise (mean
  28.3% against 27.9%); the gain is where there are more than eight
  voices, and in the numbering holding across the meeting.

## Why not

| What | Why not |
|---|---|
| Keep the pass at the end, faster | the wait, the 2-hour diarization call and the disk all grow with the meeting |
| The streaming diarization alone | eight slots; a ninth voice merges into one |
| A global re-clustering at the end | improved a little (vox14 1.6% → 0.4%) but renames speakers after the page has been written; not worth a rewritten page |
| Voices without diarization (clustering embeddings of VAD pieces) | the pieces mix voices where people overlap; the diarization model handles overlap and is already loaded |

## Costs

- **A second transcription** of the meeting, in ranges of up to 60 s,
  batched 8 pieces a call, plus one diarization call and up to eight
  embeddings per range, on the finalizer thread (one job at a time for
  every session). Diarization is no longer repeated over the whole
  meeting at the end, so the total is about what the end-of-meeting pass
  cost, spread over the meeting.
- **A model more**: WeSpeaker ResNet34-LM, 26 MB, CC-BY 4.0, from the
  mirror's `parakeet-voice` bundle: the "voice" component of
  `parakeet-bundle-small.gguf` (mudler/parakeet-cpp-gguf at 741158ae)
  written back as the standalone file it was packed from, byte for byte
  (its sha256 is the one the bundle names), without the bundle's other
  models (one of them, CED, has an unsettled licence upstream).
- **The live and the final speakers are numbered apart**: the live ones
  stay eight streaming slots per session; the page carries the final ones.

## Measured, 2026-10-25

Dev Mac (M3, Metal), the production path end to end (logbook-server, a
client streaming the file and collecting `final_segments`, the segments
written as RTTM):

- IS1009a (AMI, 14 min) at twice real time: DER 18.0% (offline call:
  24.8%), 4 speakers; the last segments 1.4 s after the audio ended.
- vox02 (17 speakers) at 8 times real time: confusion 0.9% (its DER,
  9.5%, counts as missed the speech the segments do not cover: they are
  built from words, the reference from sound).
- A range (up to 60 s) is read again in about a twentieth of its length
  while a live session runs: one finalizer thread keeps up with about
  fifteen meetings at once on this machine. At 8 times real time a
  session is eight meetings' worth, and the pass fell a minute behind.
  The server's GPU is to be measured; more finalizer threads are the
  knob if it falls behind.
- The live text's latency with the pass running is unchanged (200 s of
  EN2002a: p50 1.31 s against 1.33 s, p90 4.91 s both).
