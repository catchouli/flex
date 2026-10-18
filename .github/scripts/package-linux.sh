#!/usr/bin/env bash
# Packages a Linux build: flex and the plugins, with the Rust crates' licenses. libmpv
# (and ffmpeg, for the web UI) come from the system, so nothing else is bundled.
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

[[ -f $BIN/flex ]] || fail "no flex in $BIN (build with --release --workspace)"
NAME="flex-${TAG//\//-}-linux-x64"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
DIR="$WORK/$NAME"
mkdir -p "$DIR/licenses"

cp "$BIN/flex" "$DIR/flex"
# Plugins: plugins/<id>/plugin.toml plus its program. They're off until switched on in
# Settings → Plugins.
for MANIFEST in "$ROOT"/plugins/*/plugin.toml; do
    ID=$(basename "$(dirname "$MANIFEST")")
    COMMAND=$(sed -n 's/^command *= *"\(.*\)"/\1/p' "$MANIFEST")
    [[ -f $BIN/$COMMAND ]] || fail "plugin $ID: no $BIN/$COMMAND (build with --workspace)"
    mkdir -p "$DIR/plugins/$ID"
    cp "$MANIFEST" "$BIN/$COMMAND" "$DIR/plugins/$ID/"
done

LICENSE_FILE=$(ls "$ROOT"/LICENSE* 2>/dev/null | head -1 || true)
[[ -n $LICENSE_FILE ]] && cp "$LICENSE_FILE" "$DIR/LICENSE.txt"
(cd "$ROOT" && cargo about generate --fail about.hbs) >"$DIR/licenses/rust-crates.txt"

cat >"$DIR/README.txt" <<EOF
Flex $TAG for Linux (x86_64)
https://github.com/catchouli/flex

Run ./flex. Libraries, watch history and settings are kept in ~/.local/share/flex
(or \$XDG_DATA_HOME/flex).

Flex needs libmpv from your system: on Debian or Ubuntu, "apt install libmpv2". It looks
for libmpv.so.2 next to flex and then on the usual library path; FLEX_LIBMPV=/path/to/it
overrides that. Optional:
- fonts-noto-cjk: Japanese, Chinese and Korean titles.
- ffmpeg: playing in the web UI (Settings → Web access) when a browser can't play a file
  as it is.
- Being in the "input" group: the remote's play/pause, stop and next/previous buttons
  (they're read from /dev/input). Arrows, OK and Back work either way.

Flex runs under Wayland (including kiosk compositors like cage) or X11. There's no tray
on Linux: closing the window quits.

The plugins folder has optional add-ons (an LED strip behind the TV, Trakt); they only run
once switched on in Settings → Plugins. The Rust crates' licenses are in licenses/.
EOF

rm -f "$OUT/$NAME.tar.gz"
tar -C "$WORK" -czf "$OUT/$NAME.tar.gz" "$NAME"
echo "Packaged $OUT/$NAME.tar.gz"
