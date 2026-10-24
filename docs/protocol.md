# The live protocol

One WebSocket per meeting, at `ws://HOST:3300/live`. Types in
`crates/engine/src/protocol.rs`.

## Client to server

1. A text frame, `start`, naming the channels:

   ```json
   {"type":"start","channels":[{"name":"mic","diarize":false},{"name":"system","diarize":true}]}
   ```

   `diarize` is per channel: off for one person (your microphone with
   headphones), on for the call's audio or a room. `echo_of`, optional,
   names the channel this one may hear again (`"system"` on a mic near
   the speakers): its echoed words are dropped (docs/pipeline.md, "Echo").

2. Binary frames, the audio: one byte, the channel's index in `channels`,
   then 16 kHz mono PCM, signed 16-bit little-endian. The CLI sends 100 ms
   a frame (3.2 KB, 256 kbit/s a channel). Channels are independent: each
   has its own clock, starting at its first sample.

3. A text frame, `{"type":"stop"}`, when there is no more audio.

## Server to client

- `{"type":"ready"}` once the channels' streams exist; audio sent before
  it waits in the socket.
- `{"type":"utterance", ...}` for each run of words from one speaker:

  ```json
  {"type":"utterance","channel":"system","speaker":0,"start":12.08,"end":15.11,
   "text":"Grazie Alice.","words":[{"w":"Grazie","start":12.08,"end":12.5,"conf":0.98}, ...]}
  ```

  Seconds from the channel's first sample. `speaker` is 0-based in order of
  appearance on that channel, `null` on a channel that is not diarized or
  for words no turn covers. Utterances of one channel arrive in order;
  channels interleave freely, and a channel with `echo_of` lags behind
  the one it is filtered against.
- `{"type":"done"}` after `stop`, once every phrase has been sent. The
  server then closes.
- `{"type":"error","message":"..."}` ends the session.

Closing the socket without `stop` drops the session and what it held.

## Not yet

Compression (Opus), reconnection with a resume point, authentication: the
prototype trusts its network.
