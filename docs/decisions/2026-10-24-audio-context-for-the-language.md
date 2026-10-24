# Each live phrase is transcribed with the audio before it

**Verdict.** A live clip carries up to 3 s (`--context`) of the channel's
audio before its phrase; the words that belong to the previous phrase are
dropped from the transcript. The model then picks the phrase's language
with the sentence around it, instead of from one or two words.

## Why

Two people talking Italian at the laptop's mic (live, AEC on): among the
Italian phrases came "I need to skip a money.", "J'en fais moi.", "I
don't think.". tdt-0.6b-v3 detects the language per clip and takes no
hint (`parakeet_capi_*_lang` is ignored by non-prompt models); a phrase
of a second has too little to go on.

Reproduced with macOS's Italian voice, quiet, in pink noise: "Ma va là."
alone came out "Mais voilà." / "Ma voilà." (live and final pass alike);
after 3 s of Italian in the same clip, "Ma va là." AEC was ruled out:
the same voice through the canceller with a silent far end transcribed
identically.

## The boundary

Word times run early after silence (a first word at 0.64–0.96 s for
speech from 1.0 s; "Ma" at 3.62–3.86 for a phrase the VAD started at
3.76), so cutting at the phrase's own start lost first words ("Non",
"Ma", "Well"). The cut is at the previous phrase's end by the VAD: its
last word ends 0.3 s or more before the next phrase in the cases
measured, and a word centred before that point is that phrase's. Without
a previous phrase in the clip nothing is dropped.

## Cost, measured 2026-10-24

Dev Mac (M3), `bench` LibriSpeech fixture, 8 sessions: latency p50 0.99
→ 1.15 s, p90 1.49 → 1.47 s. Each clip grows by up to 3 s of audio:
transcription work per phrase up to about three times a 1.5 s phrase's.
Batching absorbs it on this load; on a busy GPU it is the first thing to
lower (`--context 1`, or 0). A phrase with nothing before it (the first
of the meeting, or after a long silence) is not helped: "Ma voilà." stays.

## Rejected

| What | Why not |
|---|---|
| Forcing the language | the model has no language input |
| A multilingual prompt model (nemotron ASR) | a different model from brainy-worker's, and not in the mirror |
| Dropping words of another language | would drop the real foreign words of an Italian meeting, and a misread phrase is not a set of foreign words but the same sounds read wrongly |
| Longer phrases (`--min-pause` higher) | text comes later for every phrase, and a short reply stays short |
