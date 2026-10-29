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
   `"offset"`, optional (default 0): where on the meeting's clock the
   session's first sample is, in seconds; every time the server sends is
   moved by it. `"voices"`, optional: the meeting's speakers so far, as
   in `final_segments`; the final pass keeps their numbers. Both are for
   a session that takes a meeting's final pass up ("Reconnection").
   `"codec":"opus"` with `"lookahead":N`, optional: the client would send
   the audio in Opus, whose encoder delays it by N samples ("The audio",
   below). Without them, or with a codec the server does not know, the
   audio is PCM.

2. Binary frames, the audio, in the codec `ready` agreed on ("The
   audio"). The client sends 100 ms a frame. Every channel's sample 0 is
   the session's start, and the client keeps them on that clock, sending
   silence for gaps (the CLI's `Timeline`): times of different channels
   are compared (echo, speakers), and a device that delivers nothing while
   idle would otherwise shift its channel.

3. A text frame, `{"type":"stop"}`, when there is no more audio.

## Server to client

- `{"type":"ready"}` once the channels' streams exist; audio sent before
  it waits in the socket. `{"type":"ready","codec":"opus"}` when `start`
  asked for Opus and the server takes it: the audio is Opus. A bare
  `ready` means PCM, whatever `start` asked (an older server ignores
  `codec` and answers a bare one).
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
  The last ones have `until` 1.8e308 (JSON has no infinity). `voices` (absent when there is none): every
  speaker of the diarized channels so far, as
  `{"channel","speaker","embedding":[256 floats, L2-normalized],"seconds"}`,
  the mean of its WeSpeaker embeddings over `seconds` of audio where it
  spoke alone. The client names speakers from them
  (`docs/decisions/2026-10-27-speakers-named-by-voice.md`); they are
  voiceprints, not kept past the meeting's result.
- `{"type":"finalizing","audio_s":1834.2,"id":"9f3c…"}` with a final
  pass: the live transcript is complete; the final pass finishes the last
  minute or so (seconds). Its result is also at `GET /finals/{id}` for an
  hour after it is ready: the segments of its last ten minutes or so
  (`from`), not the whole meeting's.
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

## The audio

Both codecs start every binary frame with one byte, the channel's index
in `channels`. Then:

- **PCM** (no codec agreed): 16 kHz mono, signed 16-bit little-endian.
  100 ms is 3.2 KB: 256 kbit/s a channel.
- **Opus** (`"codec":"opus"` in `ready`): the number of samples the frame
  stands for (u32 LE), then the 20 ms packets those samples completed
  (16 kHz mono, VoIP, 32 kbit/s), each a u16 LE length and its bytes, as
  in an upload. 100 ms is about 415 bytes: 33 kbit/s a channel with the
  framing. The encoder keeps samples short of a packet for the next
  frame (a frame is not always a multiple of 20 ms: the `Timeline` fills
  gaps), and may send a frame with no packet. Before `stop` the client
  sends each channel a last frame of 0 samples whose packets flush its
  encoder: the remainder, and `lookahead` samples of silence, padded to a
  whole packet.

  The server decodes each channel as its frames come and passes on
  exactly as many samples as the frames said, in all: it drops the first
  `lookahead` it decodes and nothing past the count (the padding). So
  channel i gets sample for sample what the client captured, and the
  channels stay on one clock. The last `lookahead` samples of each frame
  (6.5 ms) come with the next one: the only delay Opus adds. Each session
  has its own encoder and decoder per channel: a session opened again
  after a lost connection starts new ones, the kept audio it is sent
  again included.

  `codec.rs` in the engine (`LiveEncoder`, `LiveDecoder`);
  `docs/decisions/2026-10-29-opus-on-the-live-socket.md`. The CLI's
  hidden `--live-codec pcm` (`LOGBOOK_LIVE_CODEC=pcm`) asks for PCM, to
  compare or to debug.

## Reconnection

A lost connection is the client's to repair; the server drops the
session, its recording and its final pass, and does not know the next
session is the same meeting. It drops it at once, the audio still queued
for it included (engine `session.rs`, `Feed`): a session behind the
audio would otherwise transcribe those minutes for nobody, beside the
one that takes the meeting up. Until its `stop` a client sends audio all
the time, silence too: a socket that brings nothing for 30 s is a client
gone without closing it (its network dropped), and is closed. With a final pass, the client opens one
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
  `{"channels":[...],"offset":1834.2,"voices":[...]}` as in
  `start` (all but `channels` optional), then one part `audio` per
  channel in the same order, starting at `offset` on the meeting's clock:
  the client uploads from where the final segments it has stopped. Its
  times come back on the meeting's clock. Answers `202 {"id":"…"}`, or
  `400` with the reason. At most 4 GB (about 140 hours of two channels):
  the client encodes it and the server decodes it a part at a time, to
  its recordings folder.
- `GET /finals/{id}`: `{"state":"running","audio_s":…}`,
  `{"state":"done","utterances":[...],"warnings":[...],"voices":[...],"from":5400.0}` or
  `{"state":"failed","message":"…"}`. `from` (seconds, on the meeting's
  clock): every segment starting at or after it is in `utterances`, none
  before. An upload's result starts at its `offset`; a live session's
  keeps only the last ten minutes or so of what it sent (engine
  session.rs, `KEEP_SENT`), so a client
  whose final segments stop before `from` has a gap and uploads its
  audio from where they stop, as for a `404`. Missing (a server older
  than it) is 0: everything. `404` if the server does not know
  the id (the server that ran it stopped, or the result expired an hour
  after it was ready): the client uploads again. Any replica answers
  (`docs/decisions/2026-10-29-replicas-share-the-data-folder.md`); `503`
  if it cannot read the data folder, which the client retries.

- `POST /reports`, JSON `{"title":"…","notes":"…","transcript":"…"}`
  (`title` and `notes` optional): the meeting's report by the server's
  model (docs/pipeline.md, "Report"). `transcript` is the final
  transcript as the client writes it, Markdown with the speakers' names.
  Answers `200 {"markdown":"…"}`; `501` on a server without a model
  (`--llm-url`); `502` with the reason when the model fails. At most
  64 MB (a long meeting is reported in steps, one request to the model
  each, and holds its place for all of them); as many at once as
  `--report-parallel`, the others wait.

- `POST /tasks`, JSON `{"title":"…","notes":"…","transcript":"…","date":"2026-10-29"}`
  (`title` and `notes` optional; `date` the meeting's day, from which
  a due date is resolved): the tasks agreed in the meeting, found by the
  server's model (docs/pipeline.md, "Tasks"). Answers `200 {"tasks":
  [{"task":"…","owner":"Luca"|null,"due":"2026-11-06"|null,"at":754}]}`
  (`at`, seconds from the meeting's start of the turn where it was
  agreed; `owner` a speaker's label in the transcript), in order; an
  answer of the model that could not be read is `{"tasks": []}`. `501`
  without a model, `502` when it cannot be reached; same limits as
  `/reports`, and it takes one of its `--report-parallel` places. A
  server older than it answers `404`: the client publishes without
  tasks.

An `audio` part is the channel in Opus (`crates/engine/src/codec.rs`):
`LBO1`, the number of samples (u64 LE), the encoder's lookahead in
samples (u16 LE), then 20 ms packets at 32 kbit/s, each a u16 LE length
and its bytes. The decoder drops the lookahead and cuts at the number of
samples, so that sample i is the meeting's sample i.

## Tokens

`/live`, `/finals`, `/finals/{id}`, `/reports` and `/tasks` need
`Authorization: Bearer <token>`, the token of a person (`lbk_` and 64
hex digits), on the WebSocket's upgrade request as on every other; `401`
without one the server knows, `503` if it cannot read its tokens. The client sends it from `--server-token`
(`LOGBOOK_SERVER_TOKEN`) or the app's settings (kept in its `secrets.json`). The
server logs whose token opened each session, upload and report.
`--no-auth` lets anyone in, for a server only its own computer reaches.

They are made and revoked on the admin page, `GET /` (its typeface at `/assets/geist.woff2`, brainy's look), which asks
for the admin token (`--admin-token`, or the one made at first start
in `admin.token` in the data folder, `--data`) and calls:

- `GET /admin/api/tokens`: `[{"id","name","created","last_used"}]`
  (seconds since the epoch; `last_used` noted at most once a minute by
  each replica; `id` is the token's SHA-256).
- `POST /admin/api/tokens`, `{"name":"Luca"}`: `201
  {"id","name","token"}`, the only time the token is seen.
- `DELETE /admin/api/tokens/{id}`: `204`; refused at once from then on,
  by every replica.

The server keeps only each token's SHA-256, a file each in `tokens/` in
the data folder (mode 600), which replicas may share
(`crates/server/src/auth.rs`). A `tokens.json` of an older server is
moved there at start.

## Not yet

TLS: tokens, audio and transcripts cross the network in clear unless a
proxy in front of the server (Caddy, nginx, the cluster's ingress)
serves HTTPS. A final pass's result is not tied to the token that asked
for it: any token with its id (128 random bits, known only to that
client) reads it. No limits per person.
