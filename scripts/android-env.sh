#!/usr/bin/env bash
# Source this (don't execute it) to point cargo at the Android NDK:
#
#     source scripts/android-env.sh
#
# Picks the newest complete NDK under $ANDROID_HOME/ndk. Override by setting
# ANDROID_NDK_HOME yourself before sourcing.

: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
export ANDROID_HOME

if [ -z "${ANDROID_NDK_HOME:-}" ]; then
    # Some NDK directories exist but are empty from an interrupted install, so
    # require the toolchain to actually be there.
    for candidate in $(ls -1r "$ANDROID_HOME/ndk" 2>/dev/null); do
        if [ -d "$ANDROID_HOME/ndk/$candidate/toolchains/llvm/prebuilt" ]; then
            export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$candidate"
            break
        fi
    done
fi

if [ -z "${ANDROID_NDK_HOME:-}" ]; then
    echo "No usable NDK found under $ANDROID_HOME/ndk" >&2
    echo "Install one with: sdkmanager 'ndk;27.0.12077973'" >&2
    return 1 2>/dev/null || exit 1
fi

HOST_TAG=$(ls -1 "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt" | head -1)
NDK_BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin"

# min_sdk_version 26 — must match [package.metadata.android.sdk] in Cargo.toml.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android26-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"

echo "ANDROID_NDK_HOME=$ANDROID_NDK_HOME"
