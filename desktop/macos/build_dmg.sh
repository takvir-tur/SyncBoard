#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"
APP_BUNDLE="$BUILD_DIR/ClipSync.app"
DMG_PATH="$SCRIPT_DIR/ClipSync-macos-universal.dmg"
if [ -z "${SDKROOT:-}" ]; then
    DETECTED_SDK="$(xcrun --sdk macosx --show-sdk-path)"
    if [[ "$DETECTED_SDK" == *MacOSX27* ]] && [ -d "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk" ]; then
        DETECTED_SDK="$(cd /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk && pwd -P)"
    fi
    export SDKROOT="$DETECTED_SDK"
fi
SDK_PATH="$SDKROOT"
export MACOSX_DEPLOYMENT_TARGET=13.0

rm -rf "$BUILD_DIR" "$DMG_PATH"
mkdir -p "$BUILD_DIR/universal-lib" "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Resources"

rustup target add aarch64-apple-darwin x86_64-apple-darwin

echo "Building Rust core (arm64 + x86_64)..."
cargo build --release --manifest-path "$REPO_ROOT/core/Cargo.toml" --target aarch64-apple-darwin
cargo build --release --manifest-path "$REPO_ROOT/core/Cargo.toml" --target x86_64-apple-darwin

lipo -create \
    "$REPO_ROOT/target/aarch64-apple-darwin/release/libclipsync_core.a" \
    "$REPO_ROOT/target/x86_64-apple-darwin/release/libclipsync_core.a" \
    -output "$BUILD_DIR/universal-lib/libclipsync_core.a"

echo "Building Swift menu-bar agent (arm64 + x86_64)..."
SWIFT_FLAGS=(
    -O
    -swift-version 5
    -sdk "$SDK_PATH"
    -I "$SCRIPT_DIR/Sources/CClipSync/include"
    "$BUILD_DIR/universal-lib/libclipsync_core.a"
    -framework AppKit
    -framework SwiftUI
    -framework CoreImage
    -framework Security
    -framework ServiceManagement
    -framework SystemConfiguration
)

swiftc "${SWIFT_FLAGS[@]}" -target arm64-apple-macosx13.0 \
    -o "$BUILD_DIR/ClipSync-arm64" "$SCRIPT_DIR"/Sources/ClipSyncMac/*.swift

swiftc "${SWIFT_FLAGS[@]}" -target x86_64-apple-macosx13.0 \
    -o "$BUILD_DIR/ClipSync-x86_64" "$SCRIPT_DIR"/Sources/ClipSyncMac/*.swift

lipo -create "$BUILD_DIR/ClipSync-arm64" "$BUILD_DIR/ClipSync-x86_64" \
    -output "$APP_BUNDLE/Contents/MacOS/ClipSync"
strip -x "$APP_BUNDLE/Contents/MacOS/ClipSync"

cp "$SCRIPT_DIR/Info.plist" "$APP_BUNDLE/Contents/Info.plist"
codesign --force --deep --sign - "$APP_BUNDLE" || true

mkdir -p "$BUILD_DIR/dmg-staging"
cp -R "$APP_BUNDLE" "$BUILD_DIR/dmg-staging/"
ln -s /Applications "$BUILD_DIR/dmg-staging/Applications"

hdiutil create -volname "ClipSync" \
    -srcfolder "$BUILD_DIR/dmg-staging" \
    -ov -format UDZO \
    "$DMG_PATH"

ls -lh "$DMG_PATH"
