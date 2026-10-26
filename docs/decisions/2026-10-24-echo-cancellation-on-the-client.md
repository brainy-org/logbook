# Echo cancellation on the client, before the word filter

**Verdict.** Without headphones, the CLI cancels the call from the
microphone with WebRTC's AEC3 (`sonora`, a pure Rust port, BSD-3), using
the system channel as the far end, before the audio is sent or kept. The
server's word filter (`docs/pipeline.md`, "Echo") stays, for what AEC3
leaves. `--no-aec` turns it off to compare.

## Why

Tested with a YouTube video playing on the MacBook's speakers and the
person talking over it: the person's words were lost whenever the video
spoke (24 s to 2:27 of the test), and came through in its pauses. The word
filter compares the two transcripts; with the speakers louder than the
person at the mic, the mic's transcript was the video's (seen with the
filter's debug log: `RUST_LOG=logbook_server=debug`, every mic phrase the
video's words), so the filter dropped it whole, and there was nothing of
the person's left to keep. A filter on words can drop the call's words,
not recover the words under them: the sound has to be cleaned first.

## How

`crates/cli/src/aec.rs`. Both channels are on the session's clock, so the
mic's sample i is processed after the system channel's sample i + 150 ms
has been given to AEC3: the mic waits for the system channel to reach it,
and 150 ms more, because on a real meeting the call reached the mic before
the system channel had it on the clock
(`docs/decisions/2026-10-26-the-far-end-ahead-of-the-mic.md`; until then,
sample i after sample i). AEC3 estimates the rest of the delay (the
output's latency, the room) by itself; the stream delay is set to 0. While
nothing plays, the system channel is filled with silence after 0.25 s
(`IDLE_AFTER`, audio.rs, down from 0.5): that wait, plus the 150 ms, is
what the mic's live text loses at most.

## Measured, 2026-10-24

- Synthetic (unit test): noise played back 30 ms later at half level, the
  echo's energy fell by 47 dB once converged; 6 s processed in under
  50 ms in release (over 100 times real time on one core).
- MacBook Air M3, built-in speakers and mic, the LibriSpeech fixture
  played with `afplay`, `live --system`, nobody talking: without AEC the
  mic transcribed the 7 phrases of the fixture (all dropped by the word
  filter); with AEC, none of them reached the mic's transcript, one
  "Yeah." did.
- Talking over a call with AEC: not yet measured.

## Rejected

| What | Why not |
|---|---|
| Only the word filter | it cannot give back the person's words under the call's (above) |
| webrtc-audio-processing (C++ WebRTC) | the same AEC3, but a C++ build with meson on every platform |
| macOS Voice Processing I/O | macOS only, and unclear whether it cancels other apps' audio; Windows and Linux would need another anyway |
| AEC on the server | the server would need the mic and the system channel raw anyway, and the cost per sample of every meeting on the server rather than on each laptop |
