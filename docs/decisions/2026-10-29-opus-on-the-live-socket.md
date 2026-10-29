# Opus on the live socket

**Verdict.** The live session sends its audio in Opus at 32 kbit/s, as
the upload already did, instead of PCM at 256: 8 to 10 times less
traffic per channel, the same transcript, speakers and voices within the
noise of any perturbation of the audio, and no added latency worth
measuring (the encoder's lookahead, 6.5 ms). It is negotiated: the
client asks in `start` (`"codec":"opus"`, with the lookahead), the
server confirms in `ready`; an older server answers a bare `ready` and
gets PCM, an older client asks nothing and sends PCM. The decoded stream
is sample for sample as long as what was captured, so every channel
stays on the session's clock (docs/protocol.md, "The audio").

## Why

- **Bandwidth.** 256 kbit/s a channel is 230 MB an hour for a call with
  two channels, on a laptop's Wi-Fi or a phone's tether, both ways
  contended with the call itself. And after a lost connection the
  client replays the audio kept since the final segments stopped
  (`docs/decisions/2026-10-28-meetings-of-hours-and-days.md`): 8 hours
  offline is about 920 MB a channel in PCM, about 110 MB in Opus.
- **It costs nothing the transcript sees** (below): the models were
  measured on audio through the codec, as for the upload.
- **The codec was already there**, for `POST /finals`: same encoder
  settings (16 kHz mono, VoIP, 32 kbit/s, 20 ms packets), same packet
  framing, one decoder for both.

## The framing

Each binary frame: the channel (u8), then the number of samples the
frame stands for (u32 LE), then the 20 ms packets those samples
completed, each a u16 LE length and its bytes. The lookahead (in
samples) is said once, in `start`. At the end, before `stop`, each
channel gets a frame of 0 samples whose packets flush the encoder (the
remainder and the lookahead in silence, padded to a packet).

The decoder emits exactly the cumulative count the frames said, after
dropping the first `lookahead` samples it decodes: nothing of the
padding, nothing missing at the end. That is what keeps channels
aligned (echo, speakers, the final pass's ranges compare them by time).

- **A count per frame, not a total in a header** (as the upload has):
  a live stream has no known end. Frames are not always multiples of
  20 ms (the `Timeline` fills a gap with any number of samples; the
  replay reads 1 s parts), so the encoder keeps a remainder across
  frames and a frame may carry no packet; the count says what the
  frame stands for whatever its packets.
- **The lookahead in `start`, not in every frame**: it is the
  encoder's, fixed for the session. Two bytes per frame would cost
  little, but a value that cannot change does not belong in the stream.
  Opus without it is not taken (PCM is answered).
- **A codec per session**: a session opened again after a lost
  connection has new encoders and decoders; the replay of kept audio
  goes through them like live audio. Nothing is carried across sessions.
- **The decoder's output is identical to the upload's** for the same
  samples (tested: `a_live_stream_comes_back_sample_for_sample`): the
  encoder is deterministic, so the measurements made through
  `examples/opus.rs` hold for the live stream.

## Latency

A 100 ms frame of whole packets is decoded at once but for its last
`lookahead` samples (6.5 ms), which come with the next frame. A phrase's
end is found by the VAD on the decoded audio, so it can be late by at
most one frame, and is late on average by the lookahead.

`logbook bench`, server and client on one M3 Mac (Metal), the first
300 s of AMI ES2004a (Mix-Headset), diarization on, PCM forced with
`LOGBOOK_LIVE_CODEC=pcm`, runs alternated:

| Sessions | PCM p50 / p90 | Opus p50 / p90 |
|---|---|---|
| 1 | 1.23 / 1.60 s, 1.23 / 1.60 s | 1.23 / 1.58 s, 1.23 / 1.58 s |
| 4 | 1.26 / 2.63 s, 1.25 / 1.63 s | 1.39 / 10.03 s, 1.38 / 4.13 s |

Alone, no difference. Four sessions are at the edge of what this Mac
keeps up with (eight did not: minutes of backlog with either codec), and
there a quarter of a run can fall behind, in either codec, so the p90 of
four sessions swings with the run; the two Opus runs had the worse
quarters. Opus makes slightly different phrases (61 utterances per
session against 63: the audio is not the same, so neither are the
pauses the VAD finds), so batches differ too. A server at its limit
should be measured again on its own hardware before reading more into
it.

## Cost at scale

- **Server**: one Opus decode per 20 ms packet, on the socket's task
  (`AudioIn`), into a buffer of one frame; no allocation or copy per
  sample. Decoding runs about 3000 times real time on one M3 core
  (`examples/opus.rs`): 100 channels (50 meetings of two) take about 3%
  of one core. A replay of 8 hours of backlog is about 10 s of a core
  per channel on the server.
- **Client**: encoding runs about 160 times real time on one M3 core:
  two channels live take about 1.3% of a core. A replay of 8 hours of
  backlog takes about 3 minutes of a core per channel, encoded one
  second at a time on the session's task: faster than a 5 Mbit/s link
  sends its PCM, and the server's pipeline transcribes the replay slower
  than either.
- **Memory**: per channel one Opus encoder or decoder (a few tens of
  KB) and a 20 ms buffer, for the session's life.

## Measured, 2026-10-29

M3 Mac, `crates/engine/examples/opus.rs` and `scripts/der.py`; AMI
Mix-Headset (CC-BY 4.0).

- **Diarization DER, PCM → Opus 32 kbit/s**, live strategy (stream-low)
  and final (each 60 s window alone, then voices):

  | Meeting | Live | Final |
  |---|---|---|
  | IS1009a | 25.3 → 24.7 | 19.3 → 19.3 |
  | ES2004a | 25.2 → 25.3 | 26.1 → 26.5 |
  | EN2002a | 28.6 → 28.5 | 30.8 → 30.2 |

- **WER against AMI's manual words**, full pipeline (`logbook play
  --final --speed 4`, local), five meetings (IS1009a, ES2004a, ES2004c,
  IS1009c, TS3003a; 18,863 reference words), pooled by words:

  | | PCM | Opus 32 | Opus 48 | Opus 64 |
  |---|---|---|---|---|
  | Live | 20.90% | 20.86% | 21.33% | 21.09% |
  | Final | 17.43% | 17.28% | 17.63% | 17.76% |

  Per meeting the differences swing up to about ±2 points in both
  directions at every bitrate: any perturbation of the audio moves
  uncertain words (PCM run twice gives identical text). Higher bitrates
  do not help, so 32 kbit/s is kept. The absolute WER is high for all
  because Mix-Headset mixes overlapping speakers and the reference
  counts fillers.
- **Voices** (WeSpeaker, 68 clips of 5 s, 11 speakers): the same clip in
  PCM and in Opus, cosine 0.964 (min 0.928); the same speaker across
  clips, PCM–PCM 0.555, PCM–Opus 0.546; different speakers 0.17. The
  0.45 threshold sits well between.
- **Bandwidth**: 32 kbit/s against 256 a channel (33 with the framing),
  8 to 10 times less; 8 hours offline replayed is about 110 MB a channel
  instead of about 920.

Not measured: languages other than English; a call's audio already
compressed by the call's own codec (double compression).

## Rejected

| What | Why not |
|---|---|
| Opus always, no negotiation | an older server would read Opus as PCM: noise, no error |
| Ogg Opus or WebM on the socket | a container's framing for no reader but ours; the sample count would still be ours to add |
| The total of samples in a header, as the upload | a live stream has none; per frame it is known |
| A higher bitrate (48, 64 kbit/s) | not better on WER or DER (above) |
| Opus in the local backend | no network: nothing to save, and it would only perturb the audio |
