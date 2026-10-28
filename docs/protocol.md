# The protocol

One WebSocket per live session, at `ws://HOST:3300/live`; final passes
also over HTTP, at `http://HOST:3300/finals`, and reports at
`http://HOST:3300/reports`. Types in
`crates/engine/src/protocol.rs`. Every one of them needs a person's
token ("Tokens", below).

## Client to server

1. A text frame, `start`, naming the channels:

   ```json
   {"type":"start","channels":[{"name":"mic","diarize":false},{"name":"system","diarize":true}]}
   ```

   `diarize` is per channel: off for one person (your microphone with
   headphones), on for the call's audio or a room. `echo_of`, optional,
   names the channel this one may hear again (`"system"` on a mic near
   the speakers): its echoed words are dropped (docs/pipeline.md, "Echo").
   `"final_pass": true`, optional, asks for the final pass after `stop`:
   the server records the audio for it and deletes it afterwards.
   `"polish": true`, optional, has the final transcript cleaned up on a
   server with a model for it (`--polish-url`), which then reads the
   meeting's text. Default `false`: as heard, and nothing sent to it.
   `"offset"`, optional (default 0): where on the meeting's clock the
   session's first sample is, in seconds; every time the server sends is
   moved by it. `"voices"`, optional: the meeting's speakers so far, as
   in `final_segments`; the final pass keeps their numbers. Both are for
   a session that takes a meeting's final pass up ("Reconnection").

2. Binary frames, the audio: one byte, the channel's index in `channels`,
   then 16 kHz mono PCM, signed 16-bit little-endian. The CLI sends 100 ms
   a frame (3.2 KB, 256 kbit/s a channel). Every channel's sample 0 is the
   session's start, and the client keeps them on that clock, sending
   silence for gaps (the CLI's `Timeline`): times of different channels
   are compared (echo, speakers), and a device that delivers nothing while
   idle would otherwise shift its channel.

3. A text frame, `{"type":"stop"}`, when there is no more audio.

## Server to client

- `{"type":"ready"}` once the channels' streams exist; audio sent before
  it waits in the socket.
- `{"type":"utterance", ...}` for each run of words from one speaker:

  ```json
  {"type":"utterance","channel":"system","speaker":0,"start":12.08,"end":15.11,
   "text":"Grazie Alice.","words":[{"w":"Grazie","start":12.08,"end":12.5,"conf":0.98}, ...]}
  ```

  Seconds from the session's start. `speaker` is 0-based in order of
  appearance on that channel, `null` on a channel that is not diarized or
  for words no turn covers. Utterances of one channel arrive in order;
  channels interleave freely, and a channel with `echo_of` lags behind
  the one it is filtered against.
- `{"type":"final_segments","until":118.4,"utterances":[...],"voices":[...]}` with a
  final pass, during the meeting, about a minute or so behind it: every
  channel's final segments that start before `until`, by start time, in
  the shape of `utterance` without the `type`. Never sent again, never
  changed: the next ones start at `until` or later. The final pass's
  speakers are numbered once for the whole meeting (not as the live ones).
  The last ones have `until` 1.8e308 (JSON has no infinity). On a server
  that cleans the transcript up (`--polish-url`), when `start` said
  `"polish": true`, `text` is the cleaned
  text and `words` stay as heard: the two may differ, and a segment of
  fillers alone may be missing. `voices` (absent when there is none): every
  speaker of the diarized channels so far, as
  `{"channel","speaker","embedding":[256 floats, L2-normalized],"seconds"}`,
  the mean of its WeSpeaker embeddings over `seconds` of audio where it
  spoke alone. The client names speakers from them
  (`docs/decisions/2026-10-27-speakers-named-by-voice.md`); they are
  voiceprints, not kept past the meeting's result.
- `{"type":"finalizing","audio_s":1834.2,"id":"9f3c…"}` with a final
  pass: the live transcript is complete; the final pass finishes the last
  minute or so (seconds). The whole result is also at `GET /finals/{id}`
  for an hour after it is ready.
- `{"type":"final","utterances":[],"warnings":[...],"voices":[...]}` with a final pass:
  it is complete (`utterances`: any segment not sent as `final_segments`;
  the server sends them all there); `warnings` says what was left out
  ("120–181 s of system was not transcribed again: ...").
- `{"type":"done"}` after `stop`, once every phrase (and the final pass,
  if asked) has been sent. The server then closes.
- `{"type":"error","message":"..."}` ends the session.

Closing the socket without `stop` drops the session, what it held and its
recording. Closing it after `stop` does not stop the final pass: its
result is kept under the `finalizing` id.

## Reconnection

A lost connection is the client's to repair; the server drops the
session, its recording and its final pass, and does not know the next
session is the same meeting. With a final pass, the client opens one
that takes it up: `offset` is where the last `final_segments` stopped
(their `until`, on a sample), `voices` the speakers they last sent. It
sends the audio it kept from there to now first (each channel in step,
as fast as the socket takes it), then the audio as it comes, and the
final pass goes on from `offset`: nothing before it is read again, the
speakers keep their numbers. Live utterances come again for what is
sent twice; the client passes on only those that end after the last it
showed. Without a final pass the new session has none, starts now and
the client moves its times by where it started
(`docs/decisions/2026-10-28-meetings-of-hours-and-days.md`).

## Final passes over HTTP

- `POST /finals`, multipart: a part `meeting` first, JSON
  `{"channels":[...],"polish":true,"offset":1834.2,"voices":[...]}` as in
  `start` (all but `channels` optional), then one part `audio` per
  channel in the same order, starting at `offset` on the meeting's clock:
  the client uploads from where the final segments it has stopped. Its
  times come back on the meeting's clock. Answers `202 {"id":"…"}`, or
  `400` with the reason. At most 4 GB (about 140 hours of two channels):
  the client encodes it and the server decodes it a part at a time, to
  its recordings folder.
- `GET /finals/{id}`: `{"state":"running","audio_s":…}`,
  `{"state":"done","utterances":[...],"warnings":[...]}` or
  `{"state":"failed","message":"…"}`; `404` if the server does not know
  the id (restarted, or the result expired an hour after it was ready):
  the client uploads again.

- `POST /reports`, JSON `{"title":"…","notes":"…","transcript":"…"}`
  (`title` and `notes` optional): the meeting's report by the server's
  model (docs/pipeline.md, "Report"). `transcript` is the final
  transcript as the client writes it, Markdown with the speakers' names.
  Answers `200 {"markdown":"…"}`; `501` on a server without a model
  (`--polish-url`); `502` with the reason when the model fails or the
  transcript is too long. At most 4 MB; as many at once as
  `--polish-parallel`, the others wait.

An `audio` part is the channel in Opus (`crates/engine/src/codec.rs`):
`LBO1`, the number of samples (u64 LE), the encoder's lookahead in
samples (u16 LE), then 20 ms packets at 32 kbit/s, each a u16 LE length
and its bytes. The decoder drops the lookahead and cuts at the number of
samples, so that sample i is the meeting's sample i.

## Tokens

`/live`, `/finals`, `/finals/{id}` and `/reports` need
`Authorization: Bearer <token>`, the token of a person (`lbk_` and 64
hex digits), on the WebSocket's upgrade request as on every other; `401`
without one the server knows. The client sends it from `--server-token`
(`LOGBOOK_SERVER_TOKEN`) or the app's settings (kept in its `secrets.json`). The
server logs whose token opened each session, upload and report.
`--no-auth` lets anyone in, for a server only its own computer reaches.

They are made and revoked on the admin page, `GET /` (its typeface at `/assets/geist.woff2`, brainy's look), which asks
for the admin token (`--admin-token`, or the one made at first start
in `admin.token` in the data folder, `--data`) and calls:

- `GET /admin/api/tokens`: `[{"id","name","created","last_used"}]`
  (seconds since the epoch; `last_used` noted at most once a minute).
- `POST /admin/api/tokens`, `{"name":"Luca"}`: `201
  {"id","name","token"}`, the only time the token is seen.
- `DELETE /admin/api/tokens/{id}`: `204`; refused at once from then on.

The server keeps only each token's SHA-256, in `tokens.json` (mode 600)
in the data folder (`crates/server/src/auth.rs`).

## Not yet

TLS: tokens, audio and transcripts cross the network in clear unless a
proxy in front of the server (Caddy, nginx, the cluster's ingress)
serves HTTPS. A final pass's result is not tied to the token that asked
for it: any token with its id (128 random bits, known only to that
client) reads it. No limits per person.
