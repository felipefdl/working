#!/bin/sh
# Builds the optimized binary and wraps it as target/release/Working.app, so macOS shows the app's name and icon.
#
# Release builds (scripts/release.sh) set two variables:
#   WORKING_BINARY            a prebuilt binary to wrap instead of building one
#   WORKING_SIGNING_IDENTITY  a Developer ID identity; signs with the hardened runtime and entitlements
set -eu
cd "$(dirname "$0")/.."

binary="${WORKING_BINARY:-}"
if [ -z "$binary" ]; then
  cargo build --release -p working
  binary="target/release/working"
fi
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

app="target/release/Working.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/Working"
cp crates/app/assets/Info.plist "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" -c "Set :CFBundleVersion $version" \
  "$app/Contents/Info.plist"
# Notification sounds the settings offer: a notification can only play a sound that sits in the app.
for sound in Glass Ping Pop Tink Hero Submarine Purr Funk; do
  cp "/System/Library/Sounds/$sound.aiff" "$app/Contents/Resources/"
done
# App icon: the Icon Composer document compiles to Assets.car, which holds its light and dark renditions, and to
# Working.icns for macOS versions before 26. Needs Xcode's actool.
actool_log="$(xcrun actool logo/Working.icon --compile "$app/Contents/Resources" --app-icon Working \
  --platform macosx --target-device mac --minimum-deployment-target 13.0 \
  --output-partial-info-plist /dev/null --output-format human-readable-text)" || {
  echo "$actool_log" >&2
  exit 1
}
if [ -n "${WORKING_SIGNING_IDENTITY:-}" ]; then
  codesign --force --options runtime --timestamp --entitlements crates/app/assets/Working.entitlements \
    --sign "$WORKING_SIGNING_IDENTITY" "$app"
  codesign --verify --strict --verbose=2 "$app"
else
  # Signed with the Apple Development certificate when the keychain has one: macOS ties permissions the user
  # grants (camera, microphone) to the signature, and an ad-hoc one changes with every build,
  # so each rebuild would lose them. Without a certificate, ad-hoc is enough to launch.
  identity="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
  codesign --force --sign "${identity:--}" "$app" >/dev/null 2>&1
fi
echo "$app"
