# The server lets in a token per person, made on its admin page

Verdict: every route of a meeting on logbook-server (`/live`, `/finals`,
`/reports`) needs `Authorization: Bearer <token>`, a token made for one
person on the server's admin page (its address, `/`) and revocable
there. The
admin page is guarded by one admin token, given to the server or made
at its first start. The server keeps the tokens' SHA-256 only, in a file
in its data folder. `--no-auth` opens it, for a server only its own
computer reaches.

## Why

- **The server was open to whoever reached it**: anyone on its network
  could use its GPU, and with the id of a final pass read a meeting's
  transcript. A server shared by a team, or reachable from outside,
  needs to know who is using it, and to stop one person without
  stopping everyone.
- **A token per person, not one for everyone**: one shared token cannot
  be taken back from one person who leaves, and the logs could not say
  whose session is whose.
- **The server's own tokens, not brainy's**: the client holds a brainy
  token, and the server could ask brainy who it belongs to. It would then
  receive brainy credentials, which it must never hold (CLAUDE.md), and
  brainy would have to be up for anyone to transcribe.
- **SHA-256, not a password hash**: the tokens are 256 random bits, not
  words someone chose; a slow hash protects guessable secrets, and these
  are not. A copy of `tokens.json` lets no one in.
- **An admin token, not accounts**: the people who manage the server are
  one or two; a page that asks for one token, kept in the tab's session
  only, is all it needs. Made at first start and written to a file only
  the server's user reads, so a server is never started with an admin
  page anyone can open.
- **A page served by the server itself**, one HTML file in the binary
  with no outside resources and a strict Content-Security-Policy, names
  set as text: nothing to deploy beside the server. In brainy's look
  (its teal, greys and dark theme, Geist served by the server), as the
  desktop app: the same family of tools.
- **The admin token's place is said, not shown**: the server logs the
  file's path at every start, and the sign-in page lists the usual
  ones; the token itself is never printed.

## Rejected

- **Auth off by default, on with a flag**: a server started without
  thinking about it would be open. `--no-auth` has to be asked for.
- **Tokens in the client's URL** (`/live?token=`): URLs end up in proxy
  logs. The WebSocket client sends the header on its upgrade request.
- **Final passes tied to the token that started them**: their ids are
  128 random bits, known only to the client that asked; worth doing when
  the server keeps results longer than an hour.

## Not yet

TLS in the server: for now a proxy in front of it serves HTTPS. Limits
per person (sessions at once, hours a month).
