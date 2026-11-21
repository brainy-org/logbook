#!/usr/bin/env bash
# The app (apps/logbook), for the desktop and Android, from this checkout:
#
#   scripts/app.sh dev     run it with the pages reloading as they change
#   scripts/app.sh cert    once per Mac: a self-signed code-signing
#                          certificate in the login keychain
#   scripts/app.sh build   Logbook.app and a .dmg, signed with it, in
#                          target/release/bundle/
#   scripts/app.sh windows the Windows installer (x64, NSIS), cross-built
#                          from this Mac, in target/x86_64-pc-windows-msvc/
#                          release/bundle/nsis/; not signed
#   scripts/app.sh android the Android app (arm64), a debug APK in
#                          apps/logbook/src-tauri/gen/android/app/build/
#                          outputs/apk/; records and sends to a server
#                          (docs/decisions/2026-10-29-android-app.md)
#
# Signed, not notarized: notarization needs an Apple Developer ID. The
# certificate is what keeps macOS's permissions (microphone, the call's
# audio) from one build to the next: they are tied to the signature, and
# an ad-hoc one changes with every build
# (docs/decisions/2026-10-27-desktop-app.md). A Mac that did not build the
# app blocks it at first open: System Settings → Privacy & Security →
# Open Anyway, or `xattr -dr com.apple.quarantine /Applications/Logbook.app`.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/apps/logbook"
# The certificate's name: LOGBOOK_SIGNING_IDENTITY picks another one (a
# Developer ID, once there is one).
identity="${LOGBOOK_SIGNING_IDENTITY:-Logbook Self-Signed}"
# The key that signs the app's updates (src-tauri updater.rs): its public
# half is in tauri.conf.json. Without it, a build has no updater files
# (a build to try, not one to release).
updater_key="${LOGBOOK_UPDATER_KEY:-$HOME/.config/logbook/updater.key}"
no_updater=()
if [[ -s "$updater_key" ]]; then
  TAURI_SIGNING_PRIVATE_KEY="$(<"$updater_key")"
  export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
else
  no_updater=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi

install() {
  if [[ ! -d "$app/node_modules" ]]; then
    (cd "$app" && pnpm install --frozen-lockfile)
  fi
}

has_identity() {
  security find-certificate -c "$identity" >/dev/null 2>&1
}

case "${1:-}" in
dev)
  install
  # Each build of `tauri dev` is a new binary: signed ad hoc, macOS asks
  # again for the microphone and the call's audio. With the certificate, cargo runs it through
  # `signed-run`, which signs it first; without it, it runs as it is.
  if has_identity; then
    triple=$(rustc -vV | sed -n 's/^host: //p' | tr 'a-z-' 'A-Z_')
    export "CARGO_TARGET_${triple}_RUNNER=$root/scripts/app.sh signed-run"
  fi
  cd "$app" && exec pnpm tauri dev
  ;;
signed-run)
  # cargo's runner for `dev`: the binary, then its arguments. No hardened
  # runtime, as any debug build: libparakeet loads without entitlements.
  bin="$2"
  shift 2
  codesign --force --sign "$identity" "$bin"
  exec "$bin" "$@"
  ;;
cert)
  if has_identity; then
    echo "\"$identity\" is already in the keychain"
    exit 0
  fi
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  cat >"$work/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $identity
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
  # macOS's own LibreSSL: `security import` reads its PKCS#12, not the
  # one of a newer OpenSSL from Homebrew ("MAC verification failed").
  ssl=/usr/bin/openssl
  # Ten years: the permissions last as long as the certificate is the same.
  "$ssl" req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$work/cert.cnf" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
  "$ssl" pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -name "$identity" -passout pass:logbook -out "$work/cert.p12"
  keychain="$HOME/Library/Keychains/login.keychain-db"
  security import "$work/cert.p12" -k "$keychain" -P logbook -T /usr/bin/codesign
  # Trusted for code signing on this Mac: macOS asks for your password.
  security add-trusted-cert -r trustRoot -p codeSign -k "$keychain" "$work/cert.pem"
  echo "\"$identity\" is in the login keychain"
  ;;
build)
  install
  if ! has_identity; then
    echo "no \"$identity\" certificate: run scripts/app.sh cert first" >&2
    exit 1
  fi
  cd "$app"
  # CI=true: the bundler skips the AppleScript that opens the .dmg in
  # Finder to lay out its window. The .dmg keeps Finder's default layout.
  CI=true APPLE_SIGNING_IDENTITY="$identity" pnpm tauri build ${no_updater[@]+"${no_updater[@]}"}
  bundle="$root/target/release/bundle/macos/Logbook.app"
  codesign --verify --deep --strict "$bundle"
  codesign -d --entitlements - --xml "$bundle" 2>/dev/null | grep -q disable-library-validation
  echo "signed by \"$identity\": $bundle"
  ls "$root/target/release/bundle/dmg/"*.dmg
  ;;
windows)
  # cargo-xwin fetches Microsoft's CRT and SDK on first use (accepting
  # their licence) and links with LLVM's clang-cl and lld-link; makensis
  # writes the installer. Homebrew's llvm is keg-only: on the PATH here.
  for tool in cargo-xwin makensis; do
    if ! command -v "$tool" >/dev/null; then
      echo "no $tool: brew install llvm nsis && cargo install --locked cargo-xwin" >&2
      exit 1
    fi
  done
  rustup target add x86_64-pc-windows-msvc >/dev/null
  install
  cd "$app"
  PATH="$(brew --prefix llvm)/bin:$PATH" pnpm tauri build \
    --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis \
    ${no_updater[@]+"${no_updater[@]}"}
  ls "$root/target/x86_64-pc-windows-msvc/release/bundle/nsis/"*.exe
  ;;
android)
  # The SDK and NDK where Android Studio or Homebrew's commandlinetools
  # put them (sdkmanager "platform-tools" "platforms;android-35"
  # "build-tools;35.0.0" "ndk;27.2.12479018"), and a JDK 17 or 21.
  # No CC or linker of our own: Tauri picks the NDK's clang for minSdk 28
  # (AAudio's input preset), and an older one lacks libaaudio.
  export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
  if [[ -z "${NDK_HOME:-}" ]]; then
    NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)"
    export NDK_HOME
  fi
  # opusic-sys builds libopus with CMake and finds the NDK by this name.
  export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$NDK_HOME}"
  if [[ -z "${JAVA_HOME:-}" && -d /opt/homebrew/opt/openjdk@21 ]]; then
    export JAVA_HOME=/opt/homebrew/opt/openjdk@21
  fi
  if [[ ! -d "$NDK_HOME" ]]; then
    echo "no NDK under $ANDROID_HOME/ndk: see the README, The Android app" >&2
    exit 1
  fi
  rustup target add aarch64-linux-android >/dev/null
  install
  cd "$app"
  # Line tables only: full debug info makes the APK about 450 MB.
  CI=true CARGO_PROFILE_DEV_DEBUG=line-tables-only \
    pnpm tauri android build --debug --apk --target aarch64
  ls "$app"/src-tauri/gen/android/app/build/outputs/apk/*/debug/*.apk
  ;;
*)
  echo "usage: scripts/app.sh dev | cert | build | windows | android" >&2
  exit 2
  ;;
esac
