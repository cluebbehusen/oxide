#!/bin/sh
# The Oxide target's Run Script phase: builds the Rust shell for the
# device and puts the executable where Xcode signs and packages it.
# Xcode runs this with its build settings in the environment; see
# README's iPad section for the commands that drive it.
set -eu

# Xcode's script environment does not include the user's shell setup.
PATH="$HOME/.cargo/bin:$PATH"
cd "$SRCROOT/.."

# Xcode exports the iOS SDK for its own compiles. Cargo build scripts
# run on the Mac and must not inherit it; the iOS compiles find the SDK
# themselves. IPHONEOS_DEPLOYMENT_TARGET stays exported so rustc and the
# C dependencies link for the same minimum OS.
unset SDKROOT

if [ "$PLATFORM_NAME" = "iphonesimulator" ]; then
    TARGET=aarch64-apple-ios-sim
else
    TARGET=aarch64-apple-ios
fi

if [ "$CONFIGURATION" = "Release" ]; then
    cargo build -p oxide-shell --target "$TARGET" --locked --release
    BUILT="target/$TARGET/release/Oxide"
else
    cargo build -p oxide-shell --target "$TARGET" --locked
    BUILT="target/$TARGET/debug/Oxide"
fi

mkdir -p "$TARGET_BUILD_DIR/$EXECUTABLE_FOLDER_PATH"
cp "$BUILT" "$TARGET_BUILD_DIR/$EXECUTABLE_PATH"

# The app wears the workspace version, derived like the macOS bundle's.
VERSION="$(cargo pkgid -p oxide-shell)"
VERSION="${VERSION##*[@#]}"
/usr/libexec/PlistBuddy \
    -c "Set :CFBundleShortVersionString $VERSION" \
    -c "Set :CFBundleVersion $VERSION" \
    "$TARGET_BUILD_DIR/$INFOPLIST_PATH"

test -x "$TARGET_BUILD_DIR/$EXECUTABLE_PATH"
