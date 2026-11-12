#!/bin/sh
# Rebuilds the top-bar logo from logo/logo.svg: crates/app/assets/logo.png in white for the dark bar, and
# logo-light.png in near-black for the light one. Needs resvg (brew install resvg).
set -eu
cd "$(dirname "$0")/.."

css="$(mktemp)"
echo 'svg { color: #fff }' > "$css"
resvg -w 96 -h 96 --stylesheet "$css" logo/logo.svg crates/app/assets/logo.png
echo 'svg { color: #1f1f1f }' > "$css"
resvg -w 96 -h 96 --stylesheet "$css" logo/logo.svg crates/app/assets/logo-light.png
rm -f "$css"
echo "Wrote crates/app/assets/logo.png and logo-light.png"
