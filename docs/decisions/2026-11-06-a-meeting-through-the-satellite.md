# A meeting heard through the satellite: diarization error more than doubles

**Verdict.** A real four-person meeting (AMI IS1009a, 14 min, headset mix) played from the
Mac's speakers and heard by arvel's satellite (an iPad 1–3 m away, recorded with
`arvel chat --record-room`) is diarized more than twice as badly as the original: 47–54%
diarization error against 20–25%, with a quarter of the speech missed and 9–13% of it given to
the wrong person. No window, threshold or strategy brings the confusion below 9.3%, and turning
the browser's noise suppression and echo cancellation off changes nothing: the damage is
acoustic. The test cannot tell the room and the distance from the replay itself (a mixed
recording through small speakers), so it does not yet say what a satellite hears of people at
a table. Meeting diarization in arvel waits for that measurement.

## How it was measured

The method is `docs/measuring-a-room-microphone.md`. The audio: IS1009a's Mix-Headset, a beep
added at the start for alignment, the level normalized, played with `afplay`; the satellite's
page sent what it heard to `arvel chat --record-room DIR --sleep-minutes 0`. The reference is
BUTSpeechFIT's word-based RTTM of IS1009a, which is why about 17% is "missed" even on the
original audio (pauses inside a turn counted as speech). The recording sat a steady
29.92–29.93 s after the reference, checked at four points. Scored with `examples/diarize`
(Nemotron-3 diarization, WeSpeaker ResNet34-LM voice prints) and `scripts/der.py`.

## Numbers

Original audio → satellite (DER / confusion / speakers found, of 4):

| Strategy | Original | Satellite |
|---|---|---|
| `chunked-linker`, 60 s, 0.45 (the one logbook uses) | 20.0% / 0.3% / 4 | 53.8% / 13.0% / 4 |
| `linker` | 22.7% / 3.6% / 4 | 47.3% / 9.4% / 4 |
| `offline` | 24.8% / 6.3% / 4 | 51.3% / 13.1% / 3 |

The satellite's `linker` in parts: miss 27.1%, false alarm 10.9%, confusion 9.4% (on the
original, miss about 17% and false alarm about 2%).

A grid of 48 settings (window 30–180 s, threshold 0.35–0.60, both linker strategies) on both
recordings: on the satellite the best was `linker`, 30 s, 0.40, at 47.0% with 9.3% confusion;
on the original `chunked-linker`, 60 s, 0.35–0.50, at 20.0%.

With the browser's processing off (a `--mic-raw` tried and dropped): `linker` 50.2% (miss 26.9,
false alarm 12.6, confusion 10.7, 3 speakers), `chunked-linker` 53.3% (25.5, 14.8, 13.0, 3),
`offline` 50.6% (25.6, 15.1, 9.9, 5 speakers).

## What is next

Separate the two causes: the same replay with the device 30–50 cm from the speakers, AMI's own
far-field table microphones (a real room, no replay), and a real meeting of people at a table
recorded by the satellite. Then decide between a better microphone (a USB conference
microphone on the satellite), a heavier diarization run after the meeting, or a meeting feature
that does not promise who said what.
