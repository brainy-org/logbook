#!/usr/bin/env bash
# Builds and starts logbook-server from this checkout; arguments go to the
# server (`scripts/server.sh --latency very-low`). The server itself
# fetches libparakeet and the models from the mirror on first start
# (crates/engine/src/mirror.rs), with LOGBOOK_REGISTRY_TOKEN from .env. With a Groq key in
# ~/.config/logbook/groq.key the final transcript is cleaned up there.
set -euo pipefail

cd "$(dirname "$0")/.."

# .env (not committed): LOGBOOK_REGISTRY_TOKEN and any other setting of the
# server; the environment wins over it.
if [[ -f .env ]]; then
  while IFS='=' read -r name value || [[ -n $name ]]; do
    if [[ $name =~ ^[A-Z_][A-Z0-9_]*$ && -z "${!name:-}" ]]; then
      export "$name=$value"
    fi
  done < .env
fi

# The clean-up of the final transcript (docs/pipeline.md, "Final pass"):
# for now Qwen3.8-27B on Groq, when a key is in POLISH_KEY_FILE (or
# LOGBOOK_POLISH_KEY is set); LOGBOOK_POLISH_URL and LOGBOOK_POLISH_MODEL
# pick another. Groq reads every meeting's text: a test setup, not the
# promise of the README.
key_file="${POLISH_KEY_FILE:-$HOME/.config/logbook/groq.key}"
if [[ -z "${LOGBOOK_POLISH_KEY:-}" && -s "$key_file" ]]; then
  LOGBOOK_POLISH_KEY="$(<"$key_file")"
  export LOGBOOK_POLISH_KEY
fi
if [[ -n "${LOGBOOK_POLISH_KEY:-}" ]]; then
  export LOGBOOK_POLISH_URL="${LOGBOOK_POLISH_URL:-https://api.groq.com/openai/v1}"
  export LOGBOOK_POLISH_MODEL="${LOGBOOK_POLISH_MODEL:-qwen/qwen3.8-27b}"
fi
cargo build --release -q -p logbook-server
exec target/release/logbook-server "$@"
