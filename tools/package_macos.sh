#!/bin/sh
# Packages the shell as a macOS .app bundle under dist/Oxide.app.
#
# Layout: Contents/MacOS/Oxide beside Contents/Resources/{assets,
# scenarios} — the shell resolves resources relative to the executable
# when bundled (assets::resource_root), cwd otherwise, so the same
# binary serves both lives. Run from the workspace root:
#
#   sh tools/package_macos.sh
#
# Then smoke it:  dist/Oxide.app/Contents/MacOS/Oxide --debug-server
set -eu

cargo build --release -p oxide-shell

# The bundle wears the workspace version. Derived, never typed: a pinned
# literal here shipped three releases stamped 0.9.0.
VERSION="$(cargo pkgid -p oxide-shell)"
VERSION="${VERSION##*[@#]}"

APP=dist/Oxide.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp target/release/Oxide "$APP/Contents/MacOS/"
cp -R assets "$APP/Contents/Resources/assets"
cp -R scenarios "$APP/Contents/Resources/scenarios"

# NOTE: unquoted heredoc — $VERSION interpolates, so any future plist
# edit must shell-escape $, backticks, and backslashes.
cat > "$APP/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Oxide</string>
    <key>CFBundleDisplayName</key><string>Oxide</string>
    <key>CFBundleIdentifier</key><string>com.cluebbehusen.oxide</string>
    <key>CFBundleExecutable</key><string>Oxide</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleIconFile</key><string>oxide.icns</string>
</dict>
</plist>
PLIST

# Icon, best effort: an iconset from the real mark (tools/gen_icon.py).
# Missing tools or assets just mean a default-icon app.
if command -v sips > /dev/null && command -v iconutil > /dev/null \
    && [ -f assets/icon/oxide_1024.png ]; then
    ICONSET=$(mktemp -d)/oxide.iconset
    mkdir -p "$ICONSET"
    for SIZE in 16 32 128 256 512; do
        sips -z $SIZE $SIZE assets/icon/oxide_1024.png \
            --out "$ICONSET/icon_${SIZE}x${SIZE}.png" > /dev/null
        DOUBLE=$((SIZE * 2))
        sips -z $DOUBLE $DOUBLE assets/icon/oxide_1024.png \
            --out "$ICONSET/icon_${SIZE}x${SIZE}@2x.png" > /dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/oxide.icns"
fi

# Smoke: the bundle must carry its pieces and tell the truth about its
# version. set -eu turns any miss into a packaging failure.
test -x "$APP/Contents/MacOS/Oxide"
test -f "$APP/Contents/Resources/assets/sprites/atlas.png"
test -f "$APP/Contents/Resources/assets/sprites/atlas.json"
test -f "$APP/Contents/Resources/assets/sounds/music_menu.wav"
test -f "$APP/Contents/Resources/scenarios/skirmish.json"
grep -q "<string>$VERSION</string>" "$APP/Contents/Info.plist"

echo "packaged Oxide $VERSION -> $APP"
