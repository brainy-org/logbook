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

**Update 2026-11-07: the replay was the cause.** AMI's own table microphone (Array1-01, a real
room, the people 1–2 m away, no replay) diarizes almost as well as the headset mix on four
meetings: with `chunked-linker` 60 s 0.45, confusion 0.4–2.0% (headset 0.1–1.7%), DER within
0.7–2.7 points of the headset's, the right number of speakers (two small extras on ES2004a), and
13 of 14 speakers named right by voice with 30 s of enrollment, no sure name wrong on 235
segments. See "One microphone on the table" below.

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

## One microphone on the table (2026-11-07)

Four AMI meetings (IS1009a, ES2004a, TS3003a, EN2002a), each scored on the headset mix, on one
table microphone (Array1-01), on two adjacent array microphones 7.7 cm apart (a phone's two
mics, delay-and-sum), and on all eight (delay-and-sum). DER / confusion with `chunked-linker`
60 s 0.45:

| Meeting | Headset | Array1-01 | 2 mics | 8 mics |
|---|---|---|---|---|
| IS1009a | 20.0 / 0.3 | 20.8 / 0.9 | 20.6 / 1.0 | 22.4 / 2.0 |
| ES2004a | 26.4 / 0.1 | 29.1 / 0.8 | 28.8 / 1.0 | 30.2 / 1.5 |
| TS3003a | 34.9 / 0.1 | 35.7 / 0.4 | 35.6 / 1.0 | 22.8 / 1.5 |
| EN2002a | 30.2 / 1.7 | 32.6 / 2.0 | 32.6 / 2.8 | 32.0 / 2.0 |

Almost all the DER is "miss" against the word-based reference, on the headset too: read the
confusion first. Two microphones change nothing; eight do not lower the confusion and help only
where the level was the problem (TS3003a's miss 35 → 18%). A direction-of-arrival cue from two
adjacent microphones (GCC-PHAT) puts a speaker's pieces in their own angle 28–91% of the time:
a weak tie-break, not a separator.

Cheap pre-processing (a 100 Hz high-pass and EBU R128 loudness normalization) cuts the miss by
3–7 points on IS1009a and TS3003a, confusion unchanged; DeepFilterNet3 makes it worse
(confusion up to 7.5%, quiet speech suppressed: ES2004a's miss 56–60%). On ES2004a's table
microphone a threshold above 0.45 over-splits (8–11 speakers): keep 0.45 or lower. A heavier
model after the meeting, DiariZen large (WavLM, CPU, 2.4–2.9× real time), reaches 9.4% and 11.5%
DER on IS1009a and ES2004a, almost all of it by catching more speech (it learned AMI's
reference style); its confusion (2–4%) is no better, and its models are CC-BY-NC-4.0, not
shippable. pyannote's gated models were not reachable.

Still to measure: a phone on a real table, at 3–4 m, with the people present (AMI's array sits
in the middle of the table).

### In the final pass (2026-11-07)

The final pass now conditions what its diarization and voice prints hear
(`crates/engine/src/conditioning.rs`): a 100 Hz high-pass, a 7.5 kHz low-pass
(4th order), and one gain per range bringing its gated loudness (BS.1770) to
-23 LUFS, at most ±30 dB, half of the change from the last range's taken each
range, a 5 ms look-ahead limiter at -1 dBFS; the state carried from range to
range. The words are transcribed from the audio as it came: the ASR was not
measured on it. The gain above was not the loudness: `loudnorm` works at
192 kHz, and the round trip's resampler cuts the band above 7.5 kHz (-7 dB).
Raised 15–30 dB, the table microphone's hiss there makes Nemotron-3 worse: the
gain without the low-pass (static, or a 3 s short-term gain like `loudnorm`'s,
or `loudnorm`'s own gain curve applied at 16 kHz) changes nothing (IS1009a
20.9–21.5%, TS3003a 33.8–34.2%), and the band added back to `loudnorm`'s output
takes its gain away (21.7%, 34.7%); the low-pass without the gain changes
nothing either. DER / miss / false alarm / confusion, `chunked-linker` 60 s
0.45, `examples/diarize` without and with the conditioning (`--raw` is the
first):

| Meeting | Array1-01 raw | Array1-01 conditioned | Headset raw | Headset conditioned |
|---|---|---|---|---|
| IS1009a | 20.8 / 17.9 / 2.0 / 0.9 | 17.0 / 7.2 / 7.4 / 2.4 | 20.0 / 18.1 / 1.7 / 0.3 | 19.3 / 13.4 / 3.7 / 2.2 |
| ES2004a | 29.1 / 26.8 / 1.5 / 0.8 | 27.3 / 23.6 / 2.5 / 1.2 | 26.4 / 26.1 / 0.2 / 0.1 | 26.4 / 25.8 / 0.3 / 0.3 |
| TS3003a | 35.7 / 35.2 / 0.2 / 0.4 | 20.9 / 18.5 / 2.2 / 0.2 | 34.9 / 34.7 / 0.0 / 0.1 | 34.6 / 33.4 / 1.0 / 0.1 |
| EN2002a | 32.6 / 29.9 / 0.6 / 2.0 | 29.2 / 27.3 / 0.7 / 1.2 | 30.2 / 28.1 / 0.4 / 1.7 | 30.9 / 28.9 / 0.3 / 1.7 |

On the table microphone the miss falls 2.6–16.7 points, more than offline
(`highpass,loudnorm` over the whole file: 16.6, 29.6, 28.4, 32.1% DER), the
confusion moves within ±1.5 points (IS1009a up, EN2002a down), and the false
alarm rises where the gain is large (IS1009a 2.0 → 7.4%). Speakers found: 4, 6,
4, 6 (raw 4, 6, 4, 4). The headset mix is unchanged within noise, but for
IS1009a's confusion (0.3 → 2.2%), which the gate below removes. Single runs move by 1–2 points with any
small change of the audio, so read the larger moves. Settings tried, on the
table microphone: a 7 kHz low-pass did as well on IS1009a and TS3003a and worse
on ES2004a (confusion 5.9–6.1%); at most ±20 dB was better on TS3003a (19.5%)
and worse on ES2004a (31.8%); a target of -24 or -26 LUFS the same or worse
(ES2004a 47.9% at -26, confusion 19%); a 3 s short-term gain within ±10 dB of
the range's raised the headset's confusion (IS1009a 4.4%); voices embedded from
the audio as it came instead of conditioned, the same. Conditioning costs about
18 ms a minute of audio (one core of the dev Mac), nothing beside the
diarization's seconds.

**Only where it is needed: a gate (2026-11-07).** A range whose speech is
within 10 dB of -23 LUFS passes as it came (no filter, no gain: what `--raw`
gives); a channel is conditioned from its first range that needs more than
10 dB either way, and passes as it came again only after two ranges in a row
within 7 dB (silent ones, or ones with under 3 s of speech, not counted). The
filters and the loudness run on every range so that their state is the
channel's; a switch cross-fades over 0.5 s, the gain ramping from or to 0 dB.
DER / miss / false alarm / confusion, as above (`examples/diarize --gate 10`,
the default; `--gate 0` conditions every range):

| Meeting | Mic | Raw | Conditioned | Gated (10 dB) | Ranges conditioned |
|---|---|---|---|---|---|
| IS1009a | Array1-01 | 20.8 / 17.9 / 2.0 / 0.9 | 17.0 / 7.2 / 7.4 / 2.4 | 17.0 / 7.2 / 7.4 / 2.4 | 14 of 14 |
| IS1009a | Headset | 20.0 / 18.1 / 1.7 / 0.3 | 19.3 / 13.4 / 3.7 / 2.2 | 19.6 / 17.4 / 2.0 / 0.1 | 2 of 14 |
| ES2004a | Array1-01 | 29.1 / 26.8 / 1.5 / 0.8 | 27.3 / 23.6 / 2.5 / 1.2 | 27.2 / 23.6 / 2.5 / 1.2 | 18 of 18 |
| ES2004a | Headset | 26.4 / 26.1 / 0.2 / 0.1 | 26.4 / 25.8 / 0.3 / 0.3 | 26.3 / 25.8 / 0.3 / 0.3 | 18 of 18 |
| TS3003a | Array1-01 | 35.7 / 35.2 / 0.2 / 0.4 | 20.9 / 18.5 / 2.2 / 0.2 | 20.9 / 18.5 / 2.2 / 0.2 | 26 of 26 |
| TS3003a | Headset | 34.9 / 34.7 / 0.0 / 0.1 | 34.6 / 33.4 / 1.0 / 0.1 | 34.6 / 33.4 / 1.0 / 0.1 | 26 of 26 |
| EN2002a | Array1-01 | 32.6 / 29.9 / 0.6 / 2.0 | 29.2 / 27.3 / 0.7 / 1.2 | 29.2 / 27.3 / 0.7 / 1.2 | 36 of 36 |
| EN2002a | Headset | 30.2 / 28.1 / 0.4 / 1.7 | 30.9 / 28.9 / 0.3 / 1.7 | 31.0 / 28.9 / 0.3 / 1.8 | 34 of 36 |

The table microphone keeps the whole gain (it needs 7–41 dB, every range
conditioned); IS1009a's headset mix, the only one at speech level (it needs
-7 to +11 dB), is back to its raw confusion. The other three headset mixes are
quiet recordings (ES2004a needs 6–26 dB, TS3003a 17–43, EN2002a 5–25): they
are conditioned, gated or not, and did not suffer from it (within a point of
raw). So the gate keeps loud audio as it came, not close-talk audio as such: a
quiet close-talk recording is conditioned, and that is harmless here. The
noise floor, from a near-identity run (a 1 Hz high-pass, no gain): the four
headset mixes move by up to 0.4 points of DER and 0.9 of confusion. What did
IS1009a's headset harm, conditioned throughout: the band-pass alone gives
18.8 / 13.3 / 3.4 / 2.0, the gain alone (no band-pass) 20.8 / 18.1 / 1.8 / 1.0,
so the filters, not the gain; on the other headset mixes neither alone moves
DER by more than 1.3 points. Other gates: 6 dB conditions every range of
IS1009a's headset (it hovers 3–6 dB short), as harmful as no gate; 15 dB
passes it through whole but lets IS1009a's table microphone, at 7–14 dB
short past its first minutes, go back to as it came (19.3% DER, against 17.0).
