#!/bin/sh
# Packages the shell as a macOS .app bundle under dist/Oxide.app.
#
# Layout: Contents/MacOS/oxide-shell beside Contents/Resources/{assets,
# scenarios} — the shell resolves resources relative to the executable
# when bundled (assets::resource_root), cwd otherwise, so the same
# binary serves both lives. Run from the workspace root:
#
#   sh tools/package_macos.sh
#
# Then smoke it:  dist/Oxide.app/Contents/MacOS/oxide-shell --debug-server
set -eu

cargo build --release -p oxide-shell

APP=dist/Oxide.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp target/release/oxide-shell "$APP/Contents/MacOS/"
cp -R assets "$APP/Contents/Resources/assets"
cp -R scenarios "$APP/Contents/Resources/scenarios"

cat > "$APP/Contents/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Oxide</string>
    <key>CFBundleDisplayName</key><string>Oxide</string>
    <key>CFBundleIdentifier</key><string>com.cluebbehusen.oxide</string>
    <key>CFBundleExecutable</key><string>oxide-shell</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.9.0</string>
    <key>CFBundleVersion</key><string>0.9.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>CFBundleIconFile</key><string>oxide.icns</string>
</dict>
</plist>
PLIST

# Icon, best effort: an iconset from the generated peak sprite scaled
# up. Missing tools or sprites just mean a default-icon app.
if command -v sips > /dev/null && command -v iconutil > /dev/null \
    && [ -f assets/sprites/peak_0.png ]; then
    ICONSET=$(mktemp -d)/oxide.iconset
    mkdir -p "$ICONSET"
    for SIZE in 16 32 128 256 512; do
        sips -z $SIZE $SIZE assets/sprites/peak_0.png \
            --out "$ICONSET/icon_${SIZE}x${SIZE}.png" > /dev/null
        DOUBLE=$((SIZE * 2))
        sips -z $DOUBLE $DOUBLE assets/sprites/peak_0.png \
            --out "$ICONSET/icon_${SIZE}x${SIZE}@2x.png" > /dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/oxide.icns"
fi

echo "packaged $APP"
