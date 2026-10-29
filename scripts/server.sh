#!/usr/bin/env bash
# Builds and starts logbook-server from this checkout; arguments go to the
# server (`scripts/server.sh --latency very-low`). The server itself
# fetches libparakeet and the models from the mirror on first start
# (crates/engine/src/mirror.rs), public packages, no token. With a Groq key in
# ~/.config/logbook/groq.key the report of a meeting started with --report
# is written there.
set -euo pipefail

cd "$(dirname "$0")/.."

# .env (not committed): any setting of the server; the environment wins
# over it.
if [[ -f .env ]]; then
  while IFS='=' read -r name value || [[ -n $name ]]; do
    if [[ $name =~ ^[A-Z_][A-Z0-9_]*$ && -z "${!name:-}" ]]; then
      export "$name=$value"
    fi
  done < .env
fi

# The language model of the reports (docs/pipeline.md, "Report"): for now
# Qwen3.8-27B on Groq, when a key is in LLM_KEY_FILE (or LOGBOOK_LLM_KEY is
# set); LOGBOOK_LLM_URL and LOGBOOK_LLM_MODEL pick another. Groq reads the
# text of every meeting started with --report: a test setup, not the
# promise of the README.
key_file="${LLM_KEY_FILE:-$HOME/.config/logbook/groq.key}"
if [[ -z "${LOGBOOK_LLM_KEY:-}" && -s "$key_file" ]]; then
  LOGBOOK_LLM_KEY="$(<"$key_file")"
  export LOGBOOK_LLM_KEY
fi
if [[ -n "${LOGBOOK_LLM_KEY:-}" ]]; then
  export LOGBOOK_LLM_URL="${LOGBOOK_LLM_URL:-https://api.groq.com/openai/v1}"
  export LOGBOOK_LLM_MODEL="${LOGBOOK_LLM_MODEL:-qwen/qwen3.8-27b}"
fi
cargo build --release -q -p logbook-server
exec target/release/logbook-server "$@"
