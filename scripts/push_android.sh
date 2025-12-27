#!/bin/bash
set -e

# =============================================================================
# Android Cross-Compile and Push Script
# =============================================================================
#
# Prerequisites:
#   - Android NDK installed (e.g., via Android Studio)
#   - Rust target: rustup target add aarch64-linux-android
#   - ADB installed and device connected
#
# Notes:
#   - Uses API level 24+ (required for getifaddrs)
#   - All TLS dependencies use rustls (no OpenSSL required)
#   - Output: target/aarch64-linux-android/release/merod
#
# =============================================================================

# NDK configuration
# Note: darwin-x86_64 is used for Rosetta cross-compile compatibility on macOS (Apple Silicon)
export NDK="$HOME/Library/Android/sdk/ndk/29.0.14206865"
NDK_TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64"

# Set up cross-compilation environment
export CC_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$NDK_TOOLCHAIN/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang"

# Build for Android
echo "Building merod for aarch64-linux-android..."
cargo build --release --target aarch64-linux-android -p merod

# Push binary to device
echo "Pushing merod to device..."
adb push target/aarch64-linux-android/release/merod /data/local/tmp/merod
adb shell chmod +x /data/local/tmp/merod

# Push libc++ runtime
echo "Pushing libc++ runtime..."
adb push \
  "$NDK_TOOLCHAIN/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so" \
  /data/local/tmp/

echo "Done! Binary is at /data/local/tmp/merod"
