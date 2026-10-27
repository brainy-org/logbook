#!/usr/bin/env bash
# The desktop app (apps/desktop), from this checkout:
#
#   scripts/desktop.sh dev     run it with the pages reloading as they change
#   scripts/desktop.sh cert    once per Mac: a self-signed code-signing
#                              certificate in the login keychain
#   scripts/desktop.sh build   Logbook.app and a .dmg, signed with it, in
#                              target/release/bundle/
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
app="$root/apps/desktop"
# The certificate's name: LOGBOOK_SIGNING_IDENTITY picks another one (a
# Developer ID, once there is one).
identity="${LOGBOOK_SIGNING_IDENTITY:-Logbook Self-Signed}"

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
    export "CARGO_TARGET_${triple}_RUNNER=$root/scripts/desktop.sh signed-run"
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
    echo "no \"$identity\" certificate: run scripts/desktop.sh cert first" >&2
    exit 1
  fi
  cd "$app"
  APPLE_SIGNING_IDENTITY="$identity" pnpm tauri build
  bundle="$root/target/release/bundle/macos/Logbook.app"
  codesign --verify --deep --strict "$bundle"
  codesign -d --entitlements - --xml "$bundle" 2>/dev/null | grep -q disable-library-validation
  echo "signed by \"$identity\": $bundle"
  ls "$root/target/release/bundle/dmg/"*.dmg
  ;;
*)
  echo "usage: scripts/desktop.sh dev | cert | build" >&2
  exit 2
  ;;
esac
