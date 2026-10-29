# The desktop app

`apps/desktop`: Tauri 2, the pages in React (`src/`, brainy's look,
copied from galley-client's app), the Rust side in `src-tauri/`. Why it
is built so: `docs/decisions/2026-10-27-desktop-app.md`.

| File | What it does |
|---|---|
| `src-tauri/src/lib.rs` | the commands the pages call; the data folder (the CLI's); how the local models get ready (`models`: fetched with their bytes, all files together, loaded, ready, failed; `models_state`, `load_models` to try again) |
| `src-tauri/src/recorder.rs` | the recording, one at a time: `series::meetings` run with the settings' channels and the default limits (a meeting closes after 15 minutes without a phrase, or at 8 hours), events to the pages (`meeting-state`, whose `part` counts the meetings of the recording; `utterance`, on the recording's clock; `levels`; `meetings-changed` or `meeting-failed` for a meeting that ends while the next records; `meeting-done` or `meeting-failed` at the end) |
| `src-tauri/src/settings.rs` | `settings.json` in the app's config folder (the meeting's `Mode`, chosen on the home page and saved by `start_meeting`, decides the channels: the call, the microphone diarized); the server's and brainy's tokens and the model's key in `secrets.json` beside it (mode 600); the backend, one `Local` kept while its model settings stay the same (the models load once) |
| `src-tauri/src/logs.rs` | crates/client's `tracing` lines to the pages (`log`) |
| `src/App.tsx` | the frame; the recorder's state lives here, across views (the live transcript's last 300 phrases: a meeting of hours has tens of thousands) |
| `src/components/Home.tsx` | before a meeting: its title and mode (Call, In the room, Room + call) with what each records, what will happen to it, the pending meetings |
| `src/components/Recorder.tsx` | notes, live transcript, the meter and stop; drawn again (`key`: the state's `part`) when the recording's next meeting starts, its notes empty |
| `src/components/ModelsProgress.tsx` | the models' download and loading as a bar, on the home page and in the live transcript until they are ready |
| `src/components/MeetingPage.tsx` | a kept meeting: report, notes, transcript (a phrase after 30 s of silence shows its time, any on hover), renaming speakers |
| `src/components/SettingsPage.tsx` | the settings (the language model: a provider, its address kept and the provider read back from it, then one of the models it lists at `/models`: `list_models`, engine `polish::models`), the voices known by name |

## Traps

- **What a meeting does is decided in crates/client** (`meeting.rs`):
  the app only chooses its options (`Settings::publish`, `sources`). A
  step added in `recorder.rs` is a step `logbook finish` skips for the
  same meeting left pending.
- **Pending meetings are finished through `Pending::claim`**
  (`finish_pending`): a meeting the recorder (or a `logbook finish`)
  holds is left to it. `Pending::open` only reads, for the list.
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
  meanwhile. On a first start they are fetched first (about 1 GB): the
  mirror reports the bytes of all the missing files together, counted
  against the sizes pinned beside their sha256 (`Mirror::with_progress`,
  one report a MiB), and `Local` turns them into a `Status` the pages
  show as one bar. **The pages name no model and no file**: the person
  sees the transcription getting ready, not what it is made of.
- **`minimumSystemVersion` stays at 11.0**: 14 or more breaks the
  build's proc macros on macOS 27 (the decision record, "Found on the
  way").

- **Windows is cross-built from a Mac** (`scripts/desktop.sh windows`:
  cargo-xwin, LLVM, makensis), into an NSIS installer, not signed:
  SmartScreen warns at first open (More info → Run anyway). libparakeet
  comes from the mirror as on macOS (`win-vulkan-x64` when System32 has
  the GPU driver's `vulkan-1.dll`, `win-cpu-x64` otherwise) and needs the
  Visual C++ Redistributable x64, which the installer does not bring: the
  error opening it says so.
- **On Windows the app ends with `parakeet::exit`** (`RunEvent::Exit`,
  `lib.rs`), never `process::exit`: that runs libparakeet's static
  teardown, which after a meeting crashes in the GPU driver (0xc0000409)
  (`docs/decisions/2026-10-27-windows-exit-without-teardown.md`).

## Not yet

- A recording cut into meetings shows one recorder: the clock counts the
  whole recording, the live transcript runs on across meetings, and
  nothing on the page says a new one started but the notes emptying
  (the meeting that closed keeps its own: the app sends notes with the
  meeting they were typed in, `set_notes(part, notes)`, and the recorder
  sends what it had not sent yet as it is drawn again). A meeting that
  ends meanwhile only refreshes the list.
  The limits are the defaults, not settings.

- Run on a real meeting through the app (the CLI's flow is the same, and
  was run; the app's pages were only built, not clicked through).
- The rename on a real brainy page: `rename_speaker` matches the turn
  headings as `list_blocks` returns them (`**Speaker 2** · 00:04`), read
  from brainy's code, not seen.
- Windows: the installer is built (`scripts/desktop.sh windows`), never
  run; the window's frame (`titleBarStyle` is macOS's) is untried. The
  app and the CLI, built on Windows and run over Remote Desktop (`pnpm
  tauri dev`), record, transcribe on the Vulkan build and finish a
  meeting, the microphone once its buffer was raised
  (`docs/decisions/2026-10-27-windows-audio-buffer.md`). Building there
  needs Visual Studio's C++ Build Tools and CMake (Opus).
- Linux: not built.
- A meeting started from the calendar, or when a call starts.
