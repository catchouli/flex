#!/usr/bin/env bash
# Builds libmpv 0.40.0 with Flex's patches and bundles it with what it needs, on Debian 13.
#
#   build-libmpv-linux.sh <output dir>
#
# In CI it runs as root and installs what it needs; elsewhere they have to be installed already.
set -euo pipefail

OUT=$1
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
MPV_TAG=v0.40.0

PACKAGES="build-essential ca-certificates git meson ninja-build nasm patchelf pkg-config libavcodec-dev libavfilter-dev libavformat-dev libavutil-dev libswresample-dev libswscale-dev libass-dev libplacebo-dev libluajit-5.1-dev libpulse-dev libpipewire-0.3-dev libasound2-dev libegl-dev libgl-dev libgbm-dev libx11-dev libxext-dev libxinerama-dev libxrandr-dev libxpresent-dev libxss-dev libxpresent-dev libxkbcommon-dev libwayland-dev wayland-protocols libva-dev libdrm-dev libvdpau-dev libjpeg-dev zlib1g-dev python3"
if [[ $EUID -eq 0 ]]; then
    apt-get update
    apt-get install -y --no-install-recommends $PACKAGES
else
    # Not as root (on a desktop rather than in CI): they have to be there already.
    MISSING=$(for p in $PACKAGES; do dpkg -s "$p" >/dev/null 2>&1 || printf '%s ' "$p"; done)
    [[ -z $MISSING ]] || { echo "error: install these first: sudo apt-get install $MISSING" >&2; exit 1; }
fi

WORK=$(mktemp -d)
git clone --depth 1 -b "$MPV_TAG" https://github.com/mpv-player/mpv "$WORK/mpv"
for patch in "$ROOT"/packaging/mpv/v0.40.0/*.patch; do
    git -C "$WORK/mpv" apply "$patch"
done
meson setup "$WORK/build" "$WORK/mpv" -Dbuildtype=release -Dlibmpv=true -Dcplayer=false -Dmanpage-build=disabled \
    -Dlua=enabled -Djavascript=disabled -Dcdda=disabled -Ddvdnav=disabled -Dvulkan=disabled
ninja -C "$WORK/build"
"$ROOT/.github/scripts/bundle-libmpv-linux.sh" "$WORK/build/libmpv.so.2" "$OUT"
cp "$WORK/mpv/LICENSE.GPL" "$OUT/licenses/mpv-LICENSE.GPL.txt"
cp "$WORK/mpv/Copyright" "$OUT/licenses/mpv-Copyright.txt"
echo "$MPV_TAG" >"$OUT/licenses/mpv-version.txt"
# Flex's changes to mpv, which go with its source in the notices.
mkdir -p "$OUT/licenses/mpv-patches"
cp "$ROOT"/packaging/mpv/v0.40.0/*.patch "$OUT/licenses/mpv-patches/"
