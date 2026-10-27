# Speakers named by voice, with the encoder logbook already has

**Verdict.** The call's speakers are named on the page ("Luca" rather than
"Speaker 2") from voices the person named before, with the speaker
encoder the final pass already runs (WeSpeaker ResNet34-LM, the mirror's
`parakeet-voice`). The engine sends every speaker's voice with the final
segments. The CLI matches them against a library kept only on this
computer, and `logbook speakers name MEETING SPEAKER NAME` adds a voice to
it from one of the last meetings. A speaker takes a name only when its
voice is close to one person's and not to anyone else's. Otherwise it
keeps its number, with the name as a question ("Speaker 2 (Luca?)") when
there is one. No other encoder measured beat ResNet34, so nothing new was
mirrored.

## How it works

- `speakers::Linker` keeps each meeting speaker's centroid (the sum of its
  range embeddings weighted by seconds) and the seconds behind it.
  `Progressive::voices` gives them L2-normalized as `SpeakerVoice`, and
  `final_segments`, `final` and `GET /finals/{id}` carry them
  (`docs/protocol.md`). This costs a few kB a minute and no compute: the
  embeddings were already made to link the ranges.
- The CLI (`crates/cli/src/voices.rs`) keeps `voices.json` in the data
  folder: per person, one print (embedding, seconds, meeting) per time
  they were named. A person's score is their best print's cosine.
- A speaker's label is chosen at its first segment and kept for the
  meeting, because the page is written once (`transcript.rs`). A speaker
  whose voice has less than 8 s behind it at that point stays numbered.
- A published meeting's speakers (label, voice) go to
  `voices/<meeting>.json`, for the last 20 meetings, to be named later.

## What was measured

There were two sets, real speech in both, and the same person was always
compared across two sessions:

- **AMI:** 190 people, 682 person-meetings; the same people across the
  four meetings of a series. Headset (`ihm`) and table microphone (`sdm`).
- **VoxPopuli, Italian:** 164 MEPs across plenary sessions.

A voice is the mean of 10 s chunk embeddings weighted by seconds, as
`Linker` makes it. The enrollment is 30 s from one session; the test is
30 s (or 10 s, 5 s, 3 s) from another. Two measures:

- **EER** of all cross-session pairs.
- **The naming game:** a library with half of the people, each enrolled
  from their first session; every speaker of every other session asks for
  a name. The best match above a threshold wins, one name per speaker per
  session. The game reports the share named right when at most 0.5% of
  answers are wrong (counting unknown people given a name as wrong).

| Encoder | AMI headset | AMI headset → table | AMI table | AMI headset, 3 s test | Italian | Italian, 3 s test | Game: headset | Game: → table | Game: Italian |
|---|---|---|---|---|---|---|---|---|---|
| **WeSpeaker ResNet34-LM (kept)** | 0.99% | 1.31% | 1.58% | 3.41% | 0.24% | 0.70% | 94.9% | 84.1% | 87.2% |
| ResNet34-LM as libparakeet runs it (povey window) | 0.99% | 1.30% | 1.62% | 3.46% | – | – | 95.1% | 83.2% | – |
| ReDimNet2-B2 (VoxCeleb2) | 0.99% | 1.71% | 2.33% | 3.26% | – | – | 95.0% | 91.3% | – |
| 3D-Speaker CAM++ (VoxCeleb) | 0.99% | 1.75% | 2.08% | 3.55% | 0.24% | 0.65% | 94.9% | 79.4% | 89.3% |
| 3D-Speaker CAM++ (zh+en) | 1.48% | 2.61% | 2.33% | 3.46% | 0.49% | 1.27% | 96.2% | 72.1% | 87.1% |
| 3D-Speaker CAM++ (200k zh; voice-detect's GGUF) | 2.07% | 7.49% | 5.46% | 5.87% | 2.89% | 4.28% | 84.7% | 16.9% | 68.5% |

- The amount of speech matters more than the encoder: 3 s of test speech
  triples the error of 30 s. So does the microphone: every encoder is worse
  from a headset to a table microphone.
- AS-norm (score normalization against a cohort) did not help. It gave no
  gain with a cohort from the same corpus, and lost much with one from the
  other corpus (AMI EER 0.99% → 1.9%). A shipped cohort would be of the
  second kind.
- libparakeet's voice-detect computes ResNet34's features with a povey
  window, while WeSpeaker trained with hamming. Its embeddings match the
  ONNX reference fed povey features to cos 1.0000 and hamming ones to
  0.995–0.998. The measured cost is nil (the two rows above), so nothing
  was changed.
- voice-detect's CAM++ GGUF matches its reference on 10 s clips but not
  on 3–5 s ones (cos down to 0.92). It is unexplained, and it is another
  reason not to switch.
- Results move a lot with the speakers sampled: on half of AMI's speakers
  the EER is 0.19%, against 0.99% on all of them. Differences of a few
  tenths between encoders here are not significant.
- Thresholds (`voices.rs`): pairwise, one stranger in a thousand matches at
  0.52–0.63 depending on the condition, hence MAYBE = 0.55. In the game,
  with a library of ~95 people, wrong names stay under 0.5% at 0.66–0.69
  on AMI and at 0.78 on the Italian set. A personal library is smaller,
  hence SURE = 0.70, with a 0.10 margin over the second person.
- End to end: two AMI meetings of the same four people (ES2004a and b,
  table microphone, 4 min each, `play --final` without a server). One
  speaker was named after the first meeting from 10 s of voice. In the
  second meeting, the speaker the CLI called by that name was that person
  in 5 of the 7 headings checked against the reference, and another
  person in the other 2. The check is coarse: only a heading's time was
  compared, and the audio was cut from AMI's segments, not a real call.

## Not measured, for lack of time

- ResNet293-LM, ERes2NetV2, ReDimNet2 B3–B6.
- The Opus 16 kbit/s condition (a call's audio).
- Bootstrap intervals on every encoder.

These need the GPU server, not a laptop: the larger encoders run 0.5–1 s
of CPU per 10 s of audio, and the whole set took hours.

## Why not

| What | Why not |
|---|---|
| A larger encoder (ResNet293, ReDimNet2-B6) | not measured past the screening; 5–7× the compute per embedding; voice-detect cannot run ReDimNet2 or ERes2NetV2 |
| Encoders trained on VoxBlink2 | the data is CC BY-NC-SA 4.0: no commercial use |
| The library on the server | voiceprints of people who never used logbook; the client owns them, as it owns the meeting's audio |
| Naming in the engine (`Linker` seeded with known voices) | would change how ranges are linked, measured for one meeting; a name is the client's choice, and needs no compute there |
| Renaming a speaker on a page already written | the page is written once (`published_until`); naming affects the meetings that follow |
| One threshold | a wrong name is worse than a number: the doubtful case is shown as a question instead |

## Costs

- **Privacy:**
  - Voiceprints are biometric data. They stay on the person's computer:
    the library, plus the last 20 meetings' voices.
  - The server sends a meeting's voices during that meeting and keeps
    them with the final result for an hour, like the transcript.
  - `logbook speakers forget NAME` removes a person.
- **A label is fixed at first sight.** Someone who speaks little at first
  stays "Speaker N" for the whole meeting, even if later minutes would
  have named them.
