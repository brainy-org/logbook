# The desktop app

`apps/desktop`: Tauri 2, the pages in React (`src/`, brainy's look,
copied from galley-client's app), the Rust side in `src-tauri/`. Why it
is built so: `docs/decisions/2026-10-27-desktop-app.md`.

| File | What it does |
|---|---|
| `src-tauri/src/lib.rs` | the commands the pages call; the data folder (the CLI's) |
| `src-tauri/src/recorder.rs` | the meeting being recorded, one at a time: `meeting::meeting` run with the settings' channels, events to the pages (`meeting-state`, `utterance`, `levels`, `meeting-done`, `meeting-failed`) |
| `src-tauri/src/settings.rs` | `settings.json` in the app's config folder; the server's and brainy's tokens and the model's key in `secrets.json` beside it (mode 600); the backend, one `Local` kept while its model settings stay the same (the models load once) |
| `src-tauri/src/logs.rs` | crates/client's `tracing` lines to the pages (`log`) |
| `src/App.tsx` | the frame; the recorder's state lives here, across views |
| `src/components/Recorder.tsx` | notes, live transcript, the meter and stop |
| `src/components/MeetingPage.tsx` | a kept meeting: report, notes, transcript (a phrase after 30 s of silence shows its time, any on hover), renaming speakers |
| `src/components/SettingsPage.tsx` | the settings, the voices known by name |

## Traps

- **What a meeting does is decided in crates/client** (`meeting.rs`):
  the app only chooses its options (`Settings::publish`, `sources`). A
  step added in `recorder.rs` is a step `logbook finish` skips for the
  same meeting left pending.
- **Secrets never go to the pages.** `get_settings` says whether the
  token and the key are set; `save_settings` takes new ones and writes
  them to `secrets.json` (mode 600, beside `settings.json`). A command that returns one puts it within reach
  of anything rendered in the webview.
- **Secrets are not in the keychain.** Without an Apple Team ID a
  keychain item's partition list names the reader by its cdhash, which
  every build changes: macOS asked again after each build, "Always" or
  not. `Secret` keeps them in `secrets.json` and moves there, once, any
  the keychain still holds from an earlier version. Back to the keychain
  only with a Developer ID (decision record, "Found on the way").
- **The certificate keeps the privacy permissions** (microphone, the
  call's audio): macOS ties them to the designated requirement, the same
  for every build signed with it. `scripts/desktop.sh dev` signs the
  debug binary with it too (cargo's runner, `signed-run`).
- **The report and the notes are rendered as React elements**
  (`Markdown.tsx`, react-markdown without raw HTML). A model's output in
  `dangerouslySetInnerHTML` is markup in a window that can call the
  app's commands.
- **The bundle needs its plists**: `Info.plist` (merged by Tauri) has the
  microphone's and the call's usage strings, without which macOS refuses
  them silently; `Entitlements.plist` the microphone under the hardened
  runtime and `disable-library-validation` for libparakeet.
- **Recording starts at the click, the models loaded or not.** They are
  loaded at the app's start and after the settings are saved
  (`App::preload`), in the background; a meeting started before they are
  ready records at once, its audio waiting in the channel to the session
  (about 64 KB a second) until they are (`State::ready`). Loading them
  before opening the microphone, as the CLI does, loses what is said
  meanwhile.
- **`minimumSystemVersion` stays at 11.0**: 14 or more breaks the
  build's proc macros on macOS 27 (the decision record, "Found on the
  way").

## Not yet

- Run on a real meeting through the app (the CLI's flow is the same, and
  was run; the app's pages were only built, not clicked through).
- The rename on a real brainy page: `rename_speaker` matches the turn
  headings as `list_blocks` returns them (`**Speaker 2** · 00:04`), read
  from brainy's code, not seen.
- Windows and Linux: built for macOS only (`targets`: app, dmg).
- A meeting started from the calendar, or when a call starts.
