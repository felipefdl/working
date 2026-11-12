#!/bin/sh
# Builds the optimized binary and wraps it as target/release/Working.app, so macOS shows the app's name and icon.
set -eu
cd "$(dirname "$0")/.."

cargo build --release -p working

app="target/release/Working.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/working "$app/Contents/MacOS/Working"
cp crates/app/assets/Info.plist "$app/Contents/Info.plist"
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
# Signed with the Apple Development certificate when the keychain has one: macOS ties permissions the user
# grants (screen recording, camera, microphone) to the signature, and an ad-hoc one changes with every build,
# so each rebuild would lose them. Without a certificate, ad-hoc is enough to launch.
identity="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
codesign --force --sign "${identity:--}" "$app" >/dev/null 2>&1
echo "$app"
