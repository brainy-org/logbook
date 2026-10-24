# A live server, phrases cut by VAD, an offline model

**Verdict.** Logbook transcribes meetings live on a server of our own (the
customer's, next to brainy), not on the laptop and not through a
third-party API. The client streams raw audio over a WebSocket; the server
cuts it into phrases with a streaming VAD, transcribes each phrase with the
offline parakeet-tdt-0.6b-v3, batched across sessions, and gives words a
speaker from a streaming diarization. Rust for both sides, on one engine
crate, so that a local mode can reuse it later.

## Why a server

- **The audio stays the customer's** either way: the server is
  self-hosted. Granola, for comparison, captures locally but streams the
  audio to Deepgram or AssemblyAI.
- **Same quality on every client.** A laptop without a GPU (most Windows
  ones) cannot be counted on to run a 0.6B model live; a shared GPU can.
- **Thin clients**: no 1 GB of models, no first-run benchmark, mobile and
  browser become possible.
- **Batching**: phrases of many meetings share one transcription call.

What it costs: no transcription offline, a server sized for the meetings
held at the same time, a client that must buffer through a bad network.

## Why phrases and an offline model

parakeet.cpp's streaming ASR (`parakeet_capi_stream_*`) needs a cache-aware
streaming model, such as `parakeet_realtime_eou_120m`, which is English
only. tdt-0.6b-v3 reads 25 languages, Italian included, but only whole
clips. Cutting at pauses and transcribing each phrase gives text about a
second after the speaker stops, which is what meeting notes need; partial
words while someone speaks are not. Diarization, on the other hand,
streams natively (`parakeet_capi_diarize_stream_*`, Nemotron-3-Diarization
in its 1.04 / 0.64 / 0.32 s modes).

## Why not brainy-worker

brainy-worker reads recordings as jobs: a Redis stream, a disposable child
per job, models loaded per job. A live session is the opposite: models
always loaded, a connection open for the whole meeting, answers in
hundreds of milliseconds. It stays the place for the final, offline pass
over a whole recording.

## Rejected

| What | Why not |
|---|---|
| parakeet's streaming speaker-attributed ASR (`sas_stream`) | transcribes per session in the session's thread: no batching across meetings |
| parakeet's own streaming ASR | needs a streaming model; the one available is English only |
| Linking libparakeet at build time | the release ships prebuilt libraries per platform and GPU; loading at run time picks one without rebuilding |
| Transcribing on the client | quality and latency tied to the hardware; kept as a later local mode on the same engine |

## Measured, 2026-10-24

On the dev Mac (Apple M3, Metal, the models of brainy-worker's pinned
revision), with `logbook play`:

- LibriSpeech fixture (brainy-worker's `two-speakers-en.mp3`, 23.6 s, two
  readers in turn): the four turns transcribed as offline, the two voices
  told apart; text 0.5–1.4 s after each phrase's end with diarization,
  0.5–1.0 s without.
- 45 s in Italian made with macOS voices: the natural voice exact, the
  formant voice garbled (as brainy-worker found: not speech the model was
  trained on).
- `logbook bench`: 4 sessions kept up (p90 1.4 s); 16 did not (latency
  grew to 35 s): a phrase that takes 350 ms alone took 4.5 s, the GPU being
  shared with 16 diarization streams. The server will not run on a Mac:
  capacity is to be measured on the target GPU.
