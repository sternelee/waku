#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# build.sh — Build & run the Waku mobile app on iOS or Android.
#
# Usage:
#   ./build.sh ios [--simulator|--device] [--release] [--no-run]
#   ./build.sh android [--emulator|--device] [--release] [--no-run]
#
# Prerequisites:
#   iOS:     Xcode, XcodeGen (brew install xcodegen), rustup target
#            aarch64-apple-ios / aarch64-apple-ios-sim
#   Android: Android SDK + NDK, cargo-ndk (cargo install cargo-ndk),
#            rustup target aarch64-linux-android
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# SCRIPT_DIR = mobile/
MOBILE_ROOT="$SCRIPT_DIR"
IOS_DIR="$MOBILE_ROOT/ios"
ANDROID_GRADLE_DIR="$MOBILE_ROOT/android/gradle"

info()  { echo -e "\033[0;32m▸\033[0m $*"; }
error() { echo -e "\033[0;31m✘\033[0m $*" >&2; }
step()  { echo -e "\n\033[1;36m══ $* ══\033[0m\n"; }

PLATFORM="${1:-}"
TARGET_KIND="device"
PROFILE="debug"
NO_RUN=false

case "${PLATFORM}" in
    ios|android) shift ;;
    -h|--help|"") 
        sed -n '1,20p' "$0" | grep '^#' | sed 's/^# \{0,1\}//'
        exit 0 ;;
    *) error "Unknown subcommand: $1"; exit 1 ;;
esac

while [[ $# -gt 0 ]]; do
    case "$1" in
        --simulator) TARGET_KIND="simulator" ;;
        --emulator)  TARGET_KIND="emulator" ;;
        --device)    TARGET_KIND="device" ;;
        --release)   PROFILE="release" ;;
        --no-run)    NO_RUN=true ;;
        *) error "Unknown option: $1"; exit 1 ;;
    esac
    shift
done

# ═════════════════════════════════════════════════════════════════════════════
# iOS
# ═════════════════════════════════════════════════════════════════════════════

build_ios() {
    step "iOS — ${PROFILE} — ${TARGET_KIND}"

    local rust_target xcode_sdk xcode_config cargo_flag
    if [[ "$TARGET_KIND" == "simulator" ]]; then
        rust_target="aarch64-apple-ios-sim"
        xcode_sdk="iphonesimulator"
        xcode_destination="generic/platform=iOS Simulator"
    else
        rust_target="aarch64-apple-ios"
        xcode_sdk="iphoneos"
        xcode_destination="generic/platform=iOS"
    fi

    if [[ "$PROFILE" == "release" ]]; then
        cargo_flag="--release"
        xcode_config="Release"
    else
        cargo_flag=""
        xcode_config="Debug"
    fi

    info "Ensuring Rust target ${rust_target}..."
    rustup target add "$rust_target" 2>/dev/null || true

    step "Building waku-mobile for ${rust_target}"
    cd "$MOBILE_ROOT"
    cargo build --target "$rust_target" $cargo_flag

    if ! command -v xcodegen &>/dev/null; then
        error "XcodeGen not found. Install it with: brew install xcodegen"
        exit 1
    fi

    step "Generating Xcode project"
    cd "$IOS_DIR"
    xcodegen generate --spec project.yml
    info "Project at: $IOS_DIR/WakuMobile.xcodeproj"

    step "Building Xcode project (${xcode_config}, ${xcode_sdk})"
    local build_dir="$IOS_DIR/build"
    mkdir -p "$build_dir"
    xcodebuild \
        -project WakuMobile.xcodeproj \
        -scheme WakuMobile \
        -configuration "$xcode_config" \
        -destination "$xcode_destination" \
        -derivedDataPath "$build_dir" \
        CODE_SIGN_STYLE=Automatic \
        build 2>&1 | tail -25

    info "Xcode build complete."
    if $NO_RUN; then
        return 0
    fi

    if [[ "$TARGET_KIND" == "simulator" ]]; then
        step "Installing on iOS Simulator"
        local app_path
        app_path=$(find "$build_dir" -path "*/Build/Products/${xcode_config}-iphonesimulator/Waku.app" -type d | head -1)
        [[ -n "$app_path" ]] || { error "Could not find .app bundle"; exit 1; }

        local sim_id
        sim_id=$(xcrun simctl list devices available | grep "iPhone" | head -1 | sed -E 's/.*\(([A-F0-9-]+)\).*/\1/')
        xcrun simctl boot "$sim_id" 2>/dev/null || true
        open -a Simulator 2>/dev/null || true
        xcrun simctl install "$sim_id" "$app_path"
        xcrun simctl launch "$sim_id" dev.waku.mobile
        info "App launched on simulator! 🚀"
    else
        info "Device build done — install with Xcode or devicectl."
    fi
}

# ═════════════════════════════════════════════════════════════════════════════
# Android
# ═════════════════════════════════════════════════════════════════════════════

build_android() {
    step "Android — ${PROFILE}"

    if ! command -v cargo-ndk &>/dev/null; then
        error "cargo-ndk not found. Install it with: cargo install cargo-ndk"
        exit 1
    fi

    rustup target add aarch64-linux-android 2>/dev/null || true

    step "Building waku-mobile cdylib via cargo-ndk"
    cd "$MOBILE_ROOT"
    local cargo_flag=""
    [[ "$PROFILE" == "release" ]] && cargo_flag="--release"
    cargo ndk -t arm64-v8a -P 31 -o android/gradle/app/src/main/jniLibs \
        build $cargo_flag

    step "Assembling APK via Gradle"
    cd "$ANDROID_GRADLE_DIR"
    local gradle_flag=""
    [[ "$PROFILE" == "release" ]] && gradle_flag="--release"
    ./gradlew assembleDebug $gradle_flag

    local apk="app/build/outputs/apk/debug/app-debug.apk"
    [[ "$PROFILE" == "release" ]] && apk="app/build/outputs/apk/release/app-release-unsigned.apk"
    info "APK ready: $apk"

    if $NO_RUN; then
        return 0
    fi

    if [[ "$TARGET_KIND" == "emulator" ]]; then
        adb install -r "$apk"
        adb shell am start -n dev.waku.mobile/dev.waku.mobile.GpuiActivity
        info "App launched on emulator! 🚀"
    else
        info "Install with: adb install -r $apk"
    fi
}

if [[ "$PLATFORM" == "ios" ]]; then
    build_ios
else
    build_android
fi
