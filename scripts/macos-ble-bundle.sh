#!/usr/bin/env bash
# macOS refuses CoreBluetooth to a bare CLI binary: TCC sends SIGABRT the moment
# the Bluetooth thread starts, with no error message, unless the process belongs
# to a bundle declaring NSBluetoothAlwaysUsageDescription.
#
# This wraps a built binary in a minimal .app so it can ask for Bluetooth.
#
#     ./scripts/macos-ble-bundle.sh                 # bundle + run the scanner
#     ./scripts/macos-ble-bundle.sh meatpi-app      # bundle the dashboard
#
# The first run raises a system prompt; approve it once and macOS remembers.

set -euo pipefail
cd "$(dirname "$0")/.."

BIN_NAME="${1:-ble_scan}"

case "$BIN_NAME" in
ble_*)
    cargo build -p meatpi-core --features ble --example "$BIN_NAME"
    BIN_PATH="target/debug/examples/$BIN_NAME"
    ;;
*)
    cargo build --bin "$BIN_NAME"
    BIN_PATH="target/debug/$BIN_NAME"
    ;;
esac

APP="target/${BIN_NAME}.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN_PATH" "$APP/Contents/MacOS/$BIN_NAME"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>MeatPi ${BIN_NAME}</string>
    <key>CFBundleIdentifier</key><string>com.enochcarter.meatpi.${BIN_NAME}</string>
    <key>CFBundleExecutable</key><string>${BIN_NAME}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>NSBluetoothAlwaysUsageDescription</key>
    <string>Connects to the WiCAN Pro OBD-II adapter in your car.</string>
</dict>
</plist>
PLIST

# TCC keys its decision off the code signature, so sign ad-hoc — otherwise the
# grant is forgotten every rebuild.
codesign --force --sign - "$APP" >/dev/null 2>&1 || \
    echo "warning: ad-hoc codesign failed; the permission may not stick" >&2

echo "bundled: $APP"
exec "$APP/Contents/MacOS/$BIN_NAME"
