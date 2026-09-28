#!/usr/bin/env bash
# Publish AutoJev installers: upload every recognized file to the R2 bucket
# behind cdn.autojev.ai and rewrite latest.json, the self-update feed polled
# by the desktop app's updater plugin.
#
#   scripts/publish-release.sh <release files...>
#   e.g. scripts/publish-release.sh dist/AutoJev_*          (from CI)
#
# Recognized installers (produced by release.yml's build matrix):
#   AutoJev_X.Y.Z_aarch64.dmg, AutoJev_X.Y.Z_x64.dmg,
#   AutoJev_X.Y.Z_x64-setup.exe, AutoJev_X.Y.Z_amd64.AppImage, AutoJev_X.Y.Z_amd64.deb
# Recognized updater artifacts (each needs its .sig):
#   AutoJev_X.Y.Z_aarch64.app.tar.gz → darwin-aarch64
#   AutoJev_X.Y.Z_x64.app.tar.gz     → darwin-x86_64
#   AutoJev_X.Y.Z_x64-setup.exe      → windows-x86_64 (the installer itself)
#   AutoJev_X.Y.Z_amd64.AppImage     → linux-x86_64   (the AppImage itself)
# CI's unversioned mac names (AutoJev_aarch64.app.tar.gz / AutoJev_x64.app.tar.gz)
# are accepted and renamed to the versioned form on upload.
#
# Requires wrangler auth (interactive login locally; CLOUDFLARE_API_TOKEN +
# CLOUDFLARE_ACCOUNT_ID in CI).
set -euo pipefail

R2_BUCKET="autojev"
DOWNLOAD_BASE="https://cdn.autojev.ai"

FILES=("$@")
[ "${#FILES[@]}" -gt 0 ] || { echo "usage: publish-release.sh <release files...>" >&2; exit 1; }

VERSION=""
UPD_MAC_ARM=""; UPD_MAC_ARM_SIG=""
UPD_MAC_X64=""; UPD_MAC_X64_SIG=""
UPD_WIN="";     UPD_WIN_SIG=""
UPD_LINUX="";   UPD_LINUX_SIG=""

# Pass 1: pin VERSION from the versioned names, so pass 2 can rename CI's
# unversioned mac updater archives (AutoJev_<arch>.app.tar.gz) on upload.
for f in "${FILES[@]}"; do
  [ -f "$f" ] || { echo "error: no such file: $f" >&2; exit 1; }
  V="$(basename "$f" | sed -nE 's/^AutoJev_([0-9]+\.[0-9]+\.[0-9]+)_.+$/\1/p')"
  [ -z "$V" ] && continue
  if [ -n "$VERSION" ] && [ "$VERSION" != "$V" ]; then
    echo "error: mixed versions ($VERSION vs $V)" >&2; exit 1
  fi
  VERSION="$V"
done
[ -n "$VERSION" ] || { echo "error: no recognizable installer among inputs" >&2; exit 1; }

put() {
  npx --yes wrangler r2 object put "$R2_BUCKET/$1" --file="$2" --content-type="$3" --remote
}

for f in "${FILES[@]}"; do
  NAME="$(basename "$f")"
  case "$NAME" in
    AutoJev_aarch64.app.tar.gz*) NAME="AutoJev_${VERSION}_aarch64.app.tar.gz${NAME#AutoJev_aarch64.app.tar.gz}" ;;
    AutoJev_x64.app.tar.gz*)     NAME="AutoJev_${VERSION}_x64.app.tar.gz${NAME#AutoJev_x64.app.tar.gz}" ;;
  esac
  case "$NAME" in
    AutoJev_${VERSION}_*) ;;
    *) echo "skip (unrecognized name): $NAME"; continue ;;
  esac

  # Updater signatures are inlined into latest.json, not uploaded as objects.
  case "$NAME" in
    *_aarch64.app.tar.gz.sig) UPD_MAC_ARM_SIG="$(cat "$f")"; continue ;;
    *_x64.app.tar.gz.sig)     UPD_MAC_X64_SIG="$(cat "$f")"; continue ;;
    *_x64-setup.exe.sig)      UPD_WIN_SIG="$(cat "$f")"; continue ;;
    *_amd64.AppImage.sig)     UPD_LINUX_SIG="$(cat "$f")"; continue ;;
    *.sig) echo "    (skipped, unknown signature: $NAME)"; continue ;;
  esac

  CONTENT_TYPE="application/octet-stream"
  case "$NAME" in *.tar.gz) CONTENT_TYPE="application/gzip" ;; esac

  echo "==> Uploading $NAME to R2 bucket $R2_BUCKET"
  put "$NAME" "$f" "$CONTENT_TYPE"

  case "$NAME" in
    *_aarch64.app.tar.gz) UPD_MAC_ARM="$DOWNLOAD_BASE/$NAME" ;;
    *_x64.app.tar.gz)     UPD_MAC_X64="$DOWNLOAD_BASE/$NAME" ;;
    *_x64-setup.exe)      UPD_WIN="$DOWNLOAD_BASE/$NAME" ;;
    *_amd64.AppImage)     UPD_LINUX="$DOWNLOAD_BASE/$NAME" ;;
  esac
done

# An artifact without its .sig is a hard error: a feed entry the app can't
# verify would brick that platform's updater.
for pair in "darwin-aarch64:$UPD_MAC_ARM:$UPD_MAC_ARM_SIG" \
            "darwin-x86_64:$UPD_MAC_X64:$UPD_MAC_X64_SIG" \
            "windows-x86_64:$UPD_WIN:$UPD_WIN_SIG" \
            "linux-x86_64:$UPD_LINUX:$UPD_LINUX_SIG"; do
  PLAT="${pair%%:*}"; REST="${pair#*:}"; U="${REST%%:*}"; SG="${REST#*:}"
  if [ -n "$U" ] && [ -z "$SG" ]; then
    echo "error: $PLAT updater artifact given without its .sig — pass both" >&2; exit 1
  fi
done
[ -n "$UPD_MAC_ARM$UPD_MAC_X64$UPD_WIN$UPD_LINUX" ] || { echo "error: no updater artifacts among inputs" >&2; exit 1; }

LATEST_JSON="$(mktemp -d)/latest.json"
VERSION="$VERSION" \
U_MAC_ARM="$UPD_MAC_ARM" S_MAC_ARM="$UPD_MAC_ARM_SIG" \
U_MAC_X64="$UPD_MAC_X64" S_MAC_X64="$UPD_MAC_X64_SIG" \
U_WIN="$UPD_WIN"         S_WIN="$UPD_WIN_SIG" \
U_LINUX="$UPD_LINUX"     S_LINUX="$UPD_LINUX_SIG" \
node -e '
  const p = {};
  const add = (k, u, s) => { if (u && s) p[k] = { signature: s, url: u }; };
  add("darwin-aarch64", process.env.U_MAC_ARM, process.env.S_MAC_ARM);
  add("darwin-x86_64",  process.env.U_MAC_X64, process.env.S_MAC_X64);
  add("windows-x86_64", process.env.U_WIN,     process.env.S_WIN);
  add("linux-x86_64",   process.env.U_LINUX,   process.env.S_LINUX);
  require("fs").writeFileSync(process.argv[1], JSON.stringify({
    version: process.env.VERSION,
    pub_date: new Date().toISOString(),
    platforms: p,
  }, null, 2) + "\n");
  console.log("    platforms:", Object.keys(p).join(", "));
' "$LATEST_JSON"
echo "==> Uploading latest.json (self-update feed)"
put latest.json "$LATEST_JSON" application/json
echo "✓ published AutoJev v$VERSION → $DOWNLOAD_BASE/latest.json"
