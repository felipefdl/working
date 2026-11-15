#!/usr/bin/env bash
# Writes crates/app/assets/fonts/WorkingEmoji.ttf, the color emoji font the Linux build draws every emoji with
# (docs/workarounds.md), and its license next to it. The font is Noto Color Emoji (CBDT bitmaps) from the pinned
# googlefonts/noto-emoji release with two changes:
#   - the family is "Working Emoji", so no installed font can shadow it;
#   - `m` maps to its space glyph: GPUI on Linux loads a named family only when it has a glyph for `m`.
# The PostScript name stays NotoColorEmoji: GPUI draws a font's color glyphs only under that name. The output is the
# same on every run (no new timestamp), so an unchanged pin leaves the file unchanged.
#
# Needs curl and uv (fontTools comes through it).
set -euo pipefail
cd "$(dirname "$0")/.."

release="v2026-09-24-unicode18_0"
base="https://raw.githubusercontent.com/googlefonts/noto-emoji/$release/2D/fonts"
font_sha256="15671215ab769fdc7162a045d56fd7d7e477c51b04e6b3c761d914d8fdd6cc44"
license_sha256="6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2"
out="crates/app/assets/fonts"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl --fail --location --silent --show-error --output "$work/NotoColorEmoji.ttf" "$base/NotoColorEmoji.ttf"
curl --fail --location --silent --show-error --output "$work/LICENSE" "$base/LICENSE"

mkdir -p "$out"
uv run --quiet --no-project --with fonttools==4.68.0 python - "$work" "$out" "$font_sha256" "$license_sha256" << 'EOF'
import hashlib
import shutil
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

work, out, font_sha256, license_sha256 = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4]
for name, expected in (("NotoColorEmoji.ttf", font_sha256), ("LICENSE", license_sha256)):
    actual = hashlib.sha256((work / name).read_bytes()).hexdigest()
    if actual != expected:
        sys.exit(f"{name}: sha256 {actual}, expected {expected}")

family = "Working Emoji"
font = TTFont(work / "NotoColorEmoji.ttf", recalcTimestamp=False)
if font["name"].getDebugName(6) != "NotoColorEmoji":
    sys.exit("the PostScript name is no longer NotoColorEmoji")
for table in font["cmap"].tables:
    if table.isUnicode():
        table.cmap[ord("m")] = "space"
names = font["name"]
names.names = [record for record in names.names if record.nameID not in (16, 17, 21, 22)]
for record in names.names:
    if record.nameID in (1, 3, 4):
        record.string = family
font.save(out / "WorkingEmoji.ttf")
shutil.copyfile(work / "LICENSE", out / "WorkingEmoji-LICENSE.txt")
EOF
echo "wrote $out/WorkingEmoji.ttf and $out/WorkingEmoji-LICENSE.txt"
