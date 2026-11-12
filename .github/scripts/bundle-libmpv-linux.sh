#!/usr/bin/env bash
# Copies a libmpv.so.2 and the libraries it needs into a folder that can sit beside flex, with
# each one finding the others there. What belongs to the machine (the C library, graphics,
# sound, the display) is left out, along with any library that has to match its drivers.
#
#   bundle-libmpv-linux.sh <libmpv.so.2> <output dir>
#
# Needs: bash, ldd, patchelf, dpkg (for the copyright files and which packages they're from).
set -euo pipefail

LIBMPV=$1
OUT=$(mkdir -p "$2" && cd "$2" && pwd)
HOST='^(ld-linux|libc|libm|libdl|libpthread|librt|libresolv|libgcc_s|libstdc\+\+|libGL|libEGL|libGLX|libGLdispatch|libOpenGL|libgbm|libdrm|libva|libvdpau|libX|libxcb|libxkbcommon|libwayland|libasound|libpulse|libpipewire|libdbus|libsystemd|libudev|libcap|libelf)'

fail() { echo "error: $*" >&2; exit 1; }

mkdir -p "$OUT/libmpv-deps" "$OUT/licenses/libmpv-deps"
# Each library bundled, the Debian package it's from, and that package's source package.
PACKAGES=$OUT/licenses/libmpv-deps/PACKAGES.txt
printf 'library\tpackage version\tsource package version\n' >"$PACKAGES"
cp -L "$LIBMPV" "$OUT/libmpv.so.2"
patchelf --set-rpath '$ORIGIN/libmpv-deps' "$OUT/libmpv.so.2"
ldd "$LIBMPV" | awk '/=> \// { print $3 }' | while read -r lib; do
    name=$(basename "$lib")
    [[ $name =~ $HOST ]] && continue
    cp -L "$lib" "$OUT/libmpv-deps/$name"
    chmod u+w "$OUT/libmpv-deps/$name"
    patchelf --set-rpath '$ORIGIN' "$OUT/libmpv-deps/$name"
    # Its license, and which Debian package it comes from (and that package's source), for the
    # notices. A library whose license can't be found stops the build.
    real=$(readlink -f "$lib")
    owner=$(dpkg -S "$real" 2>/dev/null || dpkg -S "${real#/usr}" 2>/dev/null) ||
        fail "$name ($real) isn't from a Debian package, so its license can't be found"
    owner=$(head -1 <<<"$owner" | sed 's/: .*//') # "package:arch", or "package"
    package=${owner%%:*}
    [[ -f /usr/share/doc/$package/copyright ]] || fail "$name: $package has no /usr/share/doc/$package/copyright"
    cp "/usr/share/doc/$package/copyright" "$OUT/licenses/libmpv-deps/$package.txt"
    dpkg-query -W -f="$name\t$package \${Version}\t\${source:Package} \${source:Version}\n" "$owner" >>"$PACKAGES"
done
echo "Bundled $(ls "$OUT/libmpv-deps" | wc -l) libraries, $(du -sh "$OUT" | cut -f1)"
