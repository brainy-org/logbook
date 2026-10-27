# The desktop app: Tauri on the CLI's client, signed but not notarized

Verdict: logbook's desktop app (`apps/desktop`) is a Tauri 2 app, as
galley-client's, built on a library that the CLI now shares
(`crates/client`: what was the CLI's audio, session, pending folder,
brainy and voices, and the meeting's run from its first sample to its
page). It is shaped after Granola: notes written during the meeting, a
report written from them and the transcript at the end, and the meetings
listed and kept. It is signed with a self-signed certificate made once per
Mac, not notarized. The report is written by logbook-server's model when
the meeting goes through a server, by the model in the app's settings
otherwise.

## Why

- **One client, two front ends.** The meeting's run (pending folder,
  final segments to brainy, `finish`) was in the CLI's `main.rs`; an app
  driving its own copy would drift from it the first time either changed.
  `crates/client` holds it (`meeting.rs`) and both only drive it, as
  `crates/engine` is driven by the server and the CLI. Its messages for
  the person go through `tracing`: the CLI prints them as lines, the app
  shows them.
- **Signed, not notarized.** Notarization needs an Apple Developer ID
  ($99 a year), which the project does not have (galley-client's
  `docs/decisions/2026-10-24-unsigned-builds.md`). Unsigned, or signed
  ad hoc, macOS ties the microphone's and the call's permissions to a
  signature that changes with every build, and asks again after each
  update. A self-signed certificate gives a designated requirement on the
  certificate's hash (`certificate leaf = H"…"`), the same for every
  build signed with it: the microphone's and the call's permissions
  stay (not the keychain's: "Found on the way"). `scripts/desktop.sh cert`
  makes one (ten years) in the login keychain, `build` signs with it.
  A Mac that did not build the app still blocks it at first open.
- **The report where the transcript is made.** With a server, the
  server's model writes it (`POST /reports`), as it cleans the transcript
  up: one model and one key, held by whoever runs the server; the app
  then holds no key. Without one, the app asks the model in its settings
  (its key in the app's `secrets.json`). The person chose this over the app always
  asking its own model.
- **The meetings are kept.** Granola's list of past meetings is half of
  its use: the report read again, a speaker named after the meeting. The
  app keeps each published meeting's text (`history.rs`,
  `<data>/meetings/<id>.json`: transcript, notes, report), never its
  audio, which goes as before (`pending.rs`). The CLI keeps nothing
  there (`Saved::keep`).
- **A speaker named after the meeting is named everywhere.** Renaming
  "Speaker 2" to "Luca" saves the voice (voices.rs, as `logbook
  speakers name`), changes the label in the kept meeting and in the
  voices kept to name, and rewrites the page's turn headings and its
  header (`Brainy::rename_speaker`). The report is not rewritten on its
  own: the app offers to write it again.

## Found on the way

- **The hardened runtime refuses libparakeet** ("mapping process and
  mapped file (non-platform) have different Team IDs"): it is loaded at
  run time from `~/.cache/brainy-org`, signed by no one the app knows.
  The app has `com.apple.security.cs.disable-library-validation`
  (`Entitlements.plist`); a CLI signed the same way without it fails at
  `dlopen`, with it transcribes.
- **A deployment target of 14 or more breaks the build** on macOS 27
  with rustc 1.92 and ld-27037: proc-macro dylibs come out with a
  "mis-aligned LINKEDIT string pool" and rustc cannot load them (E0463 on
  serde_derive, zerofrom_derive…). Tauri exports
  `MACOSX_DEPLOYMENT_TARGET` from `bundle.macOS.minimumSystemVersion`,
  so it is 11.0 (rustc's own default on Apple silicon, the CLI's), not
  the 14.6 the call's audio needs; that check is at run time (cpal). A
  release folder built with the bad target keeps its broken dylibs:
  `cargo clean --release`.

- **The keychain asks again after every build**, even signed with the
  certificate and answered "Always": the item's ACL keeps the designated
  requirement (`certificate leaf = H"…"`, stable), but its partition list
  (macOS 10.12 and later) names an app without an Apple Team ID by its
  cdhash, and each build adds a new one (`security dump-keychain -a`
  showed five on the `logbook` item after five builds). The app keeps its
  secrets in `secrets.json` in its config folder, mode 600, as the CLI
  reads its keys from files and the environment; a Developer ID (a Team
  ID, `teamid:` in the partition list) would let them go back.

## Rejected

- **Notarizing now**: the Developer ID's cost, before the app reaches
  anyone who did not build it.
- **The app always asking its own model**, even with a server: two
  places holding a key for one meeting; chosen against by the person.
- **The report in a separate page**: the page is the meeting; the report
  goes on top of it, under the header line.
