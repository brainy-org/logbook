# The call's audio through cpal, on the session's clock

**Verdict.** `logbook live --system` records what the computer plays with
cpal 0.18 by opening an input stream on an output device: cpal makes a
Core Audio process tap and a private aggregate device on macOS (14.6+),
and a WASAPI loopback on Windows. On Linux, PipeWire's "Monitor of"
devices are inputs, picked by name (`--system-device`). Only macOS has been
tried. Each channel is
kept on the session's clock by the client, because macOS's tap delivers
nothing, not silence, while nothing plays.

## Why cpal

Since 0.17 cpal does the loopback itself on macOS (`host/coreaudio/macos/
loopback.rs`: `CATapDescription`, `AudioHardwareCreateProcessTap`, an
aggregate device destroyed with the stream), as it long has on Windows.
One API for the microphone and the call on every platform, no Objective-C
of our own. ScreenCaptureKit would ask for screen-recording permission to
get sound; the tap asks only for system audio recording.

## The tap is silent when the output is idle

Measured on macOS 27.0.1 (MacBook Air M3, built-in speakers): `live
--system` started 2 s before `afplay` played a clip. The first samples
arrived when the clip started, so the channel's second 0 was the clip's
start: times were 2 s early, latencies read 2 s too high, and a quiet
stretch in a call would vanish from the timeline (and from the VAD, which
needs the pause to close a phrase). After the clip the device kept
delivering silence for a while, then stops like any idle output.

The fix is on the client (`Timeline` in `crates/cli/src/audio.rs`): every
channel's sample 0 is the session's start, and when the samples sent fall
more than 0.5 s behind the time elapsed, the gap is filled with silence
before the new samples. Measured again: the clip's first phrase at 2.6 s,
the microphone's and the system's copies of a phrase at the same second,
latency back to about 0.9 s.

## Echo

Without headphones the microphone hears the speakers: every phrase of the
call came twice, once per channel (same test, both channels). The server
now drops it by comparing words (`docs/pipeline.md`, "Echo"), which the
shared clock makes possible. The options weighed:

| What | For | Against |
|---|---|---|
| Echo cancellation on the client (Apple's VoiceProcessingIO, WebRTC AEC on Windows and Linux) | removes the call from the mic signal, keeps your voice while others speak | platform code per OS; cpal does not expose it |
| **Dropping on the server the mic's words that the call said at the same time** (chosen) | one place, every platform, no signal processing | holds the mic while the call speaks; fails where both copies are transcribed badly |
| Asking for headphones | nothing to build | the person forgets |

Measured with the 45 s Italian recording on the speakers: the natural
voice's echo was dropped whole; the robotic voice, garbled differently on
each channel, left three fragments. Client-side cancellation stays the
step after, for the desktop app.
