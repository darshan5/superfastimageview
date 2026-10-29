#!/usr/bin/env bash
# Builds SuperFastImageView.app for Apple Silicon (run on macOS).
#   scripts/bundle-macos.sh            -> dist/SuperFastImageView.app
#   scripts/bundle-macos.sh --dmg      -> also dist/SuperFastImageView.dmg
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=aarch64-apple-darwin
APP=dist/SuperFastImageView.app
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

rustup target add "$TARGET" >/dev/null
cargo build --release --target "$TARGET"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/superfastimageview" "$APP/Contents/MacOS/SuperFastImageView"

# Icon: assets/icon.png (1024px) -> AppIcon.icns
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

sed "s/__VERSION__/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

# Ad-hoc signature so Apple Silicon will run it locally. For distribution,
# sign with a Developer ID and notarize instead.
codesign --force --deep --sign - "$APP"
echo "Built $APP"

if [[ "${1:-}" == "--dmg" ]]; then
  rm -f dist/SuperFastImageView.dmg
  hdiutil create -volname SuperFastImageView -srcfolder "$APP" -ov -format UDZO dist/SuperFastImageView.dmg
  echo "Built dist/SuperFastImageView.dmg"
fi
