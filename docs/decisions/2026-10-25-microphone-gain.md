# The microphone's level raised on the client, by WebRTC's AGC2, on request

**Verdict.** `logbook live --mic-gain auto|DB|off` passes the microphone
through AGC2 (sonora, the WebRTC port that already cancels the echo,
`crates/cli/src/aec.rs`) before it is sent: `auto` is the adaptive digital
controller (up to 50 dB, 6 dB a second), a number a fixed gain in dB (0 to
under 50, AGC2's range), both with AGC2's limiter. Off by default: the
level is the device's, as before.

## Why

A quiet microphone, or a voice across the room, reaches the server some
25 dB down, and the VAD and the model lose words. Measured on the
committee recording (98 s, Italian, synthetic) turned down 25 dB, final
pass, one run each:

- **As recorded**: the reference.
- **25 dB down**: -44 dBFS; "Argo costa 62.000" became "Faro costa
  62.000", "Sono Davide Serra" became "Sono già in sede".
- **25 dB down, `auto`**: -22 to -24 dBFS, peak 26 184 (no clipping);
  those errors gone, the text as good as the reference ("il budget
  regge", where the reference had "range").

## How

- After the echo canceller, in the same APM: the call's echo is taken out
  before the voice is raised, not raised with it.
- Without `--system` the mic goes through the APM alone, block by block,
  waiting for no far end.
- AGC2's adaptive controller raises only what its own VAD takes for
  speech, so silence and steady noise are not pumped up (a test tone is
  not raised at all: the unit test checks the fixed gain only).

## Rejected

- **A VAD threshold per channel on the server**: the other meaning of
  "sensitivity", but a protocol change, and a lower threshold lets noise
  through as speech; raising the level is what the measurement asked for.
- **`auto` by default**: not yet tried on a real meeting with a near,
  loud microphone; changing what every meeting sends deserves that first.
