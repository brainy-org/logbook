# Reports by the claude or codex CLI

A meeting's report can be written by Claude Code (`claude -p`) or Codex
(`codex exec`) run as a subprocess on the machine that writes it, with the
account the CLI is signed in with, beside an OpenAI-compatible API
(`llm::Provider`: `api`, `claude`, `codex`; `crates/engine/src/llm_cli.rs`).
Whether the CLI is signed in is checked before a meeting with a report is
recorded, at the server's start and in the app's settings, and a signed-out
CLI is refused there with the command that signs it in.

## Why

Whoever has a Claude or ChatGPT subscription and no API key had no model
for the report short of a hosted provider's key (Groq's free tier in
`scripts/server.sh`). The CLIs are on most of these computers already,
signed in.

## How

- One process per request, never a process kept warm: a report is one
  request (a meeting of more than six hours, a few), so the second or so a
  process takes to start is nothing beside the model's minute. llm-api keeps
  warm processes because it answers chat turns; logbook does not.
- Nothing of the user's runs: an empty folder of its own as the working
  directory, Claude with `--tools "" --strict-mcp-config
  --disable-slash-commands --setting-sources "" --no-session-persistence`
  (no CLAUDE.md, no hooks: a hook of the user's would run on every report),
  Codex with `--sandbox read-only --ephemeral --skip-git-repo-check`.
- The prompt goes on stdin and Claude's instructions in a file
  (`--system-prompt-file`): a transcript of hours is too long for a command
  line, and Windows refuses newlines in the arguments of a `.cmd` (npm's
  shims).
- Codex has no system prompt: the instructions head the prompt in
  `<instructions>`, as llm-api does. Its answer is read from
  `--output-last-message`, not from stdout, where its progress goes too.
- Claude's `--output-format json` says `is_error` and the reason in
  `result` ("Not logged in · Please run /login") with exit 1.

## Signed in or not

Asked of the CLI, not guessed from its files: `claude auth status` and
`codex login status` exit 0 when signed in and 1 when not (checked
2026-11-21, claude 2.1.323, codex 0.171.0, with an empty
`CLAUDE_CONFIG_DIR` / `CODEX_HOME`). `claude auth status` counts
`CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY` as signed in, which is
what a server uses. Neither asks the provider, so a sign-in that expired
passes the check: a request that fails runs the check again and returns its
error when it fails, otherwise the CLI's own.

The check is refused before recording, like a report with no model at
all: a report that fails only at the meeting's end leaves the person with a
pending meeting to finish after signing in.

## Finding the CLI

A macOS app opened from the Finder has PATH `/usr/bin:/bin:/usr/sbin:/sbin`:
neither Homebrew's nor the native installer's folder. The CLI is looked for
on PATH, then in `~/.local/bin`, `~/.claude/local`, npm's, bun's and volta's
folders, `/opt/homebrew/bin` and `/usr/local/bin` (`%APPDATA%\npm` on
Windows), or where `LOGBOOK_CLAUDE_BIN` / `LOGBOOK_CODEX_BIN` say; those
folders are put on the child's PATH too, since Codex from npm is a node
script that finds `node` there.

## Not chosen

- Going through llm-api (`--llm-url http://localhost:8080/v1`): that
  already works and needs nothing here, but it is one more service to start
  and keep running on a laptop; the subprocess needs only the CLI.
- No check, only the request's error: the person learns it at the end of
  a meeting of an hour.
