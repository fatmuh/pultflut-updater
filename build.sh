#!/bin/bash
# Build pultflut_updater for Android (all ABIs)
#
# Usage: ./build.sh [release|debug]
#
# Output: dist/lib/<abi>/libpultflut_updater.so

set -e

PROFILE="${1:-release}"
NDK="${ANDROID_NDK_HOME:-C:/Users/FM/AppData/Local/Android/Sdk/ndk/28.2.13676358}"

echo "=== Build pultflut-updater for Android ($PROFILE) ==="
echo "NDK: $NDK"

cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 build --$PROFILE

# Copy to dist/
DIST="dist/lib"
rm -rf "$DIST"
mkdir -p "$DIST/arm64-v8a" "$DIST/armeabi-v7a" "$DIST/x86_64" "$DIST/x86"

cp target/aarch64-linux-android/$PROFILE/libpultflut_updater.so "$DIST/arm64-v8a/"
cp target/armv7-linux-androideabi/$PROFILE/libpultflut_updater.so "$DIST/armeabi-v7a/"
cp target/x86_64-linux-android/$PROFILE/libpultflut_updater.so "$DIST/x86_64/"
cp target/i686-linux-android/$PROFILE/libpultflut_updater.so "$DIST/x86/"

echo ""
echo "=== Done ==="
ls -la "$DIST"/*/libpultflut_updater.so
