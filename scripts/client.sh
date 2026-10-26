#!/usr/bin/env bash
# Runs the logbook CLI from this checkout, built in release; arguments go to
# it (`scripts/client.sh live --system`). Without LOGBOOK_SERVER the meeting
# is transcribed in the CLI itself. Stays in the current folder, so that a
# relative path given to `play` or `bench` is the caller's.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --release -q -p logbook --manifest-path "$root/Cargo.toml"
exec "$root/target/release/logbook" "$@"
