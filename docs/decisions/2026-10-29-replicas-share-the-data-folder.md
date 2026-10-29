# Replicas of the server share the data folder, a file per record

Verdict: logbook-server runs alone, as before, or as several replicas
behind one address (Kubernetes) that mount one data folder (`--data`, a
volume they all write). What a request may need from another replica
lives there, a file per record replaced whole (engine `store.rs`): a
person's token (`tokens/<hash>.json`, its last use in `<hash>.used`) and
a final pass's state (`finals/<id>.json`). No lock, no database, no
session affinity. A meeting's audio stays on the replica that records it
(`--recordings`, a folder of its own).

## Why

- **Replicas without affinity**: a client that uploads a meeting polls
  `GET /finals/{id}` every few seconds, and after a lost connection opens
  a new session; a load balancer may send each request to another
  replica. The tokens were in one file each server read only at start
  (a token made on one replica was refused by the others, and the last
  to write erased what the others had made), the passes in memory (any
  other replica answered `404`, and the client uploaded the whole
  meeting again).
- **A file per record, not one file for all**: two servers writing one
  file each read, change and write it back, and one loses the other's
  change unless they hold a lock. Locks on a network volume (NFS, EFS)
  are not to be trusted. With a file per record, two admins making a
  token at once make two files; a revocation is a deletion.
- **Replaced whole**: written to a temporary file of its own name, then
  renamed over the old one. A reader on any replica sees one or the
  other, never part of one. Read again when a read fails for another
  reason than absence (NFS answers `ESTALE` once for a file replaced on
  another computer while being read).
- **The last use beside the token, not in it**: a server noting a use
  as another revokes the token would write the token back. Written
  beside it, the use outlives the token at worst; nothing reads it then.
- **The id of a token is its hash**: a revocation deletes one named
  file, no listing of the folder (which an NFS client may show a few
  seconds late). The hash of 256 random bits gives nothing of the token.
- **The admin token is made once** by servers starting together:
  written to a temporary file then hard-linked to `admin.token`, which
  fails when the name is taken; the others read the one made.
- **A pass runs where its audio is**; only its state is shared. Its
  owner notes every 10 s, in `servers/<instance>`, that it is alive. A
  running pass whose owner has not for 60 s is unknown (`404`): the
  owner stopped, and the client uploads the meeting again, as after a
  restart. A replica stopped cleanly deletes its note, and its passes
  are unknown at once. Results expire an hour after they are ready, as
  before; any replica deletes those past it.
- **`503`, not `401` or `404`, when the folder cannot be read**: a `401`
  tells the person their token is refused, a `404` makes the client
  upload again. A `503` is retried.
- **A new name is never cached as missing**: an NFS client keeps for a
  while that a name it looked up was not there. The ids of passes and
  the hashes of tokens are new random names nobody looked up before, so
  a pass started on one replica is found on the next one at once.

## Cost at scale

Every request of a meeting reads its token's file (off the async
threads, `spawn_blocking`), and a poll reads its pass's file: tens of
small reads a second for tens of meetings. Written: a token's use once a
minute per replica, a pass at its start and end, each replica's note
every 10 s, and a sweep of the folder every 5 minutes. Nothing per
frame or per sample. A revocation is seen at once by every replica,
having no cache.

## What it costs

- **A result is on disk for an hour** (mode 600), where it was in
  memory: the transcript and the speakers' voices (`SpeakerVoice`),
  kept no longer than before but on the volume. It outlives a restart,
  which spares the client an upload. Audio is still only in the
  replica's `Recording`.
- **A replica stalled for 60 s** (a frozen disk) has its passes taken
  for stopped: their clients upload again, and the stalled pass's result
  is written for no one. Twice the work, never a wrong transcript.
- **The clocks of the replicas** must agree within seconds (NTP, as on
  any cluster): a note is the writer's time, read with the reader's.

## Rejected

- **SQLite on the volume**: its locking is not reliable over NFS, and
  its WAL needs memory shared by one host.
- **Session affinity** (sticky routing by token or id): a replica that
  stops still loses its passes, the load balancer must support it, and
  the tokens are still a file each server reads at start.
- **Postgres or Redis**: a service to run beside a server that runs
  alone today, and two stores (one for a single server).
- **Each replica forwarding to the pass's owner** (its address in the
  id): the replicas must reach one another, and the tokens are not
  solved.
