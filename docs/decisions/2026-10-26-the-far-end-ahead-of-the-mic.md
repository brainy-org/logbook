# The system channel given to AEC3 150 ms ahead of the mic

**Verdict.** The client's echo canceller (`crates/cli/src/aec.rs`) gives
AEC3 the system channel `LEAD` (150 ms) before the mic's sample of the
same time, not at the same time. On a real meeting the call reached the
mic 40 ms *before* the system channel had it on the session's clock;
AEC3 subtracts only an echo that comes after its far end, so it cancelled
nothing and the mic transcribed the call. The cost is 150 ms more before
the mic's live text.

## What was seen

2026-10-26, a real call (Italian, four people on the system channel),
MacBook speakers and mic, `logbook live --system`, AEC on, no gain; the
person never spoke. The mic's live transcript was a stream of fragments
the word filter could not match to the call: garbled Italian ("elettrica
di scalizzata" for "legacy virtualizzata"), English, Ukrainian and
Polish made out of the call's echo.

Measured on the pending meeting's audio (the mic as kept, after AEC):

- The mic, after AEC, at -22 dBFS (median, seconds with the call
  speaking) against the system channel's -20; -45 when the call was quiet.
- Cross-correlation of the two, 8 s windows every 30 s over 9 minutes:
  the mic's copy of the call **40 ms before** the system channel's in
  every window (-36 to -41.5 ms), 4 to 14 dB under it.

Why the system channel is late on the clock was not measured (the tap's
buffering, timestamps by arrival); only that it is, steadily.

## Why a lead

AEC3's linear filter models the echo as the far end delayed: an echo that
precedes its far end has no filter to subtract it, and only the
suppressor acts. On the unit test's noise, the echo's energy after AEC3:

| Echo vs system channel | Before (no lead) | With 150 ms lead |
|---|---|---|
| 30 ms after | -47.5 dB | -47.6 dB |
| at the same time | -47.6 dB | -45.6 dB |
| 10 ms before | -18.5 dB | -47.3 dB |
| 40 ms before | -18.4 dB | -46.1 dB |
| 100 ms before | -18.3 dB | -46.0 dB |
| 200 ms before | -18.1 dB | -18.6 dB |

18 dB of suppression on speech leaves the echo garbled rather than gone:
what the model turned into other languages. The tests now ask for 30 dB.

150 ms: over three times the 40 ms seen, still inside AEC3's delay range
for an echo after its far end (the lead adds to the speakers' and the
room's delay) and far inside its render queue (100 blocks; the lead is 15).

## Cost

- Latency: the mic waits for the system channel plus 150 ms (at most
  `IDLE_AFTER` plus 150 ms while nothing plays). The call's channel is
  unchanged. The echo filter on the server already holds the mic for the
  call's phrase to close (seconds), so with the call speaking it adds
  nothing seen.
- At the end of a session, the last 150 ms of the mic go out uncancelled.
- CPU, memory: none (15 blocks of AEC3's render queue).

## Not yet measured

- A real meeting with the lead: whether the mic's fragments are gone, and
  the person's words over the call kept.
- Other machines and devices (Bluetooth output adds delay the other way,
  which the lead does not hinder).

## Rejected

| What | Why not |
|---|---|
| Shift the system channel's timeline back by its latency | the latency is not known per device; a wrong guess moves every word of the call against the mic's, which the word filter and the final pass compare by time |
| A larger lead (500 ms) | latency for nothing measured, and closer to the end of AEC3's delay range |
| Tell AEC3 the delay (`set_stream_delay_ms`) | a hint for the external delay estimator, which is off; it cannot make an echo causal |
