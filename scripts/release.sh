#!/usr/bin/env bash
# A release of the desktop app on GitHub, from this Mac:
#
#   scripts/release.sh 0.2.0
#
# Sets the version (tauri.conf.json, the workspace's Cargo.toml and
# Cargo.lock), builds the macOS .dmg (Apple Silicon, signed with the
# certificate of `scripts/app.sh cert`) and the Windows installer (x64,
# cross-built, not signed), then commits "Logbook 0.2.0", tags v0.2.0,
# pushes both and makes a draft release with the two installers attached,
# and what the installed apps update from: `latest.json`, the macOS bundle
# as a .tar.gz and the Windows installer, signed with the updater's key
# (~/.config/logbook/updater.key, LOGBOOK_UPDATER_KEY; lost, the apps out
# there can never be updated again: keep a copy). The draft is published by
# hand once looked at, and only then do the apps offer it:
#   gh release edit v0.2.0 --draft=false
#
# The macOS build must be signed with the same certificate every time:
# macOS ties the microphone's and the call's permissions to it, and a
# release signed with another asks every user for them again
# (docs/decisions/2026-10-27-desktop-app.md). Neither installer is
# notarized or signed by a known authority: macOS blocks the app at first
# open and Windows' SmartScreen warns; the release notes say what to do.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="${1:-}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: scripts/release.sh X.Y.Z" >&2
  exit 1
fi
tag="v$version"

# Released from main as it is on GitHub, with nothing uncommitted.
if [[ "$(git branch --show-current)" != main ]]; then
  echo "not on main" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "uncommitted changes: commit or stash them first" >&2
  exit 1
fi
git fetch -q origin main --tags
if [[ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]]; then
  echo "main is not origin/main: pull or push first" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "$tag exists already" >&2
  exit 1
fi
gh auth status >/dev/null
updater_key="${LOGBOOK_UPDATER_KEY:-$HOME/.config/logbook/updater.key}"
if [[ ! -s "$updater_key" ]]; then
  echo "no updater key at $updater_key: the apps could not update to this release" >&2
  exit 1
fi

# The version, in the two places that hold it, and Cargo.lock.
current=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' apps/logbook/src-tauri/tauri.conf.json)
echo "Logbook $current → $version"
sed -i.bak "s/^  \"version\": \"$current\",\$/  \"version\": \"$version\",/" \
  apps/logbook/src-tauri/tauri.conf.json
sed -i.bak "s/^version = \"$current\"\$/version = \"$version\"/" Cargo.toml
rm -f apps/logbook/src-tauri/tauri.conf.json.bak Cargo.toml.bak
cargo update -q --workspace
undo() {
  echo "failed: the version is back to $current (git checkout)" >&2
  git checkout -q -- apps/logbook/src-tauri/tauri.conf.json Cargo.toml Cargo.lock
}
trap undo ERR

scripts/app.sh build
scripts/app.sh windows
dmg="target/release/bundle/dmg/Logbook_${version}_aarch64.dmg"
exe="target/x86_64-pc-windows-msvc/release/bundle/nsis/Logbook_${version}_x64-setup.exe"
ls "$dmg" "$exe" "$exe.sig" target/release/bundle/macos/Logbook.app.tar.gz{,.sig} >/dev/null

# What the apps update from: the macOS bundle under a name of its own, and
# latest.json naming each platform's file with its signature.
assets=$(mktemp -d)
app_tar="Logbook_${version}_aarch64.app.tar.gz"
cp target/release/bundle/macos/Logbook.app.tar.gz "$assets/$app_tar"
download="https://github.com/brainy-org/logbook/releases/download/$tag"
jq -n --arg version "$version" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg notes "See https://github.com/brainy-org/logbook/releases/tag/$tag" \
  --arg mac_url "$download/$app_tar" \
  --arg mac_sig "$(<target/release/bundle/macos/Logbook.app.tar.gz.sig)" \
  --arg win_url "$download/$(basename "$exe")" --arg win_sig "$(<"$exe.sig")" \
  '{version: $version, notes: $notes, pub_date: $date, platforms: {
     "darwin-aarch64": {url: $mac_url, signature: $mac_sig},
     "windows-x86_64": {url: $win_url, signature: $win_sig}}}' >"$assets/latest.json"

git commit -q -m "Logbook $version" -- apps/logbook/src-tauri/tauri.conf.json Cargo.toml Cargo.lock
trap - ERR
git tag -a "$tag" -m "Logbook $version"
git push -q origin main "$tag"

notes=$(
  cat <<'EOF'
## Install

**macOS** (Apple Silicon): open the `.dmg` and drag Logbook to Applications. The app is not notarized, so macOS blocks it the first time: System Settings → Privacy & Security → Open Anyway (or `xattr -dr com.apple.quarantine /Applications/Logbook.app` in a terminal).

**Windows** (x64): run the `-setup.exe`. It is not signed, so SmartScreen warns: More info → Run anyway. Transcribing on the computer itself needs the [Visual C++ Redistributable x64](https://aka.ms/vs/17/release/vc_redist.x64.exe); with a logbook-server it does not.

The speech models (about 1 GB) download at the first meeting transcribed on the computer. Once installed, Logbook offers each new version itself.
EOF
)
gh release create "$tag" "$dmg" "$exe" "$assets/$app_tar" "$assets/latest.json" \
  --draft --title "Logbook $version" \
  --notes "$notes" --generate-notes
rm -rf "$assets"
echo "draft release $tag made; publish it with: gh release edit $tag --draft=false"
