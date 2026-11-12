#!/usr/bin/env bash
# Builds the signed, notarized release into target/dist. Run by .github/workflows/publish.yml.
#
# Needs: WORKING_SIGNING_IDENTITY (Developer ID Application identity in the keychain), APPLE_API_KEY,
# APPLE_API_ISSUER, APPLE_API_KEY_PATH (App Store Connect key for notarytool), and
# CARGO_PACKAGER_SIGN_PRIVATE_KEY / CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD (minisign key for the updater).
#
# Output, for version X:
#   Working_X_universal.dmg              signed, notarized, stapled; what people and Homebrew install
#   Working_X_universal.app.tar.gz(.sig) the bundle the in-app updater downloads, with its minisign signature
set -euo pipefail
cd "$(dirname "$0")/.."

: "${WORKING_SIGNING_IDENTITY:?}" "${APPLE_API_KEY:?}" "${APPLE_API_ISSUER:?}" "${APPLE_API_KEY_PATH:?}"
: "${CARGO_PACKAGER_SIGN_PRIVATE_KEY:?}" "${CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD:?}"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
dist="target/dist"
rm -rf "$dist"
mkdir -p "$dist"

notarize() {
  xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" \
    --issuer "$APPLE_API_ISSUER" --wait --timeout 30m
}

# WORKING_UPDATES turns the self-updater on; only these builds have it.
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  WORKING_UPDATES=1 MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release --locked -p working --target "$target"
done
lipo -create -output target/release/working-universal \
  target/aarch64-apple-darwin/release/working target/x86_64-apple-darwin/release/working

WORKING_BINARY=target/release/working-universal ./scripts/bundle.sh
app="target/release/Working.app"

ditto -c -k --keepParent "$app" "$dist/notarize.zip"
notarize "$dist/notarize.zip"
rm "$dist/notarize.zip"
xcrun stapler staple "$app"

tarball="$dist/Working_${version}_universal.app.tar.gz"
tar -C target/release -czf "$tarball" Working.app
cargo packager signer sign "$tarball"

dmg="$dist/Working_${version}_universal.dmg"
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname Working -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
rm -rf "$stage"
codesign --force --timestamp --sign "$WORKING_SIGNING_IDENTITY" "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"

ls -l "$dist"
