# A 100 ms WASAPI buffer on Windows

**Verdict.** On Windows the client opens every capture stream, the
microphone and the call's loopback, with `BufferSize::Fixed` of a tenth of
a second of the device's rate (`capture`, client `audio.rs`). With cpal's
default, Remote Desktop's microphone lost over half its frames and nothing
it heard was transcribed. macOS and Linux keep the default: there a fixed
size sets the period, and with it the latency.

## What was seen

The first run on Windows: Windows 11 reached over Remote Desktop, both
devices named "Audio remoto" (the client's microphone and speakers,
redirected). `live --system` transcribed what the computer played; `live`
on the microphone printed nothing, and cpal's error callback reported "A
buffer underrun or overrun occurred" on about two callbacks out of three.

A probe with cpal 0.18.2 alone, the microphone's only config (2 channels,
44 100 Hz, buffer 441 to 441 frames), three seconds each:

| Stream | Frames a second | Of nominal | Errors a second |
|---|---|---|---|
| default buffer (441 frames, 10 ms) | 20 370 | 0.46 | 42 |
| 1 channel, or 22 050, 16 000, 48 000 Hz | | 0.45 to 0.48 | 40 to 43 |
| `Fixed(4410)` (100 ms) | 42 336 | 0.96 | 0 |
| `Fixed(22050)`, `Fixed(44100)` | 42 336 to 42 924 | 0.96 to 0.97 | 0 |

The format was not the cause: every rate and channel count lost the same
share. cpal's WASAPI capture (`host/wasapi/stream.rs`) passes each packet
on even when `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` is set and reports
the flag as an xrun: the frames were already gone in Windows. The
redirected microphone arrives from the network in bursts larger than one
10 ms period and overflows a buffer of exactly one period.

With half the frames, the channel's audio was compressed in time and the
session's clock (`Timeline::gap`) filled what was missing with silence:
speech the model could not read. With the 100 ms buffer, `live` on the
same microphone transcribed Italian speech at the usual latency (about
2 s in a debug build) with no error lines.

## Why 100 ms, and why only Windows

In shared event-driven mode the buffer is room, not delay: the callbacks
kept arriving about 96 times a second with every size tried, each with
what was ready. A tenth of a second covers the bursts seen with margin;
cpal does not check `Fixed` against the supported range on Windows (it
reports 441 to 441 and opened 44 100).

On macOS, `Fixed` sets Core Audio's I/O period: a 100 ms period would add
latency for nothing, since the built-in and USB devices there did not lose
frames. Linux's ALSA likewise ties the size to the period.
