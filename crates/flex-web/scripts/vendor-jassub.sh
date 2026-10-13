#!/usr/bin/env bash
# Rebuilds web/vendor/jassub: JASSUB (libass for the browser, draws ASS subtitles) bundled
# into plain files, so the web UI needs no internet and no build step. Run after
# changing JASSUB_VERSION; needs node/npm.
set -euo pipefail

JASSUB_VERSION=2.5.16
ESBUILD_VERSION=0.28.2

OUT=$(cd "$(dirname "$0")/.." && pwd)/web/vendor/jassub
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"
npm init -y >/dev/null
npm install --silent --ignore-scripts --no-audit --no-fund "jassub@$JASSUB_VERSION" "esbuild@$ESBUILD_VERSION"
J=node_modules/jassub
rm -rf "$OUT" && mkdir -p "$OUT"
# The page script and the worker, each with its npm dependencies bundled in. Worker and
# WebAssembly URLs are passed in explicitly (see web/subtitles.js).
npx esbuild "$J/dist/jassub.js" --bundle --format=esm --minify --target=es2020 --outfile="$OUT/jassub.js" --log-level=warning
npx esbuild "$J/dist/worker/worker.js" --bundle --format=esm --minify --target=es2020 --outfile="$OUT/worker.js" --log-level=warning
cp "$J/dist/wasm/jassub-worker.wasm" "$J/dist/wasm/jassub-worker-modern.wasm" "$J/dist/default.woff2" "$OUT/"
cp "$J/LICENSE" "$OUT/LICENSE.txt"
cat >"$OUT/README.txt" <<NOTE
JASSUB $JASSUB_VERSION (https://github.com/ThaUnknown/jassub), bundled with esbuild by
crates/flex-web/scripts/vendor-jassub.sh. Licenses: $(node -p "require('./$J/package.json').license").
The WebAssembly is libass and its dependencies (FreeType, HarfBuzz, FriBidi, ...).
default.woff2 is JASSUB's default font (Liberation Sans, SIL Open Font License).
NOTE
ls -la "$OUT"
