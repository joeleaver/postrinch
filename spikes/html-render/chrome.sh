#!/bin/bash
# Chrome baseline screenshots at the same viewport. No network in this sandbox.
cd "$(dirname "$0")"; mkdir -p out/chrome
P=$(mktemp -d -p "${SCRATCH:-/tmp}"); trap 'rm -rf -- "$P"' EXIT
for f in fixtures/*.html; do n=$(basename "$f" .html)
  timeout 30 google-chrome --headless=new --disable-gpu --no-sandbox --user-data-dir="$P" --hide-scrollbars --window-size=700,1100 --screenshot="$PWD/out/chrome/$n.png" "file://$PWD/$f" >/dev/null 2>&1
done
