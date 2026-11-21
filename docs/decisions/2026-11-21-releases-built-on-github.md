# Releases are built on GitHub's runners

A release is a tag: `scripts/release.sh X.Y.Z` sets the version, commits,
tags `vX.Y.Z` and pushes, and `.github/workflows/release.yml` builds the
macOS `.dmg` on a macOS runner and the Windows installer on a Windows one,
then publishes the release with `latest.json`. `scripts/release.sh
--local` keeps the Mac-only way (both built here, a draft published by
hand).

## Why

- The repository is public, so GitHub's runners cost nothing, macOS's
  included; a release no longer holds the Mac for twenty minutes.
- Windows is built by Windows' own tools instead of cargo-xwin and
  makensis on a Mac.

## How

- **Draft, build, publish**: a first job checks that the tag is
  `tauri.conf.json`'s version and makes a draft release; each platform's
  job uploads its own files under fixed names; the last job writes
  `latest.json` from both signatures and publishes. Two jobs merging one
  `latest.json` (tauri-action's `includeUpdaterJson`) can each overwrite
  the other's platform, so neither writes it. A release is published whole
  or not at all: a failed platform leaves a draft no app is offered.
- **Published directly**, unlike `--local`'s draft: the tag is the decision
  to release, and the CI workflow has checked `main` before it.
- **The macOS certificate** is the self-signed one of `scripts/app.sh
  cert`, from the `MACOS_CERTIFICATE` secret, imported into a keychain of
  the run's own and trusted for code signing in the runner's System
  keychain (`sudo security add-trusted-cert`, no prompt on a runner): an
  untrusted identity is not valid for `codesign`. A new certificate would
  have macOS ask every user for the microphone and the call's audio again.
  It was exported from Keychain Access by hand: `security export` exports
  every identity of the keychain at once, and the login keychain holds
  others whose keys have no business in a CI secret.
- **The updater's key** is the `TAURI_SIGNING_PRIVATE_KEY` secret. Secrets
  are not given to workflows run for pull requests from forks, and this
  one runs only for tags.
- **A tag with a release already** (made with `--local`) is left alone.
