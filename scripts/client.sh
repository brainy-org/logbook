#!/usr/bin/env bash
# Runs the logbook CLI from this checkout, built in release; arguments go to
# it (`scripts/client.sh live --system`). LOGBOOK_SERVER picks another server
# (default ws://127.0.0.1:3300/live). Stays in the current folder, so that a
# relative path given to `play` or `bench` is the caller's.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --release -q -p logbook --manifest-path "$root/Cargo.toml"
exec "$root/target/release/logbook" "$@"
