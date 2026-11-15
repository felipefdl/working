#!/usr/bin/env bash
# Builds the optimized binary and wraps it as target/dist/Working_<version>_<arch>.AppImage (<arch> is x86_64 or
# aarch64, the machine's own), the file people run and the in-app updater downloads: cargo-packager-updater writes
# the raw AppImage named in latest.json over the running one ($APPIMAGE).
#
# The AppImage carries the binary, its desktop entry (working.desktop, the app id its windows and notifications
# name), the icon from logo/, and the license of the emoji font built into the binary (crates/app/assets/fonts).
# GTK 3, WebKitGTK 4.1, and libsoup 3 come from the system, as for the binary itself: WebKitGTK runs helper
# processes from fixed system paths, so it cannot be moved into the image.
#
# Variables:
#   WORKING_BINARY                    a prebuilt binary to wrap instead of building one
#   WORKING_UPDATES=1                 builds the self-updater in (release builds only)
#   APPIMAGETOOL                      an appimagetool to use; else the official one is downloaded once into
#                                     target/appimage-tools
#   CARGO_PACKAGER_SIGN_PRIVATE_KEY   with CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD, also writes the minisign .sig the
#                                     updater verifies (cargo-packager 0.11.8: cargo install cargo-packager --locked)
set -euo pipefail
cd "$(dirname "$0")/.."

arch="$(uname -m)"
case "$arch" in
  x86_64 | aarch64) ;;
  *)
    echo "unsupported architecture: $arch" >&2
    exit 1
    ;;
esac
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

binary="${WORKING_BINARY:-}"
if [ -z "$binary" ]; then
  cargo build --release --locked -p working
  binary="${CARGO_TARGET_DIR:-target}/release/working"
fi

tool="${APPIMAGETOOL:-}"
if [ -z "$tool" ]; then
  tool="target/appimage-tools/appimagetool-$arch.AppImage"
  if [ ! -x "$tool" ]; then
    mkdir -p "$(dirname "$tool")"
    curl --fail --location --silent --show-error --output "$tool.part" \
      "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
    chmod +x "$tool.part"
    mv "$tool.part" "$tool"
  fi
fi

appdir="target/release/Working.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/512x512/apps" \
  "$appdir/usr/share/licenses/working"
install -m 755 "$binary" "$appdir/usr/bin/working"
install -m 644 logo/icon-light.png "$appdir/usr/share/icons/hicolor/512x512/apps/working.png"
install -m 644 crates/app/assets/fonts/WorkingEmoji-LICENSE.txt "$appdir/usr/share/licenses/working/"
cat > "$appdir/usr/share/applications/working.desktop" << EOF
[Desktop Entry]
Type=Application
Name=Working
GenericName=Chat
Comment=Google Chat on your desktop
Exec=working
Icon=working
Terminal=false
Categories=Network;InstantMessaging;Chat;
StartupWMClass=working
StartupNotify=true
X-AppImage-Version=$version
EOF
if command -v desktop-file-validate > /dev/null; then
  desktop-file-validate "$appdir/usr/share/applications/working.desktop"
fi
ln -s usr/share/applications/working.desktop "$appdir/working.desktop"
ln -s usr/share/icons/hicolor/512x512/apps/working.png "$appdir/working.png"
ln -s working.png "$appdir/.DirIcon"
cat > "$appdir/AppRun" << 'EOF'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/usr/bin/working" "$@"
EOF
chmod 755 "$appdir/AppRun"

dist="target/dist"
mkdir -p "$dist"
appimage="$dist/Working_${version}_${arch}.AppImage"
rm -f "$appimage" "$appimage.sig"
# Extract-and-run: appimagetool is an AppImage itself, and CI containers have no FUSE to mount it.
ARCH="$arch" VERSION="$version" "$tool" --appimage-extract-and-run --no-appstream "$appdir" "$appimage"

if [ -n "${CARGO_PACKAGER_SIGN_PRIVATE_KEY:-}" ]; then
  cargo packager signer sign "$appimage"
fi
ls -l "$dist"
