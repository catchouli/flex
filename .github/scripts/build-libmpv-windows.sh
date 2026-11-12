#!/usr/bin/env bash
# Builds a single-file libmpv-2.dll with Flex's patches, using the same toolchain as the libmpv
# builds Flex used to ship (shinchiro's mpv-winbuild-cmake), cross-compiled on Linux. Takes hours
# the first time; run again with the same work folder and it carries on from where it got to.
#
#   build-libmpv-windows.sh <output dir> [work dir]
#
# Needs the host tools in mpv-winbuild-cmake's README (on Debian or Ubuntu, its apt-get line),
# and meson, mako, jsonschema and rst2pdf from pip on the PATH.
set -euo pipefail

OUT=$(mkdir -p "$1" && cd "$1" && pwd)
WORK=${2:-$(mktemp -d)}
WORK=$(mkdir -p "$WORK" && cd "$WORK" && pwd)
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TOOLCHAIN=7e36e1eab5094a642c5a3c1771551f06b5ec3224
MPV_COMMIT=$(cat "$ROOT/packaging/mpv/windows/COMMIT")
PATCHES=$(ls "$ROOT"/packaging/mpv/windows/*.patch | tr '\n' ' ')

# Some of the packages' own steps commit; this needs nobody's git settings changing.
export GIT_AUTHOR_NAME=flex-build GIT_AUTHOR_EMAIL=build@localhost GIT_COMMITTER_NAME=flex-build GIT_COMMITTER_EMAIL=build@localhost
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0='*'

if [[ ! -d $WORK/toolchain ]]; then
    git clone https://github.com/shinchiro/mpv-winbuild-cmake "$WORK/toolchain"
fi
cd "$WORK/toolchain"
git checkout -q -f "$TOOLCHAIN"
sed -i "s|^    UPDATE_COMMAND \"\"|    GIT_TAG $MPV_COMMIT\n    UPDATE_COMMAND \"\"\n    PATCH_COMMAND git -C <SOURCE_DIR> apply $PATCHES|" packages/mpv.cmake
grep -q "GIT_TAG $MPV_COMMIT" packages/mpv.cmake
# The recipes take each library's newest code. OpenSSL's now puts Brotli, zstd and zlib into
# libcrypto, which ngtcp2's QUIC check doesn't link with; TLS compression isn't needed here.
sed -i 's/^        enable-brotli$/        no-brotli/; s/^        enable-zstd$/        no-zstd/' packages/openssl.cmake
sed -i '/Configure$/,/BUILD_COMMAND/ s/^        zlib$/        no-zlib/' packages/openssl.cmake
grep -q "no-brotli" packages/openssl.cmake && grep -q "no-zlib" packages/openssl.cmake
# libssh used to get zlib through libcrypto; FFmpeg's sftp:// support isn't needed, so leave it out
# rather than mend its link.
sed -i '/^        --enable-libssh$/d' packages/ffmpeg.cmake
! grep -q -- "--enable-libssh" packages/ffmpeg.cmake

BUILD=build_x86_64
mkdir -p clang_root
cmake -DTARGET_ARCH=x86_64-w64-mingw32 -DSINGLE_SOURCE_LOCATION="$PWD/src_packages" -DRUSTUP_LOCATION="$PWD/clang_root/install_rustup" -G Ninja -B "$BUILD" -S "$PWD"
ninja -C "$BUILD" download || true
if [[ ! -e $BUILD/.gcc-done ]]; then
    ninja -C "$BUILD" gcc
    rm -rf "$BUILD/toolchain"
    touch "$BUILD/.gcc-done"
fi
ninja -C "$BUILD" mpv
DLL=$(find "$BUILD" -name libmpv-2.dll -size +1M | head -1)
[[ -n $DLL ]] || { echo "error: no libmpv-2.dll was built" >&2; exit 1; }
for symbol in mpv_flex_set_audio_tap mpv_flex_set_sound; do
    grep -q "$symbol" "$DLL" || { echo "error: the DLL isn't patched ($symbol is missing)" >&2; exit 1; }
done
cp "$DLL" "$OUT/libmpv-2.dll"
