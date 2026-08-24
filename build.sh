#!/usr/bin/env bash
# Build the browser application into dist/.
#
#   ./build.sh            release build
#   ./build.sh --dev      faster build, much larger .wasm, panics keep their messages
#
# Output is a directory of static files. There is no server component: copy dist/
# anywhere that serves files and it works, as long as .wasm is served as
# application/wasm.

set -euo pipefail
cd "$(dirname "$0")"

PROFILE=--release
if [[ "${1:-}" == "--dev" ]]; then
  PROFILE=--dev
fi

rm -rf dist
mkdir -p dist

echo "==> compiling the engine and interface to WebAssembly"
wasm-pack build crates/slice-web \
  --target web \
  --out-dir ../../dist/pkg \
  --out-name slice_web \
  --no-typescript \
  "$PROFILE"

# wasm-pack leaves packaging metadata behind that a static site has no use for.
rm -f dist/pkg/package.json dist/pkg/.gitignore dist/pkg/README.md dist/pkg/LICENSE*

echo "==> copying the page"
cp -R web/. dist/

# The logo lives with the documentation site, which needs it inside `docs/` for
# Jekyll to publish it. Copying rather than duplicating keeps one canonical file,
# so the application and the website cannot show different marks.
cp docs/assets/slice-icon.svg dist/

echo
echo "dist/ is ready:"
du -h dist/pkg/slice_web_bg.wasm | awk '{print "  wasm  " $1}'
echo
echo "Serve it with any static file server, for example:"
echo "  python3 -m http.server --directory dist 8080"
