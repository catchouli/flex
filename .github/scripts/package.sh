#!/usr/bin/env bash
# Packages a Windows release: flex.exe plus libmpv and ffmpeg, with every license and
# where to get the source of the bundled GPL parts.
#
#   package.sh <tag, e.g. v0.2.0> <path to flex.exe> <output dir>
#
# Needs: bash, curl, sha256sum (or shasum), gh (GH_TOKEN set), 7-Zip (`7z`, or set
# SEVENZIP), cargo-about. Makes a zip, and also an installer (flex.iss) when ISCC is set
# to Inno Setup's compiler. Runs on the GitHub Windows runner; also works on macOS for
# testing (without the installer).
set -euo pipefail

TAG=$1
EXE=$2
OUT=$(mkdir -p "$3" && cd "$3" && pwd)
SEVENZIP=${SEVENZIP:-7z}
ISCC=${ISCC:-}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)

# ffmpeg: a pinned gyan.dev build (GPLv3; includes NVENC and x264). Its versioned
# releases stay up, so this URL keeps working. To update: pick a newer release at
# https://github.com/GyanD/codexffmpeg/releases and paste its full_build-shared.7z
# digest here.
FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://github.com/GyanD/codexffmpeg/releases/download/${FFMPEG_VERSION}/ffmpeg-${FFMPEG_VERSION}-full_build-shared.7z"
FFMPEG_SHA256=4d2060a8b34a940aa47d785142055bb92a63053781e55f2ace4546edd519a8f5

# libmpv: the newest shinchiro build (it only keeps about two months of builds, so a
# pinned one would soon disappear). The exact build goes into the notices.
MPV_BUILDS=shinchiro/mpv-winbuild-cmake

fail() { echo "error: $*" >&2; exit 1; }
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

# --- Checks ---------------------------------------------------------------------------

[[ $TAG =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || fail "tag '$TAG' isn't vX.Y.Z"
VERSION=${BASH_REMATCH[1]}
CARGO_VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml")
[[ $VERSION == "$CARGO_VERSION" ]] ||
    fail "tag $TAG doesn't match the version in Cargo.toml ($CARGO_VERSION): bump [workspace.package] version first"
# The bundle includes GPL software, so Flex needs a (GPL-compatible) license of its own.
LICENSE_FILE=$(ls "$ROOT"/LICENSE* 2>/dev/null | head -1 || true)
[[ -n $LICENSE_FILE ]] || fail "Flex has no LICENSE file yet; add one before making a release"
[[ -f $EXE ]] || fail "no flex.exe at $EXE"

NAME="flex-${TAG}-windows-x64"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DIR="$WORK/$NAME"
mkdir -p "$DIR/licenses"

# --- ffmpeg ---------------------------------------------------------------------------

echo "Downloading ffmpeg $FFMPEG_VERSION"
curl -fsSL --retry 3 -o "$WORK/ffmpeg.7z" "$FFMPEG_URL"
[[ $(sha256 "$WORK/ffmpeg.7z") == "$FFMPEG_SHA256" ]] || fail "ffmpeg download doesn't match its checksum"
"$SEVENZIP" x -y -bso0 -o"$WORK/ffmpeg" "$WORK/ffmpeg.7z"
FF=$(echo "$WORK"/ffmpeg/ffmpeg-*)
cp "$FF"/bin/ffmpeg.exe "$FF"/bin/ffprobe.exe "$FF"/bin/*.dll "$DIR/"
cp "$FF/LICENSE" "$DIR/licenses/ffmpeg-LICENSE.txt"
cp "$FF/README.txt" "$DIR/licenses/ffmpeg-build-README.txt" # versions of everything in the build

# --- libmpv ---------------------------------------------------------------------------

MPV_TAG=$(gh release view -R "$MPV_BUILDS" --json tagName --jq .tagName)
MPV_ASSET=$(gh release view "$MPV_TAG" -R "$MPV_BUILDS" --json assets --jq '.assets[].name' |
    grep -E '^mpv-dev-x86_64-[0-9]{8}-git-[0-9a-f]+\.7z$' | head -1) ||
    fail "no mpv-dev-x86_64 package in $MPV_BUILDS release $MPV_TAG"
MPV_COMMIT=$(sed -E 's/.*-git-([0-9a-f]+)\.7z/\1/' <<<"$MPV_ASSET")
echo "Downloading $MPV_ASSET (mpv $MPV_COMMIT)"
gh release download "$MPV_TAG" -R "$MPV_BUILDS" -p "$MPV_ASSET" -D "$WORK"
"$SEVENZIP" x -y -bso0 -o"$WORK/mpv" "$WORK/$MPV_ASSET" libmpv-2.dll
cp "$WORK/mpv/libmpv-2.dll" "$DIR/"
for f in LICENSE.GPL Copyright; do
    curl -fsSL --retry 3 -o "$DIR/licenses/mpv-$f.txt" "https://raw.githubusercontent.com/mpv-player/mpv/$MPV_COMMIT/$f"
done

# --- Flex -----------------------------------------------------------------------------

cp "$ROOT/crates/flex-web/web/vendor/jassub/LICENSE.txt" "$DIR/licenses/jassub-LICENSE.txt"
JASSUB_VERSION=$(sed -n 's/^JASSUB \([0-9.]*\).*/\1/p' "$ROOT/crates/flex-web/web/vendor/jassub/README.txt")

cp "$EXE" "$DIR/flex.exe"
cp "$LICENSE_FILE" "$DIR/LICENSE.txt"
(cd "$ROOT" && cargo about generate --fail about.hbs) >"$DIR/licenses/rust-crates.txt"

cat >"$DIR/README.txt" <<EOF
Flex $TAG for Windows (64-bit)
https://github.com/catchouli/flex

Run flex.exe. Libraries, watch history and settings are kept in %APPDATA%\\Flex, so
updating doesn't lose anything.

To update: run the new release's installer, which stops and restarts Flex for you. Or,
with the zip: quit Flex (with "Close to system tray" on, right-click the tray icon and
choose Quit, or run "flex.exe --quit"), then replace these files with the new
release's.

Everything in this folder besides flex.exe is third-party software; see
THIRD-PARTY-NOTICES.txt.
EOF

cat >"$DIR/THIRD-PARTY-NOTICES.txt" <<EOF
Flex $TAG bundles the following software. Their licenses are in the licenses folder.

FFmpeg $FFMPEG_VERSION (ffmpeg.exe, ffprobe.exe and the DLLs other than libmpv-2.dll)
  License: GNU GPL version 3 (licenses/ffmpeg-LICENSE.txt). The libraries built in, and
  their versions, are listed in licenses/ffmpeg-build-README.txt.
  Build: gyan.dev full_build-shared,
    https://github.com/GyanD/codexffmpeg/releases/tag/$FFMPEG_VERSION
  Source: https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz
    (sources of the bundled libraries: see https://www.gyan.dev/ffmpeg/builds/)

libmpv (libmpv-2.dll), mpv commit $MPV_COMMIT
  License: GNU GPL version 2 or later (licenses/mpv-LICENSE.GPL.txt,
  licenses/mpv-Copyright.txt).
  Build: $MPV_BUILDS $MPV_TAG ($MPV_ASSET),
    https://github.com/$MPV_BUILDS/releases/tag/$MPV_TAG
  Source: https://github.com/mpv-player/mpv/tree/$MPV_COMMIT
    (build scripts and the versions of its dependencies:
    https://github.com/$MPV_BUILDS)

JASSUB $JASSUB_VERSION (built into flex.exe; draws subtitles in the web UI's player)
  JASSUB itself: MIT (licenses/jassub-LICENSE.txt). Its WebAssembly contains libass
  (ISC), FreeType (FTL or GPL 2), HarfBuzz (MIT), FriBidi (LGPL 2.1 or later) and their
  dependencies; Liberation Sans (SIL Open Font License) is its default font.
  Source, including those libraries: https://github.com/ThaUnknown/jassub/tree/v$JASSUB_VERSION

Rust crates compiled into flex.exe: licenses/rust-crates.txt
EOF

cat >"$OUT/release-notes.md" <<EOF
Run **$NAME-setup.exe** to install or update Flex: it installs for your account only (no admin prompt), and if Flex is running it's stopped first and started again afterwards. For unattended updates, run it with \`/SILENT\` or \`/VERYSILENT\`. Your libraries and history are kept in \`%APPDATA%\\Flex\`.

Or unzip **$NAME.zip** anywhere and run \`flex.exe\`. To update that way, quit Flex first (tray icon → Quit, or \`flex.exe --quit\`) and replace the files.

Bundled: FFmpeg $FFMPEG_VERSION ([gyan.dev](https://github.com/GyanD/codexffmpeg/releases/tag/$FFMPEG_VERSION)), libmpv [mpv@$MPV_COMMIT](https://github.com/mpv-player/mpv/tree/$MPV_COMMIT) ([$MPV_TAG](https://github.com/$MPV_BUILDS/releases/tag/$MPV_TAG)). Licenses and source links are in THIRD-PARTY-NOTICES.txt.
EOF

rm -f "$OUT/$NAME.zip"
(cd "$WORK" && "$SEVENZIP" a -tzip -mx=7 -bso0 "$OUT/$NAME.zip" "$NAME")
echo "Packaged $OUT/$NAME.zip"

if [[ -n $ISCC ]]; then
    winpath() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
    # MSYS would otherwise take the /D options for paths and rewrite them.
    MSYS2_ARG_CONV_EXCL='*' "$ISCC" /Q "/DAppVersion=$VERSION" "/DSourceDir=$(winpath "$DIR")" \
        "/DOutputDir=$(winpath "$OUT")" "/DOutputName=$NAME-setup" "$(winpath "$ROOT/.github/scripts/flex.iss")"
    echo "Packaged $OUT/$NAME-setup.exe"
else
    echo "ISCC isn't set: no installer"
fi
ls -la "$OUT"
