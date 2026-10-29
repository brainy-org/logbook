# `finish --wait` tries again until the server answers

**Verdict.** `logbook finish --wait` finishes every pending meeting,
oldest first, and tries again those that could not reach the server or
brainy, every 5 s doubling up to 5 minutes (back to 5 s once a meeting
finishes), forever unless interrupted. Only failures that waiting can
fix are tried again; the others are reported and their meeting left
pending for the run. Each pending folder now has a `.lock` held by the
process that records or finishes it, and `finish` (and the app) leave a
held meeting alone. The loop is the client library's
(`crates/client/src/finishing.rs`); the CLI only sets `wait`.

## What is tried again

Something on the way did not answer, or answered "not now":

- a connection refused, reset or timed out (reqwest's connect, timeout,
  request and body errors; the matching `io::ErrorKind`s);
- an HTTP 5xx but 501, 408, 425 or 429, from the server or brainy
  (`HttpStatus`, carried in the error chain instead of a formatted
  string, so the status can be read back);
- the server answering `404` for a pass it had just started (it
  restarted: `Transient`).

Everything else is permanent for the run: brainy refusing the page with
a 4xx (a parent page deleted, a token without write access), a server
token refused (401), a final pass that failed, brainy's address or token
missing, a pending folder that cannot be read. Tried again every few
seconds, each would fail the same way and hide the failures that can
pass.

## Why a lock

Before, `finish` listed every folder under `pending/`, including the one
a running `logbook live` (or the app's recorder, or a meeting of a
series that is still recording) was writing to: it would upload the
audio so far, publish a partial transcript and delete the folder under
the recorder. With `finish --wait` left running beside the recorder this
becomes the common case, not a mistake. Two `finish` processes (or the
CLI and the app) could also finish one meeting twice and write its page
twice. The lock is an OS lock on `.lock` (`File::try_lock`), released
when its process dies, so a crashed recorder leaves its meeting
finishable. A held meeting is "busy": with `--wait` it is looked at
again each round, since its holder leaves it pending if it fails.

## Costs

- A round in which the server is down takes about a minute per meeting
  that needs it: `upload` keeps its own tries (2 s doubling, six of
  them) before the round's backoff. A meeting that only waits for
  brainy (its `final.json` is there) does not touch the server.
- `--wait` with only busy meetings keeps polling a lock every 5 minutes
  at most: nothing on the network.
- An older client does not take the lock; mixed versions on one data
  folder are as unsafe as before.

## Rejected

| What | Why not |
|---|---|
| Retry every error | a 4xx from brainy fails the same way forever, every 5 minutes, and buries the meetings that can be finished |
| A health check before each round | one more request that can pass while the upload fails; the upload's own failure says the same |
| Exit after the first round with failures, for cron to call again | the backoff and the "given up" set would be lost between runs, and a cron job is one more thing to set up |
