# The final transcript cleaned up by a language model, off by default

**Verdict.** logbook-server can send the final segments to an
OpenAI-compatible `/chat/completions` endpoint, whose model removes
fillers, false starts and the words a speaker took back, and punctuates,
as Wispr Flow does for dictation (`crates/engine/src/polish.rs`,
`crates/server/src/polisher.rs`). Only `text` changes; speakers, times and
`words` stay as heard. Every answer is checked against what was heard, and
a segment the model rewrote keeps the heard text. It is off unless
`--polish-url` and `--polish-model` are set, and then only for a meeting
that asks for it (`"polish": true` in `start` or the upload; the CLI's
`--polish`): another meeting's text never reaches the model. Of the models tried,
Qwen3.8-27B (on Groq) cleans well in under a second a part; LFM2.5-8B-A1B
is too slow and copies segments; gpt-oss-120b rewrites names: see
"Measured".

## How

- **Where**: on the final segments, after the echo filter and the
  speakers, before `final_segments` leaves. The live transcript is not
  touched (latency), and a final segment is never sent twice, so the clean
  text must be the one sent.
- **Not on the finalizer thread**: a request takes seconds to tens of
  seconds, and the finalizer is shared by every session's pass. A pool of
  `--polish-parallel` threads asks the model; a session has one part out
  at a time, so its segments leave in order and each part is sent with the
  last three segments already cleaned as context (names, the sentence
  across the cut).
- **A part is about a minute** (what `Progressive::ready` gives), split
  at 3000 characters; an upload's whole result is split the same way.
- **The checks** (`accept`): a text that grew past 1.25× (+15 characters),
  or fell under 0.4× past six words, or has under 75% of its words heard in
  its own segment (a word within a third of its letters counts, so
  "Anno" → "Hanno" passes) keeps the heard text. An empty answer drops a
  segment of three words or fewer ("Ehm."), not a longer one. An answer
  that cannot be read, or a failed request, leaves the part as heard, and
  `warnings` says how many parts.
- **The text goes to the endpoint**: a hosted provider reads every meeting.
  A model on the server's own machine or network keeps the README's
  promise; the flag's help says so. The choice is per meeting, made by
  the person through the client, and kept with a pending meeting so that
  its upload honours it too. Off by default: the server's operator picks
  the model, but whether a meeting's text leaves for it is the person's
  to say, and a hosted model must not read a meeting nobody agreed to
  send. (Rejected: on by default with `--no-polish`, which sent every
  meeting on a server with a key to Groq unless someone remembered the
  flag.)

## Measured (2026-10-25, M3 16 GB, Unsloth Studio)

`unsloth/LFM2.5-8B-A1B-GGUF` (UD-Q6_K_XL), the Italian committee
recording (98 s, four speakers), `play --final --speed 4`, against a server
without clean-up:

- **18–49 s per part** of 1–7 segments: the model reasons first (5 000
  characters of reasoning for 5 segments) and Unsloth Studio's API neither
  takes `chat_template_kwargs.enable_thinking=false` (500) nor honours
  `reasoning_effort`. The page lags the meeting by that much more.
- **It copied a segment's text into the next one** twice in one run
  ("Sì, li chiamo oggi pomeriggio." in place of "Io preparo la bozza del
  contratto…"): lengths alike, so only the heard-words check, added for
  it, catches it. With the check the result was identical to the raw one.
- **It cleaned almost nothing**: called directly on segments with
  planted fillers, "cioè" and "no anzi" stayed; "il buggito range" (budget
  regge) was not fixed.

Qwen3.8-27B and gpt-oss-120b on Groq, the same recording and part:

- **`qwen/qwen3.8-27b`: 0.3–0.7 s per part**, no reasoning, nothing
  refused by the checks. On the meeting: punctuation between the prices,
  "faro en la" → "Faro è la", "buggito" → "budget", "l'unanimità" →
  "all'unanimità"; it left "Anno" (Hanno) and "range" (regge). On the
  planted fillers: "ehm", "cioè" gone, "all'unanimità no anzi a
  maggioranza" → "a maggioranza", "algo vigilanza" → "Argo vigilanza".
- **`openai/gpt-oss-120b`** (`reasoning_effort` low, 1.8 s): "Allora
  proponiamo algo vigilanza" became "Proponiamo un servizio di
  vigilanza": a name lost, which the heard-words check passes (most
  words were heard). Not to be used.

## Rejected

- **Cleaning in the CLI**: the person's own key and model, but an upload's
  pass and the future local mode would each need it again, and the CLI
  appends segments to the page as they come.
- **Rewriting `words` too**: they are what the echo filter and the times
  rest on; the text alone is what a reader reads.
- **One request per segment**: no context for names and cut sentences, and
  ten times the requests.

## Still to do

- A real meeting with real fillers: the recordings tried are synthetic.
- Where the model runs: Groq sends every meeting's text to a hosted
  provider, against the README's promise; Qwen3.8-27B on our own GPU is
  the same model without that.
- Its cost at scale: one request a minute per meeting, ~1 000 tokens in,
  ~500 out; tens of meetings need a server that batches concurrent
  requests (vLLM, llama.cpp with `--parallel`), and the model mirrored in
  `models` before it runs anywhere but a laptop.
