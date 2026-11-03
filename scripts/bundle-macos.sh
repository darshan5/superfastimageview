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
  # The usual drag-to-install layout: the app next to a link to /Applications.
  STAGE=$(mktemp -d)/SuperFastImageView
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f dist/SuperFastImageView.dmg
  RW=$(mktemp -d)/rw.dmg
  hdiutil create -volname SuperFastImageView -srcfolder "$STAGE" -ov -format UDRW "$RW"

  # Big icons, app on the left, Applications on the right. Needs Finder, so
  # it is best effort: without it the image still works, just unarranged.
  MNT=$(hdiutil attach "$RW" -readwrite -noverify -noautoopen | grep -o '/Volumes/.*$')
  osascript <<EOS || echo "warning: could not arrange the DMG window"
tell application "Finder"
  tell disk "$(basename "$MNT")"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, 740, 460}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set position of item "SuperFastImageView.app" of container window to {140, 150}
    set position of item "Applications" of container window to {400, 150}
    update without registering applications
    delay 1
    close
  end tell
end tell
EOS
  sync
  hdiutil detach "$MNT" -quiet || hdiutil detach "$MNT" -force
  hdiutil convert "$RW" -format UDZO -o dist/SuperFastImageView.dmg
  echo "Built dist/SuperFastImageView.dmg"
fi
