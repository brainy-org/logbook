# The app

`apps/logbook`, one app for the desktop (macOS, Windows) and Android: Tauri 2, the pages in React (`src/`, brainy's look,
copied from galley-client's app), the Rust side in `src-tauri/`. Why it
is built so: `docs/decisions/2026-10-27-desktop-app.md`. The same app
builds for Android, as a recorder for a server ("Android" below).

| File | What it does |
|---|---|
| `src-tauri/src/lib.rs` | the commands the pages call; the data folder (the CLI's; the app's own on Android); how the local models get ready (`models`: fetched with their bytes, all files together, loaded, ready, failed; `models_state`, `load_models` to try again) |
| `src-tauri/src/recorder.rs` | the recording, one at a time: `series::meetings` run with the settings' channels and the default limits (a meeting closes after 15 minutes without a phrase, or at 8 hours), events to the pages (`meeting-state`, whose `part` counts the meetings of the recording; `utterance`, on the recording's clock; `levels`; `meetings-changed` or `meeting-failed` for a meeting that ends while the next records; `meeting-done` or `meeting-failed` at the end) |
| `src-tauri/src/settings.rs` | `settings.json` in the app's config folder (the meeting's `Mode`, chosen on the home page and saved by `start_meeting`, decides the channels: the call, the microphone diarized); the server's and brainy's tokens and the model's key in `secrets.json` beside it (mode 600); the backend, one `Local` kept while its model settings stay the same (the models load once) |
| `src-tauri/src/logs.rs` | crates/client's `tracing` lines to the pages (`log`) |
| `src-tauri/src/finisher.rs` | the meetings left pending finished while the app is open, desktop and Android alike: `finishing::finish_all` with `wait`, at the app's start, at a recording's end, from Finish now or Try again (`finish_pending`) and when the settings are saved (a token or an address changed); each meeting's state to the pages as it changes (`finishing`: its step, `meeting::Step`, and how far, or what stopped it, `finishing::Problem`, and when it is tried again), kept for `list_pending` |
| `src-tauri/src/screenshot.rs` | the desktop's screenshots for the notes: the screen under the pointer (xcap), the app's window hidden while it is taken when it is in front, or on macOS an area (`screencapture -i`, the system's ⌘⇧4 selection, Esc to give up); the buttons (`take_screenshot`) or `SCREEN` (⌘⇧1, Ctrl+Shift+1) and `AREA` (⌘⇧2), macOS's order without its keys, registered only while a meeting is recorded (`screenshot` events); the system's shutter sound for both, so a screenshot taken from another app is heard; the `shot` scheme the pages load them from |
| `src/App.tsx` | the frame; the recorder's state lives here, across views (the live transcript's last 300 phrases: a meeting of hours has tens of thousands) |
| `src/components/Home.tsx` | before a meeting: its title and mode (Call, In the room, Room + call) with what each records, who is expected among the voices known by name (none: every voice; a field that offers up to 6 matching names, only the people chosen shown, so hundreds of voices take no room; `Publish::expected`, client voices.rs); the page is centred with `justify-center-safe`, so a page taller than the window starts at its top instead of losing it, what will happen to it, the pending meetings |
| `src/components/PendingMeetings.tsx` | the pending meetings on the home page, oldest first: each one's step with a bar where it says how far (sending, the server's pass), the time it has taken otherwise; what stopped it (the server or brainy not reached, with the countdown to the next try; a token refused, with Open settings; refused or failed, with Try again) and its full error under Details |
| `src/lib/finishing.ts` | the steps' labels and how far they are, and the problems' texts, for the card and the recorder's pill |
| `src/components/Recorder.tsx` | notes, live transcript, the meter and stop; drawn again (`key`: the state's `part`) when the recording's next meeting starts, its notes empty; a screenshot put where the cursor is (or was last); the title edited in place, sent as the notes are (`set_title(part, title)`: this meeting and the next ones of the recording, `series::Notebook`); no speaker is named while it records (`docs/decisions/2026-11-04-speakers-named-during-the-meeting.md`) |
| `src/lib/shots.ts` | the screenshots' `shot` addresses and shortcuts (their lines in the notes, `![Screenshot 10:32](screenshot-….png)`, are crates/client `shots.rs`) |
| `src/components/NotesEditor.tsx` | the notes, edited as they look, for writing while listening (from a UX review): paragraphs, headings 1–3 (Backspace at the start makes a paragraph), lists, actions (`[] `), code (left with Enter on an empty last line, ⌘Enter or Esc), screenshots (a block with its caption: a click selects, a double click, Space or its button enlarges, draggable; put in place of an empty line, or after the line being written without moving the cursor); no quotes, rules or strikes, no toolbar or menu; a paragraph always last. In and out as Markdown (tiptap-markdown), the form the client keeps and the report reads: a screenshot is written as a block of its own (`addStorage`), or the next block starts on its line; any image that is not a screenshot is left out |
| `src/components/Shots.tsx` | a screenshot over the whole window, from the report or the notes |
| `src/components/ModelsProgress.tsx` | the models' download and loading as a bar, on the home page and in the live transcript until they are ready |
| `src/components/MeetingPage.tsx` | a kept meeting: Send to brainy for one published without it (brainy off in the settings when it ended: `send_to_brainy`, client `meeting::send_to_brainy`, a new page under the settings' parent with the report, or the notes, under its header, never the transcript; Write it again only rewrites the report of a page there is), report, notes (the same editor, saved 400 ms after typing stops, on blur and on leaving; the report and the brainy page are not rewritten), transcript (a phrase after 30 s of silence shows its time, any on hover; `get_meeting` sends the times on the displayed clock, 00:00 at the first word, as the page has them), renaming speakers (the field searches the voices known by name as the name is typed, those the speaker's voice is close to first, marked: `speaker_candidates`, from the voices kept for the last 20 meetings; never a name another speaker of the meeting has; arrows and Enter pick one, Enter again names; the report written again 8 s after the last rename, or on leaving the meeting, never while a name is being typed or one is being written: `write_report` and `rename_speaker` each load and save the kept meeting, and one would undo the other) |
| `src/components/VoicesPage.tsx` | the voices known by name, a page of their own (the sidebar's Voices, or Manage voices in the settings), made for hundreds of people: searched as one types (any case or accent, ⌘F), sorted by when they were last heard (`Person::last_heard`: the latest meeting a speaker took their name in by voice, kept at the meeting's end by `voices::heard_in`, or the latest print's; linked when the meeting is still kept) or by name, either way round (newest or oldest first, A–Z or Z–A), those with no print of 30 s (`GOOD_ENROLL`) marked "Needs more voice" and filterable; renamed in place (`rename_voice`, `Library::rename`: refused onto someone else's name, any case; meetings already written keep the old one) or forgotten after asking (`forget_voice`) |
| `src/lib/platform.ts` | `android`: what the pages hide or say otherwise on a phone |
| `src/components/SettingsPage.tsx` | the settings (brainy's parent page, checked by "Save and check": `check_brainy` resolves it; the language model of the report: a provider, its address kept and the provider read back from it, then one of the models it lists at `/models`: `list_models`, engine `llm::models`), how many voices are known by name with a link to their page, the data folder |

## Traps

- **What a meeting does is decided in crates/client** (`meeting.rs`):
  the app only chooses its options (`Settings::publish`, `sources`). A
  step added in `recorder.rs` is a step `logbook finish` skips for the
  same meeting left pending.
- **Pending meetings are finished through `Pending::claim`**
  (`finisher.rs`): a meeting the recorder (or a `logbook finish`)
  holds is left to it. `Pending::open` only reads, for the list.
- **A run of the finisher is started again, never cut short.** Asked for
  while one goes on, it ends that run before its next meeting (or its
  wait), then starts one with the settings then. Stopping a run in the
  middle of a meeting would drop it between a brainy write and the note
  of it, and the next attempt would write that part again.
- **Voiceprints never go to the pages.** `list_voices` sends each
  person's name, seconds, meetings and whether they need more voice;
  their embeddings stay in the Rust side, like the library itself
  (`voices.rs`). Anything the webview holds is within reach of whatever
  it renders.
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
  for every build signed with it. `scripts/app.sh dev` signs the
  debug binary with it too (cargo's runner, `signed-run`).
- **The report and the notes are rendered as React elements**
  (`Markdown.tsx`, react-markdown without raw HTML). A model's output in
  `dangerouslySetInnerHTML` is markup in a window that can call the
  app's commands. Its images are the meeting's screenshots only, by
  file name through the `shot` scheme, which serves
  `screenshots/<name>` and `meetings/<id>/<name>` and nothing else of
  the data folder: an image from anywhere else is not loaded, so a
  report cannot make the window fetch an address.
- **A screenshot is a line of the notes, the file kept beside the
  meeting** (crates/client `shots.rs`): taken into `<data>/screenshots`,
  copied into the pending folder when the meeting ends (the notes name
  the files), into `<data>/meetings/<id>/` once it is published, and
  uploaded to brainy as an image block in its line's place
  (`Brainy::insert_with_images`: brainy's Markdown drops any image that
  is not an http(s) address). The report's model is asked to copy the
  lines where they belong; one it drops is added at the report's end
  (`shots::with_missing`). `<data>/screenshots` is emptied when the next
  recording starts, not when a meeting ends: a meeting of the series
  still finishing copies its screenshots from there.
- **A meeting is deleted at once, with a notice to undo it** (App.tsx,
  8 s, ⌘Z; ⌘⌫ deletes the one shown, Backspace or Delete a row of the
  list, the system's menu on a right click): no confirmation, which
  people learn to click through. It is moved to `<data>/trash`
  (`history::trash`), back with `restore_meeting`, gone with
  `purge_meeting` when the notice goes, and the whole trash at the app's
  next start (`history::empty_trash`): a delete is never left half done
  by the app closing during the undo. The brainy page stays.
- **The shortcuts are registered only while a meeting is recorded**
  (`screenshot::listen`, `unlisten`): a global shortcut takes the keys
  from every other app. macOS asks for Screen Recording the first time;
  refused, the capture holds the desktop without the other apps'
  windows.
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

- **Windows is cross-built from a Mac** (`scripts/app.sh windows`:
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

## Android

The app on a phone records the microphone, sends it to a logbook-server
and publishes as the desktop app does with a server; it links no model
code. Why, and what it is not: `docs/decisions/2026-10-29-android-app.md`.
Built with `scripts/app.sh android` (README).

| File | What it does |
|---|---|
| `src-tauri/src/android.rs` | before the microphone opens: the permissions, the battery exemption asked once, the foreground service started (its Stop is `Recorder::stop`); at the end the service stopped and the finisher started (finisher.rs) |
| `plugins/recording/` | `tauri-plugin-recording`: the Rust handle (`RecordingExt`, `run_mobile_plugin`) and `android/` (Kotlin): `RecordingPlugin` (permissions, start, update, stop, battery optimization) and `RecordingService` (foreground service of type microphone, its notification, the wake and Wi-Fi locks); its manifest brings the permissions and the service |
| `src-tauri/gen/android/` | the Android project `tauri android init` made (kept; its build outputs are not): `minSdk` 28, `MainActivity` keeping the pages inside the system bars and the keyboard |
| crates/client `audio.rs`, `aaudio` | the microphone through AAudio with the `UNPROCESSED` preset, else `VOICE_RECOGNITION` (cpal sets none) |

### Traps

- **Model code stays out by features, not by `cfg`.** The app's
  `Cargo.toml` takes logbook-client and logbook-engine without default
  features and adds `local`/`models` for non-Android targets only. A
  dependency (or a `use`) that pulls `logbook-engine/models` into the
  Android build links libparakeet's loader into the phone; check with
  `cargo tree -p logbook-app --target aarch64-linux-android -e features`.
- **`run` is `tauri::mobile_entry_point`.** Without it the Android library
  exports nothing and the linker drops the whole app: an APK that opens on
  nothing.
- **The service starts from the start button.** Android 12+ refuses a
  foreground service started from the background, and 14+ a microphone
  one before `RECORD_AUDIO` is granted: `android::begin` asks first, in the
  recording's task, never at the app's start.
- **The plugin's calls block** (a permission waits for the person): from
  `spawn_blocking`, never on the main thread or an async worker.
- **Settings on Android**: a server always (`Settings::server`), the
  microphone only (`sources`), no echo, `Gain::Max` when the gain is on
  (on by default). Secrets in `secrets.json` in the app's config folder,
  mode 600: weaker than a keychain (the decision record, "Secrets").

## Not yet

- On Android the notification keeps the title the meeting started
  with: the plugin's `update` changes its text only.

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
- The rename on a real brainy page: `rename_speaker` matches the
  header's list of speakers as `list_blocks` returns it, read from
  brainy's code, not seen (the report under it is written again with
  the new names; the turn headings it also matches are only on pages
  made before the page held the report alone).
- Windows: the installer is built (`scripts/app.sh windows`), never
  run; the window's frame (`titleBarStyle` is macOS's) is untried. The
  app and the CLI, built on Windows and run over Remote Desktop (`pnpm
  tauri dev`), record, transcribe on the Vulkan build and finish a
  meeting, the microphone once its buffer was raised
  (`docs/decisions/2026-10-27-windows-audio-buffer.md`). Building there
  needs Visual Studio's C++ Build Tools and CMake (Opus).
- Linux: not built.
- Android: built (a debug APK, arm64), never run on a phone.
- A meeting started from the calendar, or when a call starts.
