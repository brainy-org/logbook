# The Android app: the microphone sent to a server, kept recording by a foreground service

**Verdict.** The desktop app (`apps/desktop`) also builds for Android
(arm64, a debug APK for now), as an experiment. On a phone it records the
microphone and streams it to a logbook-server (Opus on the live socket),
publishes the meeting to brainy with the person's token, and has the
report written by the server's language model: the desktop app's flow
with a server, unchanged (`crates/client`, `meeting.rs`). It is **not** a
transcriber: no local backend, no libparakeet, no model download; the
Android binary links no model code (cargo features: engine `models`,
client `local`, both default on and off in the Android build). It keeps
recording with the screen off and the app in the background through a
foreground service of type `microphone` with an ongoing notification
(with Stop), a partial wake lock and a Wi-Fi lock, held from the start of
a recording until its last meeting has finished; Android's battery
optimization exemption is asked once. The microphone is opened through
AAudio itself with the `UNPROCESSED` preset, the raw microphone
(`VOICE_RECOGNITION` if that fails), and its level raised by AGC2 at its most (`Gain::Max`), so that
the far side of a room is heard. Pending meetings are finished again on
their own while the app is open, until the server and brainy answer. The
secrets are in a file in the app's private folder (mode 600), which is
weaker than the macOS keychain. Built, not run on a phone yet.

## What it is and is not

- **A recorder for a server.** `Settings::server()` ignores
  `transcription` on Android: there is no other backend. Without a server
  address a recording does not start ("no logbook server is set").
- **The microphone only.** There is no call to take on a phone
  (`Source::System` is never asked for); the home page offers no mode: the
  phone's microphone is diarized ("In the room"), the people around it
  told apart. No echo canceller runs (there is no far end), only the gain.
- **The report and the clean-up by the server's model**, as the desktop
  app with a server (`docs/decisions/2026-10-27-desktop-app.md`, "The
  report where the transcript is made"). `polish.rs` and `report.rs` are
  plain HTTP and are built for Android (the model list in the settings,
  `write_report` on a kept meeting), but nothing on the phone holds a
  model's key unless one is typed.
- **No devices to choose**, no models' progress, no keychain migration,
  one window (no single-instance plugin: Android has one activity).
- **The data folder is the app's own** (`app_data_dir`, under
  `/data/data/io.github.brainy_org.logbook/`): `dirs` has none on
  Android. The CLI does not share it.

## Why no models

The models are about 1 GB and the ASR wants a desktop CPU or a GPU to
keep up with speech; a phone running them for an hour would be hot and
flat, and libparakeet has no Android build in the mirror. The point of
the phone is to be in the room, not to transcribe: the server does that
for every client. Gating the code by features rather than by
`cfg(target_os)` keeps the rule checkable on the desktop too
(`cargo clippy -p logbook-client --no-default-features`), and makes the
Android binary carry no loader, no mirror and no model pins it cannot use:

- engine `models` (default): `parakeet`, `mirror`, `batcher`, `channel`,
  `final_pass`, `finalizer`, `polisher`, `progressive`, `session`,
  `speakers` and `Engine`/`Options` (`engine.rs`), with libloading, sha2,
  tar, zip, flate2 and tokio. Always built: `protocol` (now holding
  `SAMPLE_RATE` and `Word`, which `parakeet` re-exports), `codec`,
  `recording`, `polish`, `report`, `echo`, `finals`, `store`, `pins` (the
  model files' pins, which voices.rs names a voice library by).
- client `local` (default, needs `models`): `local.rs` and
  `Backend::Local`.
- the app: the base dependencies take both crates without default
  features; `cfg(not(target_os = "android"))` adds `local`/`models`,
  keyring and the single-instance plugin; `cfg(target_os = "android")`
  adds `tauri-plugin-recording`. Cargo's resolver leaves a target's
  features out of another target's build.

Checked on the built `liblogbook_desktop.so` (`llvm-nm -C`): no symbol of
`logbook_engine::{parakeet, mirror, session, batcher, progressive,
speakers, Engine}`, `logbook_client::local` or libloading, while
`codec`, `polish`, `remote`, the AAudio capture and the plugin are
there; no `libparakeet` string; it needs only libandroid, libaaudio,
liblog, libdl, libm and libc. The model files' names and sha256 are in
it as data (`pins.rs`, which names a voice library), never fetched.
`cargo tree -p logbook-desktop --target aarch64-linux-android -e
features` shows neither `models` nor `local`.

## Recording with the screen off

Android 14 and 15 stop a background app's microphone and network unless
the app runs a foreground service of the matching type. The Kotlin side
is a Tauri 2 mobile plugin kept here (`apps/desktop/plugins/recording`):

- `RecordingService`: `startForeground` with type `microphone`
  (`FOREGROUND_SERVICE_MICROPHONE`), an ongoing low-importance
  notification titled with the meeting, "Recording" with a Stop action,
  then "Finishing…" without it; a `PARTIAL_WAKE_LOCK` so the CPU runs with
  the screen off, and a Wi-Fi lock (`WIFI_MODE_FULL_HIGH_PERF`,
  deprecated in API 34 but still honoured) so the socket is not left to
  Wi-Fi power saving. `START_NOT_STICKY`: a service started again after
  the process died would have no recording behind it.
- `RecordingPlugin`: `requestPermissions` (Tauri's own, aliases
  `microphone` and `notifications`), `start`/`update`/`stop`, and the
  battery optimization state and request
  (`ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, asked once by the
  recorder, again from the settings).
- The app's Rust (`src-tauri/src/android.rs`, called from
  `recorder.rs`): before the microphone opens it asks the permissions
  (a refused microphone ends the start with a message), asks the
  exemption once, and starts the service with a channel the
  notification's Stop calls back on (`Recorder::stop`, as the page's
  button). When the recording stops, the notification says
  "Finishing…"; the service is stopped once `series::meetings` returns,
  after the last final pass, page and report: those need the network too.

The service is started from the start button, with the app in the
foreground: Android 12+ refuses to start a foreground service from the
background, and 14+ a microphone one without `RECORD_AUDIO` granted.

The audio stays in Rust (the capture thread, Opus, the socket, the
pending folder): the service only keeps the process alive and allowed to
use the microphone. A recording that runs for days is cut into meetings
as on the desktop (`docs/decisions/2026-10-29-bounded-meetings.md`); the
pending folders cover the network coming and going.

## The microphone

- **AAudio's `UNPROCESSED` preset, else `VOICE_RECOGNITION`, never
  `VOICE_COMMUNICATION`.** `UNPROCESSED` is the raw microphone: no
  effects, no gain, a flat response; whatever raises the level is ours
  (AGC2, below), the same on every phone. `VOICE_RECOGNITION` is meant
  for speech recognition from a distance and, by Android's compatibility
  definition, has no handset noise suppression or echo cancelling and the
  platform's AGC off, but the vendor's HAL may still shape it: on the S10e
  its peaks came out compressed (see "Tried on the phone"). It is the
  fallback where `UNPROCESSED` does not open. `VOICE_COMMUNICATION` would
  add a call's processing, whose noise suppression removes distant
  voices. cpal 0.18 opens AAudio streams without setting a preset, so on
  Android the microphone is opened through the `ndk` crate's AAudio API
  directly (client `audio.rs`, `aaudio`), behind the same contract as the
  cpal capture: mono 16 kHz frames on the channel, on the session's clock
  (`Timeline`: gaps filled with silence, an idle stream moving along).
  16-bit mono at 16 kHz is asked for; whatever AAudio gives is mixed down
  and resampled. The preset needs API 28: the app's `minSdk` is 28.
- **AGC2 at its most** (`Gain::Max`, client `aec.rs`; the CLI's
  `--mic-gain max`), on by default on Android ("Raise faint voices" in the
  settings): the adaptive digital controller up to 50 dB (its design
  ceiling, and the fixed gain's range), starting at 30 dB instead of 15
  and moving 10 dB a second instead of 6, so a faint voice comes up within
  seconds, and letting the output noise reach -25 dBFS instead of -50:
  that cap is what otherwise holds the gain down in a room with any hum
  (see "Tried on the phone" below for why -25 and not -40).
  The limiter after it keeps the raised signal from clipping.
- **The trade-off**: background noise, the room's hum and the phone's own
  handling sounds (a hand on the table, the phone moved) rise with the
  voices. The VAD and the ASR on the server cope with steady noise better
  than with a voice too quiet to be found; a louder room will show more
  false phrases.
- **Tried on the phone, 2026-10-29** (S10e, LineageOS 22, a quiet room,
  the Mac's server): the HAL grants `VoiceRecognition` at 16 kHz mono.
  With the noise cap at -40 dBFS the room's noise (about -46 dBFS) held
  the gain to a few dB: near the phone, speech peaked at 0 dBFS through
  the limiter; across the room it stayed near -35 dBFS RMS (peaks about
  -18) and the server transcribed nothing of 30 s of it. The same 30 s
  raised 20 dB offline (noise near -26 dBFS) gave every phrase through
  the local pipeline. The cap is now -25 dBFS.
- **Then, with the cap at -25** and the mic's level logged every 10 s
  before and after the gain (client `aec.rs`, "mic level over 10 s"), a
  phrase said near the phone, then across the room, about 20 s each way:
  - `VOICE_RECOGNITION`: in -34.1 dBFS RMS (peak -4.0), out -22.1 (peak
    0); the far "Prova da lontano?" was transcribed.
  - `UNPROCESSED` (granted: the log names it): in -36.9 dBFS RMS (peak
    -13.9), out -18.5 (peak 0), then in -30.5 (peak -10.9), out -26.8
    (peak -8.7) while AGC2 settled; the far phrase was transcribed both
    times it was said, and the conversation after it.
  - A similar level with peaks 10 dB lower: the HAL compresses
    `VOICE_RECOGNITION`; the raw signal leaves our gain the headroom.
    `UNPROCESSED` comes first since. Two short trials of different
    phrases: the direction, not a measure; a meeting at a few metres for
    longer is what would settle it.

## Pending meetings finish on their own

On the desktop `logbook finish --wait` exists; on a phone the app is the
only way to finish a meeting left pending. `android::Finisher` runs
`finishing::finish_all` with `wait` (the backoff of
`docs/decisions/2026-10-29-finish-waits-for-the-server.md`) at the app's
start, after each recording, and from "Finish now": one run at a time,
which lists the folders again each round. It runs while the process
lives; with the app closed and no recording, Android may kill it, and it
starts again at the next launch. A run started with old settings keeps
them until it ends.

## TLS

reqwest 0.13 verifies certificates with rustls-platform-verifier, which
on Android needs a JNI context handed to it at start and its own Kotlin
component in the Gradle project; without them every https request to
brainy would fail. On Android the client's HTTP clients trust Mozilla's
roots instead (`webpki-root-certs`, client `remote.rs`, `http_builder`),
as ureq already does for the language model. The cost: a certificate
from a CA installed on the phone by the user (a company's private CA) is
not trusted. The live socket has no TLS anywhere (tokio-tungstenite
without it): the server is reached over `ws://`, as from the desktop.

## Secrets

`keyring` has no Android backend. The server's token, brainy's token and
the model's key are in `secrets.json` in the app's config folder, mode
600, as on the desktop (`settings.rs`, `Secret`). Android's app sandbox
keeps other apps out, but the file is plain text to anything with root
or a backup of the app's data; the macOS keychain encrypts at rest and
asks the user. The Android Keystore would be the equivalent; not done
for a prototype.

## What the person must do on the phone

- Allow the microphone and the notifications when asked at the first
  recording (Settings → Apps → Logbook → Permissions otherwise).
- Accept "Let app always run in background?" when asked, or later from
  the app's settings ("Background recording": Allow in background), or
  Android's Settings → Apps → Logbook → Battery → Unrestricted. This
  exemption from battery optimization (Doze and app standby) is what
  matters on stock Android and LineageOS, which the test phone runs
  (LineageOS 22, Android 15).
- On Samsung's One UI (stock firmware) also: Settings → Battery → Background
  usage limits: Logbook out of "Sleeping apps" and "Deep sleeping apps"
  (or in "Never sleeping apps"). An app cannot set this itself.
- The server's address must be reachable from the phone (not
  `127.0.0.1`): the LAN address or a public one.

## Not verified

- Nothing was run on a phone: the APK was built only. Permissions,
  the service, the notification's Stop, the wake lock with the screen
  off for hours, the network across Wi-Fi changes, AAudio's preset and
  the gain are all untried.
- The pages at phone width were built and type-checked, not seen on a
  screen; the content is kept inside the system bars and the keyboard by
  `MainActivity` (edge to edge is enforced from Android 15).
- An AAudio stream stopped by the system (a headset plugged in, the
  device lost) is not opened again: its channel goes on as silence until
  the recording stops.
- Release builds (signing, minification) are not set up.

## Rejected

| Alternative | Why not |
|---|---|
| The models on the phone | 1 GB, too slow and too hot for an hour; no libparakeet for Android in the mirror |
| `cfg(target_os = "android")` around the model code instead of features | the rule could not be checked on a Mac, and nothing would stop a new model module from being linked |
| The capture in Kotlin (`AudioRecord`) passed to Rust | a second capture path with its own clock and buffers; AAudio from Rust keeps one (`Timeline`) |
| The service only while the microphone is open | the last meeting's final pass, page and report would be cut off when the screen goes off after Stop |
| `VOICE_COMMUNICATION` | its noise suppression and AGC are for a handset at the mouth; distant voices are removed as noise |
| rustls-platform-verifier on Android | its JNI initialisation and Kotlin component in the Gradle project, for the phone's user-installed CAs only |
| A file in shared storage for the secrets | readable by other apps; the app's private folder is not |
