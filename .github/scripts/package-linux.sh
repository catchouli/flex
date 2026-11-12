#!/usr/bin/env bash
# Packages a Linux build: flex and the plugins, with every license and the third-party notices,
# and Flex's patched libmpv and what it needs when LIBMPV_DIR says where they are (from
# build-libmpv-linux.sh). ffmpeg, for the web UI, comes from the system.
#
#   package-linux.sh <tag or branch, e.g. v0.9.0> <cargo release dir> <output dir>
#
# Needs: bash, tar, cargo-about.
set -euo pipefail

TAG=$1
BIN=$2
OUT=$(mkdir -p "$3" && cd "$3" && pwd)
ROOT=$(cd "$(dirname "$0")/../.." && pwd)

fail() { echo "error: $*" >&2; exit 1; }
# shellcheck source=notices.sh
source "$ROOT/.github/scripts/notices.sh"

[[ -f $BIN/flex ]] || fail "no flex in $BIN (build with --release --workspace)"
NAME="flex-${TAG//\//-}-linux-x64"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DIR="$WORK/$NAME"
mkdir -p "$DIR/licenses"

cp "$BIN/flex" "$DIR/flex"
if [[ -n ${LIBMPV_DIR:-} ]]; then
    cp -R "$LIBMPV_DIR"/. "$DIR/"
    for f in mpv-version.txt mpv-LICENSE.GPL.txt mpv-Copyright.txt libmpv-deps/PACKAGES.txt; do
        [[ -f $DIR/licenses/$f ]] || fail "no licenses/$f in $LIBMPV_DIR (rebuild it with build-libmpv-linux.sh)"
    done
    compgen -G "$DIR/licenses/mpv-patches/*.patch" >/dev/null || fail "no licenses/mpv-patches in $LIBMPV_DIR"
    MPV_TAG=$(<"$DIR/licenses/mpv-version.txt")
fi
# Plugins: plugins/<id>/plugin.toml plus its program. They're off until switched on in
# Settings → Plugins.
for MANIFEST in "$ROOT"/plugins/*/plugin.toml; do
    ID=$(basename "$(dirname "$MANIFEST")")
    COMMAND=$(sed -n 's/^command *= *"\(.*\)"/\1/p' "$MANIFEST")
    [[ -f $BIN/$COMMAND ]] || fail "plugin $ID: no $BIN/$COMMAND (build with --workspace)"
    mkdir -p "$DIR/plugins/$ID"
    cp "$MANIFEST" "$BIN/$COMMAND" "$DIR/plugins/$ID/"
done

# Video effects: shaders and their settings, offered in Settings → Picture.
cp -R "$ROOT/effects" "$DIR/effects"
# The networks video effects use (flex checks they're the ones it was built with), and their licenses.
mkdir -p "$DIR/models"
cp "$ROOT"/models/*.bin "$ROOT"/models/*.safetensors "$DIR/models/"
copy_model_licenses "$ROOT" "$DIR/licenses"
cp "$ROOT/crates/flex-web/web/vendor/jassub/LICENSE.txt" "$DIR/licenses/jassub-LICENSE.txt"
LICENSE_FILE=$(ls "$ROOT"/LICENSE* 2>/dev/null | head -1 || true)
[[ -n $LICENSE_FILE ]] && cp "$LICENSE_FILE" "$DIR/LICENSE.txt"
(cd "$ROOT" && cargo about generate --fail about.hbs) >"$DIR/licenses/rust-crates.txt"

if [[ -n ${LIBMPV_DIR:-} ]]; then
    LIBMPV_README="Flex comes with its own patched libmpv.so.2, built on Debian 13, which keeps subtitles out of
video effects and gives them the real sound. If it can't load on your system, Flex uses the
system's libmpv instead (on Debian or Ubuntu, \"apt install libmpv2\"), without those two things."
else
    LIBMPV_README="Flex uses the system's libmpv (on Debian or Ubuntu, \"apt install libmpv2\")."
fi
cat >"$DIR/README.txt" <<EOF
Flex $TAG for Linux (x86_64)
https://github.com/catchouli/flex

Run ./flex. Libraries, watch history and settings are kept in ~/.local/share/flex
(or \$XDG_DATA_HOME/flex).

$LIBMPV_README
FLEX_LIBMPV=/path/to/it picks a different one. Optional:
- fonts-noto-cjk: Japanese, Chinese and Korean titles.
- ffmpeg: playing in the web UI (Settings → Web access) when a browser can't play a file
  as it is.
- Being in the "input" group: the remote's play/pause, stop and next/previous buttons
  (they're read from /dev/input). Arrows, OK and Back work either way.

Flex runs under Wayland (including kiosk compositors like cage) or X11. There's no tray
on Linux: closing the window quits.

The plugins folder has optional add-ons (an LED strip behind the TV, Trakt, AniList); they only run
once switched on in Settings → Plugins.

flex, the plugins, the effects folder and models/moge-segment.safetensors are Flex's own
(LICENSE.txt). Everything else in this folder is third-party software; see
THIRD-PARTY-NOTICES.txt.
EOF

{
    echo "Flex $TAG bundles the following software. Their licenses are in the licenses folder."
    echo
    if [[ -n ${LIBMPV_DIR:-} ]]; then
        cat <<EOF
libmpv (libmpv.so.2), mpv $MPV_TAG with Flex's patches
  License: GNU GPL version 2 or later (licenses/mpv-LICENSE.GPL.txt,
  licenses/mpv-Copyright.txt).
  Build: on Debian 13, by
    https://github.com/catchouli/flex/blob/$TAG/.github/scripts/build-libmpv-linux.sh
  Source: https://github.com/mpv-player/mpv/tree/$MPV_TAG, with Flex's patches applied
    (licenses/mpv-patches; also https://github.com/catchouli/flex/tree/$TAG/packaging/mpv)

The libraries libmpv needs (libmpv-deps), from Debian 13's packages
  Among them are FFmpeg's libraries (libavcodec and the rest) and libass. Which package each
  one is from, and that package's source package and version, are listed in
  licenses/libmpv-deps/PACKAGES.txt; each package's copyright file, with its license, is
  licenses/libmpv-deps/<package>.txt.
  Source: each source package at that version, from https://sources.debian.org or
    https://snapshot.debian.org (or "apt-get source <source package>=<version>").

EOF
    fi
    common_notices "$ROOT" "$TAG" flex
} >"$DIR/THIRD-PARTY-NOTICES.txt"

rm -f "$OUT/$NAME.tar.gz"
tar -C "$WORK" -czf "$OUT/$NAME.tar.gz" "$NAME"
echo "Packaged $OUT/$NAME.tar.gz"
