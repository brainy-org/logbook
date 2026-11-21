# The app updates itself from the GitHub releases

The desktop app (macOS, Windows) looks for a new version in the GitHub
releases of brainy-org/logbook, with Tauri's updater plugin, and offers it;
the person installs it when they choose, never during a recording
(`apps/logbook/src-tauri/src/updater.rs`). The repository was made public
for it: the updater downloads without credentials.

## How

- **Where**: `https://github.com/brainy-org/logbook/releases/latest/download/latest.json`
  (`plugins.updater.endpoints`). `scripts/release.sh` attaches it to each
  release with the files it names: the macOS bundle as
  `Logbook_<version>_aarch64.app.tar.gz` (`darwin-aarch64`) and the Windows
  NSIS installer (`windows-x86_64`), each with its signature.
- **Drafts**: `releases/latest` is the newest *published* release, so a
  draft is offered to nobody; publishing it (`gh release edit vX.Y.Z
  --draft=false`) is what ships it.
- **Signatures**: the updater checks a minisign signature against the
  public key in `tauri.conf.json`, separate from macOS code signing and
  from any Windows signing. The private key, made with `tauri signer
  generate`, no password (releases are built unattended), lives only in
  `~/.config/logbook/updater.key` on the Mac that releases. Lost or
  replaced, every app already installed refuses every later update and
  must be reinstalled by hand. `scripts/app.sh` signs with it when it is
  there; without it a build has no updater files (`createUpdaterArtifacts`
  off), and `scripts/release.sh` refuses to run.
- **When**: 20 s after the start and every 6 hours; Settings → Version →
  Check for updates at any time. A failure (offline, GitHub down) is a log
  line, and the next look tries again.
- **Installing restarts the app**, so it is refused while a meeting is
  recorded: the recording would end mid-sentence. A pending meeting being
  finished is not waited for: its `.lock` goes with the process and the
  finisher takes it up at the next start, as after a crash.
- **The restart is the app's own on macOS**: Tauri's `app.restart()`
  starts the new process only after the exit event, and the app ends
  there with `parakeet::exit`, which never returns (it skips libparakeet's
  teardown): 0.2.0 installed 0.2.1 and quit without coming back. A
  detached `sh` waits for the old process to be gone, since a second
  instance started earlier hands over to it and quits (single-instance),
  then runs `open` on the bundle.
- **macOS**: the bundle is replaced in place and keeps its code signature
  (the self-signed certificate of `scripts/app.sh cert`), so the
  permissions (microphone, the call's audio) carry over. The download is
  made by the updater, not a browser, so no quarantine attribute is set
  and Gatekeeper does not block the new version.
- **Windows**: `installMode: passive`, the NSIS installer with a progress
  bar and no questions; it closes the app and starts the new one.
- **Android**: nothing; the APK is installed by hand.

## Not chosen

- **A public repository holding only the releases**, keeping the source
  private: one more repository to publish to; the source was made public
  instead.
- **A read-only GitHub token in the app**: anyone with the app could take
  it out.
- **Installing at once, unasked**: a restart in the middle of the
  person's work, and the notes being typed lost with the window.

## Known limits

- The apps installed before this change (0.1.1 and earlier) have no
  updater: the first release with it is installed by hand.
- The first end-to-end update is only proven once two releases with the
  updater exist.
