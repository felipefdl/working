#!/bin/sh
# Rebuilds the top-bar logo from logo/logo.svg, and the README icon from logo/Working.icon. Top bar: crates/app/assets/logo.png in white for the dark bar, and
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

# README icon: the app icon rendered from the Icon Composer document, light and dark. Needs Xcode.
ictool="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
for rendition in Default Dark; do
  name="$(echo "$rendition" | sed 's/Default/light/; s/Dark/dark/')"
  "$ictool" logo/Working.icon --export-image --output-file "logo/icon-$name.png" --platform macOS \
    --rendition "$rendition" --width 512 --height 512 --scale 1 >/dev/null
done
echo "Wrote logo/icon-light.png and logo/icon-dark.png"

# Windows icon: the light rendition at the sizes Explorer, the taskbar, and the installer use, embedded in the exe by
# crates/app/build.rs and used by the NSIS installer. Needs ImageMagick (brew install imagemagick).
magick logo/icon-light.png -define icon:auto-resize=256,64,48,40,32,24,20,16 crates/app/assets/working.ico
echo "Wrote crates/app/assets/working.ico"
